package store

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"
)

// Account 是账号的管理面视图。
type Account struct {
	ID               int64           `json:"id"`
	ChannelID        string          `json:"channelId"`
	ChannelName      string          `json:"channelName"`
	Adapter          string          `json:"adapter"`
	UID              string          `json:"uid"`
	Nickname         string          `json:"nickname"`
	Enabled          bool            `json:"enabled"`
	PriorityOverride *int            `json:"priorityOverride"`
	WeightOverride   *int            `json:"weightOverride"`
	MaxInflight      *int            `json:"maxInflight"`
	ModelAllow       []string        `json:"modelAllow"`
	ModelRename      json.RawMessage `json:"modelRename"`
	Source           string          `json:"source"`
	AddedAt          string          `json:"addedAt"`
	// BaseURL 是账号级上游地址（空 = 用适配器内置默认）。
	//
	// 为什么在账号上：渠道是计价层，同一份计价策略下可能挂多个不同上游的 Key。
	// 地址属于凭据，不属于计价 —— 放在渠道上会出现"建了渠道却不知道它服务谁"。
	BaseURL string `json:"baseUrl"`
	// Protocol 是账号级上游协议：openai-chat / openai-responses / anthropic。
	Protocol string `json:"protocol"`
	// Platform 是该账号的上游平台：workbuddy / trae / custom / fake。
	//
	// **平台在账号上，不在渠道上。** 渠道是计价分组，一个分组下面可以混
	// 不同平台的号；"我要加一个 WorkBuddy 号"不需要先建 WorkBuddy 渠道。
	Platform string `json:"platform"`
	// Options 是适配器私有参数（如 workbuddy 的 realm / outbound_identity）。
	Options json.RawMessage `json:"options"`
	// Runtime 是运行态，来自网关；网关不可用时为 nil（SPEC I5：运行态不落库）。
	Runtime *AccountRuntime `json:"runtime"`
	// Usage 是用量汇总（今日 / 累计），来自 PG 的 usage_logs 聚合。
	Usage *AccountUsage `json:"usage"`
	// ProxyID / ProxyName 是出站代理（nil = 直连）。
	ProxyID   *int64  `json:"proxyId"`
	ProxyName *string `json:"proxyName"`
}

// AccountRuntime 是账号的运行态快照。
type AccountRuntime struct {
	Status              string   `json:"status"`
	CooldownUntil       *string  `json:"cooldownUntil"`
	Inflight            int      `json:"inflight"`
	// MaxInflight 是网关的**生效并发上限**。
	//
	// 与 Account.MaxInflight 的区别必须分清：那个是库里的原始值，可为 NULL
	// （"用渠道默认"）；这个一定是正数。界面显示 `inflight/maxInflight` 时
	// 只能用后者 —— 用 NULL 会被读成"无限"，而真实上限是 3。
	MaxInflight         int      `json:"maxInflight"`
	BlockedReasons      []string `json:"blockedReasons"`
	LastError           *string  `json:"lastError"`
	LastOkAt            *string  `json:"lastOkAt"`
	CredentialExpiresAt *string  `json:"credentialExpiresAt"`
	// StaticCredential 表示该账号用静态凭据（自定义供应商的 API Key）。
	// 静态凭据没有"刷新"这回事，界面据此禁用刷新按钮而不是点了才报错。
	StaticCredential bool `json:"staticCredential"`
	// Quota 是额度快照（来自网关内存，不打上游）。
	Quota *AccountQuota `json:"quota"`
}

// AccountQuota 是账号的额度快照。
type AccountQuota struct {
	Label     string   `json:"label"`
	Unit      string   `json:"unit"`
	Remaining *float64 `json:"remaining"`
	Total     *float64 `json:"total"`
	Used      *float64 `json:"used"`
	FetchedAt string   `json:"fetchedAt"`
}

// AccountQuery 是账号列表的筛选条件。
type AccountQuery struct {
	ChannelID string
	Keyword   string
	Page      int
	PageSize  int
}

