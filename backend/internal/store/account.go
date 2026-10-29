package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"relayai/backend/internal/models"
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
	// 分配分组时必须与分组绑定的平台及版本一致。
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
	Status        string  `json:"status"`
	CooldownUntil *string `json:"cooldownUntil"`
	Inflight      int     `json:"inflight"`
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
	// ModelLimits 是**模型级**限流明细。
	//
	// 与 BlockedReasons 分开：后者是"账号整体不可用"，
	// 这里是"号可用，只是某几个模型在等"。合成一个会让整号显示成冷却中。
	ModelLimits []ModelLimit `json:"modelLimits"`
}

// ModelLimit 是一条模型级限流。
type ModelLimit struct {
	Model   string `json:"model"`
	UntilMs int64  `json:"untilMs"`
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

	var total int64
	countQuery := "SELECT count(*) FROM accounts a " + where
	if err := s.queryOne(ctx, &total, countQuery, args...); err != nil {
		return nil, 0, fmt.Errorf("统计账号失败: %w", err)
	}

	listArgs := append(args, query.PageSize, (query.Page-1)*query.PageSize)
	listQuery := fmt.Sprintf(`
		SELECT a.id, COALESCE(a.channel_id, '') AS channel_id,
		       COALESCE(c.name, a.channel_id, '') AS channel_name,
		       COALESCE(c.adapter, '') AS adapter,
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]) AS model_allow,
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options,
		       a.proxy_id, p.name AS proxy_name
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		LEFT JOIN proxies p ON p.id = a.proxy_id
		%s
		ORDER BY a.channel_id, a.added_at
		LIMIT $%d OFFSET $%d`, where, len(listArgs)-1, len(listArgs))

	var rows []accountRow
	if err := s.queryAll(ctx, &rows, listQuery, listArgs...); err != nil {
		return nil, 0, fmt.Errorf("查询账号失败: %w", err)
	}

	list := make([]Account, 0, len(rows))
	for _, row := range rows {
		account := Account{
			ID:               row.ID,
			ChannelID:        row.ChannelID,
			ChannelName:      row.ChannelName,
			Adapter:          row.Adapter,
			UID:              row.UID,
			Nickname:         row.Nickname,
			Enabled:          row.Enabled,
			PriorityOverride: row.PriorityOverride,
			WeightOverride:   row.WeightOverride,
			MaxInflight:      row.MaxInflight,
			ModelAllow:       orEmptyText(row.ModelAllow),
			Source:           row.Source,
			AddedAt:          formatTimePtr(row.AddedAt),
			BaseURL:          row.BaseURL,
			Protocol:         row.Protocol,
			Platform:         row.Platform,
			ProxyID:          row.ProxyID,
			ProxyName:        row.ProxyName,
		}
		account.ModelRename = rawJSONOrEmpty(row.ModelRename)
		account.Options = rawJSONOrEmpty(row.Options)
		list = append(list, account)
	}

	return list, int(total), nil
}

