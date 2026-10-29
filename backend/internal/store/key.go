package store

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"relayai/backend/internal/models"
)

// APIKey 是密钥的管理面视图。**永不包含明文**（SPEC I2）。
type APIKey struct {
	ID              int64    `json:"id"`
	Name            string   `json:"name"`
	KeyPrefix       string   `json:"keyPrefix"`
	ChannelAllow    []string `json:"channelAllow"`
	ModelAllow      []string `json:"modelAllow"`
	RPMLimit        *int     `json:"rpmLimit"`
	DailyTokenLimit *int64   `json:"dailyTokenLimit"`
	MaxConcurrency  *int     `json:"maxConcurrency"`
	Enabled         bool     `json:"enabled"`
	ExpiresAt       *string  `json:"expiresAt"`
	LastUsedAt      *string  `json:"lastUsedAt"`
	CreatedAt       string   `json:"createdAt"`
	// Plaintext 是密钥明文，供界面反复查看与复制（对齐 sub2api）。
	//
	// 老数据可能为 NULL：它们的明文只在创建时出现过一次，已经找不回来了。
	// 界面据此显示"仅前缀可用，请轮换"，不假装能复制。
	Plaintext *string `json:"plaintext"`
}

// KeyInput 是新建/更新密钥的输入。
type KeyInput struct {
	// UserID 是密钥归属的用户；结算时按它扣余额。
	//
	// 为空表示"没有归属"，那种 Key 产生的用量扣不到任何人头上 ——
	// 结算时会跳过，表现为"消费了但余额没少"。
	UserID          int64    `json:"userId"`
	Name            string   `json:"name"`
	ChannelAllow    []string `json:"channelAllow"`
	ModelAllow      []string `json:"modelAllow"`
	RPMLimit        *int     `json:"rpmLimit"`
	DailyTokenLimit *int64   `json:"dailyTokenLimit"`
	MaxConcurrency  *int     `json:"maxConcurrency"`
	Enabled         *bool    `json:"enabled"`
	ExpiresAt       *string  `json:"expiresAt"`
	fields          map[string]json.RawMessage
}

// UnmarshalJSON distinguishes omitted PATCH fields from explicit null resets.
func (input *KeyInput) UnmarshalJSON(data []byte) error {
	type plain KeyInput
	var decoded plain
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	*input = KeyInput(decoded)
	input.fields = fields
	return nil
}

func (input KeyInput) hasField(name string, provided bool) bool {
	if input.fields != nil {
		_, exists := input.fields[name]
		return exists
	}
	return provided
}

// ListKeys 返回全部密钥。
func (s *Store) ListKeys(ctx context.Context) ([]APIKey, error) {
	const query = `
		SELECT id, name, key_prefix,
		       COALESCE(channel_allow, ARRAY[]::text[]) AS channel_allow,
		       COALESCE(model_allow, ARRAY[]::text[]) AS model_allow,
		       rpm_limit, daily_token_limit, max_concurrency, enabled,
		       expires_at, last_used_at, created_at, key_plaintext
		FROM api_keys ORDER BY created_at DESC`

	var rows []keyRow
	if err := s.queryAll(ctx, &rows, query); err != nil {
		return nil, fmt.Errorf("查询密钥失败: %w", err)
	}

	list := make([]APIKey, 0, len(rows))
	for _, row := range rows {
		list = append(list, APIKey{
			ID:              row.ID,
			Name:            row.Name,
			KeyPrefix:       row.KeyPrefix,
			ChannelAllow:    orEmptyText(row.ChannelAllow),
			ModelAllow:      orEmptyText(row.ModelAllow),
			RPMLimit:        row.RPMLimit,
			DailyTokenLimit: row.DailyTokenLimit,
			MaxConcurrency:  row.MaxConcurrency,
			Enabled:         row.Enabled,
			ExpiresAt:       nullableTimePtr(row.ExpiresAt),
			LastUsedAt:      nullableTimePtr(row.LastUsedAt),
			CreatedAt:       formatTimeValue(row.CreatedAt),
			Plaintext:       row.KeyPlaintext,
		})
	}

	return list, nil
}