// ListAccounts 分页查询账号。
func (s *Store) ListAccounts(ctx context.Context, query AccountQuery) ([]Account, int, error) {
	if query.Page < 1 {
		query.Page = 1
	}
	if query.PageSize < 1 || query.PageSize > 200 {
		query.PageSize = 50
	}

	where := "WHERE 1=1"
	args := make([]any, 0, 4)
	if query.ChannelID != "" {
		args = append(args, query.ChannelID)
		where += fmt.Sprintf(" AND a.channel_id = $%d", len(args))
	}
	if query.Keyword != "" {
		args = append(args, "%"+query.Keyword+"%")
		where += fmt.Sprintf(" AND (a.uid ILIKE $%d OR a.nickname ILIKE $%d)", len(args), len(args))
	}

	var total int
	countQuery := "SELECT count(*) FROM accounts a " + where
	if err := s.pool.QueryRow(ctx, countQuery, args...).Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计账号失败: %w", err)
	}

	listArgs := append(args, query.PageSize, (query.Page-1)*query.PageSize)
	listQuery := fmt.Sprintf(`
		SELECT a.id, a.channel_id, COALESCE(c.name, a.channel_id), COALESCE(c.adapter, ''),
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]),
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options,
		       a.proxy_id, p.name
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		LEFT JOIN proxies p ON p.id = a.proxy_id
		%s
		ORDER BY a.channel_id, a.added_at
		LIMIT $%d OFFSET $%d`, where, len(listArgs)-1, len(listArgs))

	rows, err := s.pool.Query(ctx, listQuery, listArgs...)
	if err != nil {
		return nil, 0, fmt.Errorf("查询账号失败: %w", err)
	}
	defer rows.Close()

	list := make([]Account, 0, query.PageSize)
	for rows.Next() {
		var account Account
		var addedAt interface{}
		if err := rows.Scan(
			&account.ID, &account.ChannelID, &account.ChannelName, &account.Adapter,
			&account.UID, &account.Nickname, &account.Enabled,
			&account.PriorityOverride, &account.WeightOverride, &account.MaxInflight,
			&account.ModelAllow, &account.ModelRename, &account.Source, &addedAt,
			&account.BaseURL, &account.Protocol, &account.Platform, &account.Options,
			&account.ProxyID, &account.ProxyName,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描账号失败: %w", err)
		}
		account.AddedAt = formatTime(addedAt)
		list = append(list, account)
	}

	return list, total, rows.Err()
}

// GetAccount 按 id 取账号。
func (s *Store) GetAccount(ctx context.Context, id int64) (*Account, error) {
	const query = `
		SELECT a.id, a.channel_id, COALESCE(c.name, a.channel_id), COALESCE(c.adapter, ''),
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]),
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		WHERE a.id = $1`

	var account Account
	var addedAt interface{}
	err := s.pool.QueryRow(ctx, query, id).Scan(
		&account.ID, &account.ChannelID, &account.ChannelName, &account.Adapter,
		&account.UID, &account.Nickname, &account.Enabled,
		&account.PriorityOverride, &account.WeightOverride, &account.MaxInflight,
		&account.ModelAllow, &account.ModelRename, &account.Source, &addedAt,
		&account.BaseURL, &account.Protocol, &account.Platform, &account.Options,
	)
	if err == pgx.ErrNoRows {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("查询账号失败: %w", err)
	}
	account.AddedAt = formatTime(addedAt)

	return &account, nil
}

// AccountInput 是更新账号的输入。
type AccountInput struct {
	Nickname         *string         `json:"nickname"`
	Enabled          *bool           `json:"enabled"`
	PriorityOverride *int            `json:"priorityOverride"`
	WeightOverride   *int            `json:"weightOverride"`
	MaxInflight      *int            `json:"maxInflight"`
	ModelAllow       *[]string       `json:"modelAllow"`
	ModelRename      json.RawMessage `json:"modelRename"`
	// BaseURL / Protocol 是上游地址与协议（账号级）。
	BaseURL  *string `json:"baseUrl"`
	Protocol *string `json:"protocol"`
}

