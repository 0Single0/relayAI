// Package legacyimport migrates encrypted WorkBuddy accounts without changing target secrets.
package legacyimport

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
)

type Channel struct {
	ID       string          `json:"id"`
	Name     string          `json:"name"`
	Adapter  string          `json:"adapter"`
	Enabled  bool            `json:"enabled"`
	Priority int             `json:"priority"`
	Weight   int             `json:"weight"`
	Options  json.RawMessage `json:"options"`
}

type Account struct {
	ChannelID   string `json:"channel_id"`
	UID         string `json:"uid"`
	Nickname    string `json:"nickname"`
	Enabled     int    `json:"enabled"`
	Priority    *int   `json:"priority_override"`
	Weight      *int   `json:"weight_override"`
	MaxInflight *int   `json:"max_inflight"`
	Credential  string `json:"credential"`
	AddedAt     int64  `json:"added_at"`
	Source      string `json:"source"`
}

type Bundle struct {
	Version  int       `json:"version"`
	Channels []Channel `json:"channels"`
	Accounts []Account `json:"accounts"`
}

type preparedAccount struct {
	Account
	Plaintext []byte
}

type Prepared struct {
	Channels []Channel
	accounts []preparedAccount
}

func ReadKey(raw string) ([]byte, error) {
	key, err := base64.StdEncoding.DecodeString(strings.TrimSpace(raw))
	if err != nil || len(key) != 32 {
		return nil, errors.New("master key must be base64-encoded 32 bytes")
	}
	return key, nil
}

func open(key []byte, sealed string) ([]byte, error) {
	b, err := base64.StdEncoding.DecodeString(sealed)
	if err != nil {
		return nil, errors.New("invalid credential encoding")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, errors.New("invalid encryption key")
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil || len(b) < gcm.NonceSize()+gcm.Overhead() {
		return nil, errors.New("invalid credential envelope")
	}
	plain, err := gcm.Open(nil, b[:gcm.NonceSize()], b[gcm.NonceSize():], nil)
	if err != nil {
		return nil, errors.New("credential authentication failed")
	}
	return plain, nil
}

func seal(key, plaintext []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, errors.New("invalid encryption key")
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err = rand.Read(nonce); err != nil {
		return nil, errors.New("random nonce generation failed")
	}
	sealed := gcm.Seal(nonce, nonce, plaintext, nil)
	// Gateway stores UTF-8 base64 bytes in bytea, rather than raw AES bytes.
	return []byte(base64.StdEncoding.EncodeToString(sealed)), nil
}

func identity(options json.RawMessage) (string, error) {
	var opts struct {
		Realm    string `json:"realm"`
		Outbound string `json:"outbound_identity"`
	}
	if json.Unmarshal(options, &opts) != nil || (opts.Realm != "cn" && opts.Realm != "intl") ||
		(opts.Outbound != "workbuddy" && opts.Outbound != "codebuddy") {
		return "", errors.New("unsupported WorkBuddy channel options")
	}
	return opts.Realm + "/" + opts.Outbound, nil
}

func Load(bundlePath, keyPath string) (*Prepared, error) {
	data, err := os.ReadFile(bundlePath)
	if err != nil {
		return nil, errors.New("cannot read account bundle")
	}
	var b Bundle
	if json.Unmarshal(data, &b) != nil || b.Version != 1 || len(b.Accounts) == 0 {
		return nil, errors.New("invalid or empty account bundle")
	}
	keyData, err := os.ReadFile(keyPath)
	if err != nil {
		return nil, errors.New("cannot read source master key file")
	}
	key, err := ReadKey(string(keyData))
	if err != nil {
		return nil, err
	}
	return prepare(b, key)
}

func prepare(b Bundle, key []byte) (*Prepared, error) {
	channels := make(map[string]bool)
	for _, c := range b.Channels {
		if c.ID == "" || c.Name == "" || c.Adapter != "workbuddy" || channels[c.ID] || c.Priority < 0 || c.Weight < 1 {
			return nil, errors.New("invalid source channel")
		}
		if _, err := identity(c.Options); err != nil {
			return nil, err
		}
		channels[c.ID] = true
	}
	p := &Prepared{Channels: b.Channels}
	uids := make(map[string]bool)
	for i, a := range b.Accounts {
		if !channels[a.ChannelID] || strings.TrimSpace(a.UID) == "" || uids[a.UID] || a.AddedAt <= 0 || (a.Enabled != 0 && a.Enabled != 1) {
			return nil, fmt.Errorf("invalid or duplicate source account at row %d", i+1)
		}
		for _, override := range []*int{a.Priority, a.Weight, a.MaxInflight} {
			if override != nil && *override < 0 {
				return nil, fmt.Errorf("negative override at row %d", i+1)
			}
		}
		plain, err := open(key, a.Credential)
		if err != nil {
			return nil, fmt.Errorf("source credential at row %d: %w", i+1, err)
		}
		var credential struct {
			AccessToken  string `json:"access_token"`
			RefreshToken string `json:"refresh_token"`
			UID          string `json:"uid"`
		}
		if json.Unmarshal(plain, &credential) != nil || credential.AccessToken == "" || credential.RefreshToken == "" || (credential.UID != "" && credential.UID != a.UID) {
			return nil, fmt.Errorf("invalid WorkBuddy credential at row %d", i+1)
		}
		uids[a.UID] = true
		p.accounts = append(p.accounts, preparedAccount{Account: a, Plaintext: plain})
	}
	return p, nil
}

func (p *Prepared) Count() int { return len(p.accounts) }