// CreateKey 新建密钥，返回**一次性明文**。
//
// 明文只在这一次返回里出现：库里只存 SHA-256 与前缀（SPEC I2）。
func (s *Store) CreateKey(ctx context.Context, input KeyInput) (*APIKey, string, error) {
	plaintext, prefix, hash, err := generateKey()
	if err != nil {
		return nil, "", err
	}

	enabled := true
	if input.Enabled != nil {
		enabled = *input.Enabled
	}

	const query = `
		INSERT INTO api_keys (name, key_hash, key_prefix, key_plaintext, channel_allow, model_allow,
		                      rpm_limit, daily_token_limit, max_concurrency, enabled, expires_at, user_id)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
		RETURNING id, created_at`

	var created struct {
		ID        int64     `gorm:"column:id"`
		CreatedAt time.Time `gorm:"column:created_at"`
	}
	if err := s.queryOne(ctx, &created, query,
		input.Name, hash, prefix, plaintext,
		models.TextArray(emptyToNil(input.ChannelAllow)), models.TextArray(emptyToNil(input.ModelAllow)),
		input.RPMLimit, input.DailyTokenLimit, input.MaxConcurrency,
		enabled, input.ExpiresAt, nullableUserID(input.UserID),
	); err != nil {
		return nil, "", fmt.Errorf("新建密钥失败: %w", err)
	}
	id := created.ID
	createdAt := formatTimeValue(created.CreatedAt)

	key := &APIKey{
		ID:              id,
		Name:            input.Name,
		KeyPrefix:       prefix,
		ChannelAllow:    orEmpty(input.ChannelAllow),
		ModelAllow:      orEmpty(input.ModelAllow),
		RPMLimit:        input.RPMLimit,
		DailyTokenLimit: input.DailyTokenLimit,
		MaxConcurrency:  input.MaxConcurrency,
		Enabled:         enabled,
		ExpiresAt:       input.ExpiresAt,
		CreatedAt:       formatTime(createdAt),
		Plaintext:       &plaintext,
	}

	return key, plaintext, nil
}

// UpdateKey 更新密钥。
func (s *Store) UpdateKey(ctx context.Context, id int64, input KeyInput) (*APIKey, error) {
	sets := make([]string, 0, 8)
	args := []any{id}
	add := func(column string, value any) {
		args = append(args, value)
		sets = append(sets, fmt.Sprintf("%s = $%d", column, len(args)))
	}
	if input.hasField("name", input.Name != "") && strings.TrimSpace(input.Name) != "" {
		add("name", strings.TrimSpace(input.Name))
	}
	if input.hasField("channelAllow", input.ChannelAllow != nil) {
		add("channel_allow", models.TextArray(emptyToNil(input.ChannelAllow)))
	}
	if input.hasField("modelAllow", input.ModelAllow != nil) {
		add("model_allow", models.TextArray(emptyToNil(input.ModelAllow)))
	}
	if input.hasField("rpmLimit", input.RPMLimit != nil) {
		add("rpm_limit", input.RPMLimit)
	}
	if input.hasField("dailyTokenLimit", input.DailyTokenLimit != nil) {
		add("daily_token_limit", input.DailyTokenLimit)
	}
	if input.hasField("maxConcurrency", input.MaxConcurrency != nil) {
		add("max_concurrency", input.MaxConcurrency)
	}
	if input.hasField("enabled", input.Enabled != nil) && input.Enabled != nil {
		add("enabled", input.Enabled)
	}
	if input.hasField("expiresAt", input.ExpiresAt != nil) {
		add("expires_at", input.ExpiresAt)
	}
	if len(sets) > 0 {
		affected, err := s.exec(ctx, "UPDATE api_keys SET "+strings.Join(sets, ", ")+" WHERE id = $1", args...)
		if err != nil {
			return nil, fmt.Errorf("更新密钥失败: %w", err)
		}
		if affected == 0 {
			return nil, ErrNotFound
		}
	}

	list, err := s.ListKeys(ctx)
	if err != nil {
		return nil, err
	}
	for _, key := range list {
		if key.ID == id {
			return &key, nil
		}
	}

	return nil, ErrNotFound
}