// UpdateAccount 更新账号。
func (s *Store) UpdateAccount(ctx context.Context, id int64, input AccountInput) (*Account, error) {
	sets := make([]string, 0, 6)
	args := make([]any, 0, 8)
	arg := func(value any) string {
		args = append(args, value)
		return fmt.Sprintf("$%d", len(args))
	}

	if input.Nickname != nil {
		sets = append(sets, "nickname = "+arg(*input.Nickname))
	}
	if input.Enabled != nil {
		sets = append(sets, "enabled = "+arg(*input.Enabled))
	}
	if input.PriorityOverride != nil {
		sets = append(sets, "priority_override = "+arg(*input.PriorityOverride))
	}
	if input.WeightOverride != nil {
		sets = append(sets, "weight_override = "+arg(*input.WeightOverride))
	}
	if input.MaxInflight != nil {
		sets = append(sets, "max_inflight = "+arg(*input.MaxInflight))
	}
	if input.ModelAllow != nil {
		sets = append(sets, "model_allow = "+arg(*input.ModelAllow))
	}
	if len(input.ModelRename) > 0 {
		sets = append(sets, "model_rename = "+arg(input.ModelRename))
	}
	if input.BaseURL != nil {
		sets = append(sets, "base_url = "+arg(strings.TrimSpace(*input.BaseURL)))
	}
	if input.Protocol != nil {
		sets = append(sets, "protocol = "+arg(strings.TrimSpace(*input.Protocol)))
	}

	if len(sets) == 0 {
		return s.GetAccount(ctx, id)
	}

	query := "UPDATE accounts SET " + joinSets(sets) + " WHERE id = " + arg(id)
	result, err := s.pool.Exec(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("更新账号失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return nil, ErrNotFound
	}

	return s.GetAccount(ctx, id)
}

// DeleteAccount 删除账号。
func (s *Store) DeleteAccount(ctx context.Context, id int64) error {
	result, err := s.pool.Exec(ctx, "DELETE FROM accounts WHERE id = $1", id)
	if err != nil {
		return fmt.Errorf("删除账号失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// CreateAccount 手工新增账号（自定义渠道填凭据）。
func (s *Store) CreateAccount(ctx context.Context, channelID, uid, nickname, cipher string) (*Account, error) {
	const query = `
		INSERT INTO accounts (channel_id, uid, nickname, enabled, credential_cipher, source)
		VALUES ($1, $2, $3, true, $4, 'manual')
		ON CONFLICT (channel_id, uid) DO UPDATE SET
		    nickname = EXCLUDED.nickname,
		    credential_cipher = EXCLUDED.credential_cipher
		RETURNING id`

	var id int64
	if err := s.pool.QueryRow(ctx, query, channelID, uid, nickname, []byte(cipher)).Scan(&id); err != nil {
		return nil, fmt.Errorf("新增账号失败: %w", err)
	}

	return s.GetAccount(ctx, id)
}

// FindAccountByUID 按渠道与 uid 找账号（登录完成 / 手填凭据建号后回查）。
func (s *Store) FindAccountByUID(ctx context.Context, channelID, uid string) (*Account, error) {
	const query = `
		SELECT a.id, a.channel_id, COALESCE(c.name, a.channel_id), COALESCE(c.adapter, ''),
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]),
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		WHERE a.channel_id = $1 AND a.uid = $2`

	var account Account
	var addedAt any
	err := s.pool.QueryRow(ctx, query, channelID, uid).Scan(
		&account.ID, &account.ChannelID, &account.ChannelName, &account.Adapter,
		&account.UID, &account.Nickname, &account.Enabled,
		&account.PriorityOverride, &account.WeightOverride, &account.MaxInflight,
		&account.ModelAllow, &account.ModelRename, &account.Source, &addedAt,
		&account.BaseURL, &account.Protocol, &account.Platform, &account.Options,
	)
	if err == pgx.ErrNoRows {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("查询账号失败: %w", err)
	}
	account.AddedAt = formatTime(addedAt)

	return &account, nil
}

// formatTime 把 pgx 返回的时间转成 RFC3339。
func formatTime(value any) string {
	switch typed := value.(type) {
	case interface{ Format(string) string }:
		return typed.Format("2006-01-02T15:04:05Z07:00")
	default:
		return ""
	}
}