// GetAccount 按 id 取账号。
func (s *Store) GetAccount(ctx context.Context, id int64) (*Account, error) {
	const query = `
		SELECT a.id, COALESCE(a.channel_id, '') AS channel_id,
		       COALESCE(c.name, a.channel_id, '') AS channel_name,
		       COALESCE(c.adapter, '') AS adapter,
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]) AS model_allow,
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		WHERE a.id = $1`

	var row accountRow
	if err := s.queryOne(ctx, &row, query, id); err != nil {
		if errors.Is(err, ErrNotFound) {
			return nil, ErrNotFound
		}
		return nil, fmt.Errorf("查询账号失败: %w", err)
	}

	account := Account{
		ID:               row.ID,
		ChannelID:        row.ChannelID,
		ChannelName:      row.ChannelName,
		Adapter:          row.Adapter,
		UID:              row.UID,
		Nickname:         row.Nickname,
		Enabled:          row.Enabled,
		PriorityOverride: row.PriorityOverride,
		WeightOverride:   row.WeightOverride,
		MaxInflight:      row.MaxInflight,
		ModelAllow:       orEmptyText(row.ModelAllow),
		Source:           row.Source,
		AddedAt:          formatTimePtr(row.AddedAt),
		BaseURL:          row.BaseURL,
		Protocol:         row.Protocol,
		Platform:         row.Platform,
	}
	account.ModelRename = rawJSONOrEmpty(row.ModelRename)
	account.Options = rawJSONOrEmpty(row.Options)

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
	Options          json.RawMessage `json:"options"`
	APIKey           *string         `json:"apiKey"`
	CredentialCipher []byte          `json:"-"`
	// Presence distinguishes omitted overrides from an explicit reset to NULL.
	MaxInflightSet      bool `json:"-"`
	PriorityOverrideSet bool `json:"-"`
	WeightOverrideSet   bool `json:"-"`
	// BaseURL / Protocol 是上游地址与协议（账号级）。
	BaseURL  *string `json:"baseUrl"`
	Protocol *string `json:"protocol"`
	/*
		ChannelID 允许把账号**迁移到另一个分组**。

		为什么需要：分组是计价桶，把号放错桶是很常见的操作失误。
		以前只能删号重建 —— 而重建会换掉 `id`、丢掉历史用量归属。

		约束：目标分组必须存在。迁移**不改 uid、不改凭据**，
		所以 `usage_logs.account_uid` 仍然对得上这个号。
	*/
	ChannelID *string `json:"channelId"`
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
	} else if input.PriorityOverrideSet {
		sets = append(sets, "priority_override = NULL")
	}
	if input.WeightOverride != nil {
		sets = append(sets, "weight_override = "+arg(*input.WeightOverride))
	} else if input.WeightOverrideSet {
		sets = append(sets, "weight_override = NULL")
	}
	if input.MaxInflight != nil {
		sets = append(sets, "max_inflight = "+arg(*input.MaxInflight))
	} else if input.MaxInflightSet {
		sets = append(sets, "max_inflight = NULL")
	}
	if input.ModelAllow != nil {
		sets = append(sets, "model_allow = "+arg(models.TextArray(*input.ModelAllow)))
	}
	if len(input.ModelRename) > 0 {
		sets = append(sets, "model_rename = "+arg(input.ModelRename))
	}
	if len(input.Options) > 0 {
		sets = append(sets, "options = "+arg(input.Options))
	}
	if input.CredentialCipher != nil {
		sets = append(sets, "credential_cipher = "+arg(input.CredentialCipher))
	}
	if input.BaseURL != nil {
		sets = append(sets, "base_url = "+arg(strings.TrimSpace(*input.BaseURL)))
	}
	if input.Protocol != nil {
		sets = append(sets, "protocol = "+arg(strings.TrimSpace(*input.Protocol)))
	}
	/*
		迁移到另一个分组。

		先校验目标分组存在且启用：直接 UPDATE 到不存在的 channel_id
		会撞外键（或者更糟，在没外键时留下一个悬空引用，
		路由层按 channel 找账号时永远找不到它）。
	*/
	if input.ChannelID != nil {
		target := strings.TrimSpace(*input.ChannelID)
		/*
			空串 = **解除分配**（回到"待分配"），不是错误。

			这条路径要能用：号停用了、或者要从某个计价桶里摘出来。
			以前只能删号重建 —— 那会丢掉历史用量归属。
		*/
		if target == "" {
			sets = append(sets, "channel_id = NULL")
		} else {
			account, err := s.GetAccount(ctx, id)
			if err != nil {
				return nil, err
			}
			platform := account.Platform
			if platform == "" {
				platform = account.Adapter
			}
			if err := s.ValidateAccountChannel(ctx, target, platform, account.Options); err != nil {
				return nil, err
			}
			sets = append(sets, "channel_id = "+arg(target))
		}
	}

	if len(sets) == 0 {
		return s.GetAccount(ctx, id)
	}

	query := "UPDATE accounts SET " + joinSets(sets) + " WHERE id = " + arg(id)
	affected, err := s.exec(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("更新账号失败: %w", err)
	}
	if affected == 0 {
		return nil, ErrNotFound
	}

	return s.GetAccount(ctx, id)
}

