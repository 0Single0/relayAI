package models

import "time"

// AccountGroups holds routing policy, never credentials or account runtime.
type AccountGroups struct {
	AccountID int64     `gorm:"column:account_id;primaryKey;autoIncrement:false"`
	GroupID   string    `gorm:"column:group_id;primaryKey;type:text;index:idx_account_groups_group"`
	Enabled   bool      `gorm:"column:enabled;not null;default:true"`
	Priority  *int      `gorm:"column:priority;type:integer"`
	Weight    *int      `gorm:"column:weight;type:integer"`
	CreatedAt time.Time `gorm:"column:created_at;not null;default:now()"`
}

func (AccountGroups) TableName() string { return "account_groups" }
