package legacyimport

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

type Report struct {
	Mode                    string `json:"mode"`
	SourceAccounts          int    `json:"sourceAccounts"`
	NewAccounts             int    `json:"newAccounts"`
	SkippedExistingAccounts int    `json:"skippedExistingAccounts"`
	NewChannels             int    `json:"newChannels"`
}

// Avoid printing SQL errors containing data, connection strings, or credentials.
func databaseError(err error) error {
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) {
		return fmt.Errorf("database operation failed (SQLSTATE %s); transaction rolled back", pgErr.Code)
	}
	return errors.New("database operation failed; check connectivity and current relayAI schema")
}

func Import(ctx context.Context, conn *pgx.Conn, p *Prepared, targetKey []byte, apply bool) (Report, error) {
	r := Report{Mode: "preview", SourceAccounts: p.Count()}
	opts := pgx.TxOptions{IsoLevel: pgx.Serializable, AccessMode: pgx.ReadOnly}
	if apply {
		r.Mode = "apply"
		opts.AccessMode = pgx.ReadWrite
	}
	tx, err := conn.BeginTx(ctx, opts)
	if err != nil {
		return r, databaseError(err)
	}
	defer tx.Rollback(context.Background())
	if apply {
		if _, err = tx.Exec(ctx, "SELECT pg_advisory_xact_lock(734290103)"); err != nil {
			return r, databaseError(err)
		}
	}
	// Verify the deployment key against existing ciphertext before introducing new rows.
	rows, err := tx.Query(ctx, "SELECT credential_cipher FROM accounts WHERE credential_cipher IS NOT NULL AND octet_length(credential_cipher) > 0")
	if err != nil {
		return r, databaseError(err)
	}
	for rows.Next() {
		var sealed []byte
		if err = rows.Scan(&sealed); err != nil {
			rows.Close()
			return r, databaseError(err)
		}
		if _, err = open(targetKey, string(sealed)); err != nil {
			rows.Close()
			return r, errors.New("target key cannot decrypt an existing account; no accounts imported")
		}
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return r, databaseError(err)
	}
	byID := make(map[string]Channel)
	for _, c := range p.Channels {
		byID[c.ID] = c
		var adapter string
		var options json.RawMessage
		err = tx.QueryRow(ctx, "SELECT adapter, options FROM channels WHERE id=$1", c.ID).Scan(&adapter, &options)
		if errors.Is(err, pgx.ErrNoRows) {
			r.NewChannels++
			if apply {
				_, err = tx.Exec(ctx, `INSERT INTO channels (id,name,adapter,enabled,priority,weight,options)
					VALUES ($1,$2,$3,$4,$5,$6,$7::jsonb)`, c.ID, c.Name, c.Adapter, c.Enabled, c.Priority, c.Weight, string(c.Options))
				if err != nil {
					return r, databaseError(err)
				}
			}
		} else if err != nil {
			return r, databaseError(err)
		} else {
			sourceIdentity, _ := identity(c.Options)
			targetIdentity, identityErr := identity(options)
			if adapter != c.Adapter || identityErr != nil || sourceIdentity != targetIdentity {
				return r, errors.New("existing channel has a different platform/region/identity; resolve the channel conflict first")
			}
		}
	}
	for _, a := range p.accounts {
		var exists bool
		if err = tx.QueryRow(ctx, "SELECT EXISTS(SELECT 1 FROM accounts WHERE uid=$1)", a.UID).Scan(&exists); err != nil {
			return r, databaseError(err)
		}
		if exists {
			r.SkippedExistingAccounts++
			continue
		}
		if !apply {
			r.NewAccounts++
			continue
		}
		sealed, sealErr := seal(targetKey, a.Plaintext)
		if sealErr != nil {
			return r, sealErr
		}
		c := byID[a.ChannelID]
		result, insertErr := tx.Exec(ctx, `INSERT INTO accounts
			(channel_id,uid,nickname,enabled,credential_cipher,priority_override,weight_override,max_inflight,
			 model_rename,source,added_at,platform,options,base_url,protocol)
			VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'{}'::jsonb,$9,$10,'workbuddy',$11::jsonb,'','openai-chat')
			ON CONFLICT (uid) DO NOTHING`, a.ChannelID, a.UID, a.Nickname, a.Enabled != 0, sealed,
			a.Priority, a.Weight, a.MaxInflight, a.Source, time.UnixMilli(a.AddedAt).UTC(), string(c.Options))
		if insertErr != nil {
			return r, databaseError(insertErr)
		}
		if result.RowsAffected() == 1 {
			r.NewAccounts++
		} else {
			r.SkippedExistingAccounts++
		}
	}
	if apply {
		if err = tx.Commit(ctx); err != nil {
			return r, databaseError(err)
		}
	}
	return r, nil
}