// DeleteAccount 删除账号。
func (s *Store) DeleteAccount(ctx context.Context, id int64) error {
	affected, err := s.exec(ctx, "DELETE FROM accounts WHERE id = $1", id)
	if err != nil {
		return fmt.Errorf("删除账号失败: %w", err)
	}
	if affected == 0 {
		return ErrNotFound
	}
	return nil
}

// CreateAccount 手工新增账号（自定义渠道填凭据）。
//
// `channelID` 传空串表示**先建号、暂不分配分组**。
func (s *Store) CreateAccount(ctx context.Context, channelID, uid, nickname, cipher string) (*Account, error) {
	/*
		冲突目标改成 `uid` 单列。

		为什么不是 (channel_id, uid)：channel_id 现在可空，
		而 Postgres 里 `NULL <> NULL`，复合唯一索引对空分组行不起作用 ——
		同一个 uid 可以插无数次。

		改为 uid 单列唯一也更符合语义：一个上游账号在库里只该有一条记录，
		换分组是 UPDATE 而不是新插一行。
	*/
	const query = `
		INSERT INTO accounts (channel_id, uid, nickname, enabled, credential_cipher, source)
		VALUES (NULLIF($1, ''), $2, $3, true, $4, 'manual')
		ON CONFLICT (uid) DO UPDATE SET
		    nickname = EXCLUDED.nickname,
		    credential_cipher = EXCLUDED.credential_cipher,
		    -- 传了分组才改分组：空表示"保持原样"，不把已有的归组清掉。
		    channel_id = COALESCE(EXCLUDED.channel_id, accounts.channel_id)
		RETURNING id`

	var id int64
	if err := s.queryOne(ctx, &id, query, channelID, uid, nickname, []byte(cipher)); err != nil {
		return nil, fmt.Errorf("新增账号失败: %w", err)
	}

	return s.GetAccount(ctx, id)
}

// FindAccountByUID 按渠道与 uid 找账号（登录完成 / 手填凭据建号后回查）。
func (s *Store) FindAccountByUID(ctx context.Context, channelID, uid string) (*Account, error) {
	/*
		uid 是唯一键（`accounts_uid_key`），channel_id 可空。

		按 uid 查而不是 (channel_id, uid)：channelID 传空串表示
		"刚建的号还没分配分组" —— 那行的 channel_id 是 NULL，
		`channel_id = ''` 永远匹配不到它，会误报"记录不存在"。
	*/
	const query = `
		SELECT a.id, COALESCE(a.channel_id, '') AS channel_id,
		       COALESCE(c.name, a.channel_id, '') AS channel_name, COALESCE(c.adapter, '') AS adapter,
		       a.uid, a.nickname, a.enabled,
		       a.priority_override, a.weight_override, a.max_inflight,
		       COALESCE(a.model_allow, ARRAY[]::text[]),
		       a.model_rename, a.source, a.added_at, a.base_url, a.protocol, a.platform, a.options
		FROM accounts a
		LEFT JOIN channels c ON c.id = a.channel_id
		WHERE a.uid = $1`

	var row accountRow
	if err := s.queryOne(ctx, &row, query, uid); err != nil {
		if errors.Is(err, ErrNotFound) {
			return nil, ErrNotFound
		}
		return nil, fmt.Errorf("查询账号失败: %w", err)
	}

	account := Account{
		ID:               row.ID,
		ChannelID:        row.ChannelID,
		ChannelName:      row.ChannelName,
		Adapter:          row.Adapter,
		UID:              row.UID,
		Nickname:         row.Nickname,
		Enabled:          row.Enabled,
		PriorityOverride: row.PriorityOverride,
		WeightOverride:   row.WeightOverride,
		MaxInflight:      row.MaxInflight,
		ModelAllow:       orEmptyText(row.ModelAllow),
		Source:           row.Source,
		AddedAt:          formatTimePtr(row.AddedAt),
		BaseURL:          row.BaseURL,
		Protocol:         row.Protocol,
		Platform:         row.Platform,
	}
	account.ModelRename = rawJSONOrEmpty(row.ModelRename)
	account.Options = rawJSONOrEmpty(row.Options)

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