// DeleteKey 删除密钥。
func (s *Store) DeleteKey(ctx context.Context, id int64) error {
	affected, err := s.exec(ctx, "DELETE FROM api_keys WHERE id = $1", id)
	if err != nil {
		return fmt.Errorf("删除密钥失败: %w", err)
	}
	if affected == 0 {
		return ErrNotFound
	}
	return nil
}

// RotateKey 轮换密钥：旧 hash 立即失效，返回新明文。
func (s *Store) RotateKey(ctx context.Context, id int64) (string, error) {
	plaintext, prefix, hash, err := generateKey()
	if err != nil {
		return "", err
	}

	affected, err := s.exec(ctx,
		"UPDATE api_keys SET key_hash = $2, key_prefix = $3 WHERE id = $1",
		id, hash, prefix,
	)
	if err != nil {
		return "", fmt.Errorf("轮换密钥失败: %w", err)
	}
	if affected == 0 {
		return "", ErrNotFound
	}

	return plaintext, nil
}

// ResolveKey 按明文查 Key 的权限上下文（网关鉴权用）。
//
// 注意：网关是 Rust 进程，实际鉴权在那边做；这里提供给管理面的
// 「这个 Key 能用什么」预览，以及未来 Go 侧做鉴权兜底。
func (s *Store) ResolveKey(ctx context.Context, plaintext string) (*APIKey, error) {
	hash := hashKey(plaintext)

	const query = `
		SELECT id, name, key_prefix,
		       COALESCE(channel_allow, ARRAY[]::text[]) AS channel_allow,
		       COALESCE(model_allow, ARRAY[]::text[]) AS model_allow,
		       rpm_limit, daily_token_limit, max_concurrency, enabled,
		       expires_at, last_used_at, created_at
		FROM api_keys WHERE key_hash = $1`

	var row keyRow
	if err := s.queryOne(ctx, &row, query, hash); err != nil {
		return nil, ErrNotFound
	}

	return &APIKey{
		ID:              row.ID,
		Name:            row.Name,
		KeyPrefix:       row.KeyPrefix,
		ChannelAllow:    orEmptyText(row.ChannelAllow),
		ModelAllow:      orEmptyText(row.ModelAllow),
		RPMLimit:        row.RPMLimit,
		DailyTokenLimit: row.DailyTokenLimit,
		MaxConcurrency:  row.MaxConcurrency,
		Enabled:         row.Enabled,
		ExpiresAt:       nullableTimePtr(row.ExpiresAt),
		LastUsedAt:      nullableTimePtr(row.LastUsedAt),
		CreatedAt:       formatTimeValue(row.CreatedAt),
	}, nil
}

// generateKey 生成 `sk-` 开头的密钥，返回明文、前缀与哈希。
func generateKey() (plaintext, prefix, hash string, err error) {
	buffer := make([]byte, 32)
	if _, err = rand.Read(buffer); err != nil {
		return "", "", "", fmt.Errorf("生成密钥失败: %w", err)
	}

	plaintext = "sk-" + hex.EncodeToString(buffer)
	prefix = plaintext[:9] + "****"
	hash = hashKey(plaintext)

	return plaintext, prefix, hash, nil
}

func hashKey(plaintext string) string {
	sum := sha256.Sum256([]byte(plaintext))
	return hex.EncodeToString(sum[:])
}

// nilIfEmpty 把空串存成 NULL。
//
// 库里"没配"只有一种表示（NULL），查询与判断才不用同时处理 '' 和 NULL。
//
// `user_id` 是外键，0 不是合法用户 —— 直接写 0 会违反外键约束。
// 用 nil 表示"没有归属"，语义也更准确。
func nullableUserID(id int64) any {
	if id <= 0 {
		return nil
	}
	return id
}

func orEmpty(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}

func nullableTime(value any) *string {
	formatted := formatTime(value)
	if formatted == "" || strings.HasPrefix(formatted, "0001-") {
		return nil
	}
	return &formatted
}

// ParseExpiresAt 校验并解析前端传来的时间。
func ParseExpiresAt(value *string) (*time.Time, error) {
	if value == nil || *value == "" {
		return nil, nil
	}
	parsed, err := time.Parse(time.RFC3339, *value)
	if err != nil {
		return nil, fmt.Errorf("时间格式应为 RFC3339: %w", err)
	}
	return &parsed, nil
}
