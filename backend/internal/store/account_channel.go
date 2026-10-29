package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

var ErrInvalidInput = errors.New("配置无效")

// PlatformIdentity keeps WorkBuddy regions separate when assigning accounts.
func PlatformIdentity(platform string, options json.RawMessage) string {
	platform = strings.TrimSpace(platform)
	if platform != "workbuddy" {
		return platform
	}
	var value struct {
		Realm string `json:"realm"`
	}
	_ = json.Unmarshal(options, &value)
	if value.Realm == "intl" {
		return "workbuddy-intl"
	}
	return "workbuddy-cn"
}

func AccountMatchesChannel(account Account, channel Channel) bool {
	platform := account.Platform
	if platform == "" {
		platform = account.Adapter
	}
	return PlatformIdentity(platform, account.Options) == PlatformIdentity(channel.Adapter, channel.Options)
}

func (s *Store) ValidateAccountChannel(ctx context.Context, channelID, platform string, options json.RawMessage) error {
	if strings.TrimSpace(channelID) == "" {
		return nil
	}
	channel, err := s.GetChannel(ctx, strings.TrimSpace(channelID))
	if err != nil {
		return err
	}
	if !channel.Enabled {
		return fmt.Errorf("%w：目标分组已停用", ErrInvalidInput)
	}
	if !AccountMatchesChannel(Account{Platform: platform, Options: options}, *channel) {
		return fmt.Errorf("%w：账号只能分配到相同平台及版本的分组", ErrInvalidInput)
	}
	return nil
}
