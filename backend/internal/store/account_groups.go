package store

import (
	"context"
	"fmt"
	"strings"

	"relayai/backend/internal/models"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

type AccountGroup struct {
	GroupID       string `json:"groupId"`
	Name          string `json:"name"`
	ModelProvider string `json:"modelProvider"`
	Enabled       bool   `json:"enabled"`
	Priority      *int   `json:"priority"`
	Weight        *int   `json:"weight"`
}

type AccountGroupInput struct {
	GroupID  string `json:"groupId"`
	Enabled  bool   `json:"enabled"`
	Priority *int   `json:"priority"`
	Weight   *int   `json:"weight"`
}

func (s *Store) attachAccountGroups(ctx context.Context, accounts []Account) error {
	if len(accounts) == 0 {
		return nil
	}
	ids := make([]int64, 0, len(accounts))
	for _, account := range accounts {
		ids = append(ids, account.ID)
	}
	var rows []struct {
		AccountID     int64
		GroupID       string
		Name          string
		ModelProvider string
		Enabled       bool
		Priority      *int
		Weight        *int
	}
	if err := s.db.WithContext(ctx).Raw(`SELECT ag.account_id, ag.group_id, c.name, c.model_provider,
ag.enabled, ag.priority, ag.weight FROM account_groups ag JOIN channels c ON c.id = ag.group_id
WHERE ag.account_id IN ? ORDER BY ag.group_id`, ids).Scan(&rows).Error; err != nil {
		return err
	}
	byID := make(map[int64][]AccountGroup)
	for _, row := range rows {
		byID[row.AccountID] = append(byID[row.AccountID], AccountGroup{row.GroupID, row.Name, row.ModelProvider, row.Enabled, row.Priority, row.Weight})
	}
	for i := range accounts {
		account := &accounts[i]
		account.Groups = byID[account.ID]
		if account.Groups == nil {
			account.Groups = []AccountGroup{}
		}
		account.ChannelIDs = make([]string, 0, len(account.Groups))
		account.ChannelID, account.ChannelName = "", ""
		primaryEnabled := false
		for _, group := range account.Groups {
			account.ChannelIDs = append(account.ChannelIDs, group.GroupID)
			if account.ChannelID == "" || (group.Enabled && !primaryEnabled) {
				account.ChannelID, account.ChannelName = group.GroupID, group.Name
				primaryEnabled = group.Enabled
			}
		}
		account.Adapter = account.Platform
	}
	return nil
}

func normalizeAccountGroups(groups []AccountGroupInput) ([]AccountGroupInput, error) {
	seen := map[string]bool{}
	for i := range groups {
		group := &groups[i]
		group.GroupID = strings.TrimSpace(group.GroupID)
		if group.GroupID == "" || seen[group.GroupID] {
			return nil, fmt.Errorf("%w：分组不能为空或重复", ErrInvalidInput)
		}
		seen[group.GroupID] = true
		if group.Priority != nil && (*group.Priority < 0 || int64(*group.Priority) > 2147483647) {
			return nil, fmt.Errorf("%w：组内优先级必须为非负整数", ErrInvalidInput)
		}
		if group.Weight != nil && (*group.Weight < 1 || int64(*group.Weight) > 2147483647) {
			return nil, fmt.Errorf("%w：组内权重必须为正整数", ErrInvalidInput)
		}
	}
	return groups, nil
}

// One transaction saves the account and membership policy together. Locking the
// account serializes edits without resetting credentials or runtime identity.
func (s *Store) UpdateAccount(ctx context.Context, id int64, input AccountInput) (*Account, error) {
	err := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record models.Accounts
		if err := tx.Clauses(clause.Locking{Strength: "UPDATE"}).First(&record, id).Error; err != nil {
			if err == gorm.ErrRecordNotFound {
				return ErrNotFound
			}
			return err
		}
		groups := input.Groups
		if input.ChannelID != nil {
			if groups != nil {
				return fmt.Errorf("%w：不能同时提交 channelId 和 groups", ErrInvalidInput)
			}
			values := []AccountGroupInput{}
			if value := strings.TrimSpace(*input.ChannelID); value != "" {
				values = append(values, AccountGroupInput{GroupID: value, Enabled: true})
			}
			groups = &values
		}
		input.ChannelID, input.Groups = nil, nil
		transactionStore := New(tx)
		if _, err := transactionStore.updateAccountFields(ctx, id, input); err != nil {
			return err
		}
		if groups == nil {
			return nil
		}
		values, err := normalizeAccountGroups(*groups)
		if err != nil {
			return err
		}
		var existing []models.AccountGroups
		if err := tx.Where("account_id = ?", id).Find(&existing).Error; err != nil {
			return err
		}
		previous := map[string]bool{}
		for _, group := range existing {
			previous[group.GroupID] = true
		}
		for _, group := range values {
			var target models.Channels
			if err := tx.Clauses(clause.Locking{Strength: "SHARE"}).First(&target, "id = ?", group.GroupID).Error; err != nil {
				if err == gorm.ErrRecordNotFound {
					return fmt.Errorf("%w：目标分组不存在", ErrInvalidInput)
				}
				return err
			}
			if !target.Enabled && !previous[group.GroupID] {
				return fmt.Errorf("%w：目标分组已停用", ErrInvalidInput)
			}
		}
		if err := tx.Where("account_id = ?", id).Delete(&models.AccountGroups{}).Error; err != nil {
			return err
		}
		for _, group := range values {
			row := models.AccountGroups{AccountID: id, GroupID: group.GroupID, Enabled: group.Enabled, Priority: group.Priority, Weight: group.Weight}
			// Select all so false is not replaced by GORM's default true.
			if err := tx.Select("AccountID", "GroupID", "Enabled", "Priority", "Weight").Create(&row).Error; err != nil {
				return err
			}
		}
		var primary any
		if len(values) > 0 {
			primary = values[0].GroupID
		}
		return tx.Model(&models.Accounts{}).Where("id = ?", id).Update("channel_id", primary).Error
	})
	if err != nil {
		return nil, err
	}
	return s.GetAccount(ctx, id)
}
