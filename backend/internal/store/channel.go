// Package store 是管理面的数据访问层。
//
// 分工（SPEC 4.3）：
//   · 配置类（渠道、账号、模型、Key、设置）在这里直连 PG；
//   · 必须贴适配器的操作（登录、刷新、探活、额度、任务）走 gateway 客户端。
package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// ErrNotFound 表示记录不存在。
var ErrNotFound = errors.New("记录不存在")

// ErrNotRunning 表示记录存在，但当前状态不允许这个操作。
//
// 与 ErrNotFound 分开：把"已经跑完了所以不能取消"报成"记录不存在"，
// 会让用户去翻 id 是不是写错了，而真正原因在状态。
var ErrNotRunning = errors.New("记录已不在执行中")

// Store 持有连接池。
type Store struct {
	pool *pgxpool.Pool
}

// New 创建仓储。
func New(pool *pgxpool.Pool) *Store {
	return &Store{pool: pool}
}

// Pool 暴露连接池（统计类查询直接写 SQL 时用）。
func (s *Store) Pool() *pgxpool.Pool {
	return s.pool
}

// Channel 是渠道的完整视图。
type Channel struct {
	ID               string          `json:"id"`
	Name             string          `json:"name"`
	Adapter          string          `json:"adapter"`
	Enabled          bool            `json:"enabled"`
	Priority         int             `json:"priority"`
	Weight           int             `json:"weight"`
	Options          json.RawMessage `json:"options"`
	ChannelRatio     float64         `json:"channelRatio"`
	ConcurrencyLimit *int            `json:"concurrencyLimit"`
	RateLimitQPS     *int            `json:"rateLimitQps"`
	AccountCount     int             `json:"accountCount"`
	TodayRequests    int64           `json:"todayRequests"`
	Caps             ChannelCaps     `json:"caps"`
	// Platforms 是该渠道名下账号出现过的平台（去重、排序）。
	//
	// 渠道是计价分组，一个分组下可以混不同平台的号。界面显示"有哪些平台"
	// 比显示"渠道类型"有意义 —— 后者在平台下沉到账号之后已经不存在了。
	Platforms []string `json:"platforms"`
	// ModelAllow 是该分组**对外暴露**的模型白名单（空 = 不限制）。
	//
	// 与账号级 `model_allow` 的区别：那个是"这个号**能跑**什么"（能力），
	// 这个是"这个分组**愿意卖**什么"（策略）。
	ModelAllow []string `json:"modelAllow"`
	// ProbeModel 是探活使用的对外模型名；空 = 由网关按「免费优先」自动挑。
	//
	// 允许指定是因为"免费模型通了"不等于"真实模型能通"：
	// 号被单独限权、或分组只挂了推理模型时，探针要给出一致的结论。
	ProbeModel string `json:"probeModel"`
}

// ChannelCaps 是缓存的运营能力。
type ChannelCaps struct {
	Tasks   bool `json:"tasks"`
	Daily   bool `json:"daily"`
	Checkin bool `json:"checkin"`
}

// ListChannels 返回全部渠道，含账号数与今日调用量。
func (s *Store) ListChannels(ctx context.Context) ([]Channel, error) {
	const query = `
		SELECT c.id, c.name, c.adapter, c.enabled, c.priority, c.weight, c.options,
		       c.channel_ratio::float8, c.concurrency_limit, c.rate_limit_qps,
		       COALESCE(a.cnt, 0) AS account_count,
		       COALESCE(u.cnt, 0) AS today_requests,
		       COALESCE(cap.tasks, false), COALESCE(cap.daily, false), COALESCE(cap.checkin, false),
		       COALESCE(p.platforms, ARRAY[]::text[]) AS platforms,
		       COALESCE(c.model_allow, ARRAY[]::text[]) AS model_allow,
		       COALESCE(c.probe_model, '') AS probe_model
		FROM channels c
		LEFT JOIN (
			SELECT channel_id, count(*) AS cnt FROM accounts GROUP BY channel_id
		) a ON a.channel_id = c.id
		LEFT JOIN (
			SELECT channel_id, count(*) AS cnt FROM usage_logs
			WHERE ts >= date_trunc('day', now()) AND NOT is_rejection
			GROUP BY channel_id
		) u ON u.channel_id = c.id
		LEFT JOIN channel_caps cap ON cap.channel_id = c.id
		-- 该渠道下出现过的平台。用 DISTINCT 而不是取第一个：
		-- 一个计价分组本来就可能混多个平台，界面要如实显示。
		LEFT JOIN (
			SELECT channel_id, array_agg(DISTINCT platform ORDER BY platform) AS platforms
			FROM accounts WHERE platform <> '' GROUP BY channel_id
		) p ON p.channel_id = c.id
		ORDER BY c.priority DESC, c.id`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询渠道失败: %w", err)
	}
	defer rows.Close()

	list := make([]Channel, 0, 8)
	for rows.Next() {
		var channel Channel
		if err := rows.Scan(
			&channel.ID, &channel.Name, &channel.Adapter, &channel.Enabled,
			&channel.Priority, &channel.Weight, &channel.Options,
			&channel.ChannelRatio, &channel.ConcurrencyLimit, &channel.RateLimitQPS,
			&channel.AccountCount, &channel.TodayRequests,
			&channel.Caps.Tasks, &channel.Caps.Daily, &channel.Caps.Checkin,
			&channel.Platforms, &channel.ModelAllow, &channel.ProbeModel,
		); err != nil {
			return nil, fmt.Errorf("扫描渠道失败: %w", err)
		}
		list = append(list, channel)
	}

	return list, rows.Err()
}

// GetChannel 按 id 取渠道。
func (s *Store) GetChannel(ctx context.Context, id string) (*Channel, error) {
	list, err := s.ListChannels(ctx)
	if err != nil {
		return nil, err
	}

	for _, channel := range list {
		if channel.ID == id {
			return &channel, nil
		}
	}

	return nil, ErrNotFound
}

// ChannelInput 是新建/更新渠道的输入。
type ChannelInput struct {
	ID               string          `json:"id"`
	Name             string          `json:"name"`
	Adapter          string          `json:"adapter"`
	Enabled          *bool           `json:"enabled"`
	Priority         *int            `json:"priority"`
	Weight           *int            `json:"weight"`
	Options          json.RawMessage `json:"options"`
	ChannelRatio     *float64        `json:"channelRatio"`
	ConcurrencyLimit *int            `json:"concurrencyLimit"`
	RateLimitQPS     *int            `json:"rateLimitQps"`
	// ModelAllow 为 nil 表示"不改"；传空数组表示"清空限制"。
	ModelAllow []string `json:"modelAllow"`
	// ProbeModel 是探活使用的对外模型名；空 = 由网关按「免费优先」自动挑。
	//
	// 允许指定是因为"免费模型通了"不等于"真实模型能通"：
	// 号被单独限权、或分组只挂了推理模型时，探针要给出一致的结论。
	ProbeModel *string `json:"probeModel"`
}

// CreateChannel 新建渠道。
func (s *Store) CreateChannel(ctx context.Context, input ChannelInput) (*Channel, error) {
	enabled := true
	if input.Enabled != nil {
		enabled = *input.Enabled
	}
	priority := 100
	if input.Priority != nil {
		priority = *input.Priority
	}
	weight := 1
	if input.Weight != nil {
		weight = *input.Weight
	}
	ratio := 1.0
	if input.ChannelRatio != nil {
		ratio = *input.ChannelRatio
	}
	options := input.Options
	if len(options) == 0 {
		options = json.RawMessage(`{}`)
	}

	const query = `
		INSERT INTO channels (id, name, adapter, enabled, priority, weight, options,
		                      channel_ratio, concurrency_limit, rate_limit_qps, model_allow)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)`

	if _, err := s.pool.Exec(ctx, query,
		input.ID, input.Name, input.Adapter, enabled, priority, weight, options,
		ratio, input.ConcurrencyLimit, input.RateLimitQPS,
		// nil 存 NULL（= 不限制）；空数组也当 NULL —— 两者语义相同，
		// 存成空数组会让"未配置"和"配置了空"出现两种表示。
		emptyToNil(input.ModelAllow),
	); err != nil {
		return nil, fmt.Errorf("新建渠道失败: %w", err)
	}

	// 能力缓存先按适配器写一条默认值，网关启动后会被真实探测覆盖。
	caps := defaultCaps(input.Adapter, options)
	if _, err := s.pool.Exec(ctx,
		`INSERT INTO channel_caps (channel_id, tasks, daily, checkin)
		 VALUES ($1, $2, $3, $4)
		 ON CONFLICT (channel_id) DO UPDATE SET
		     tasks = EXCLUDED.tasks, daily = EXCLUDED.daily, checkin = EXCLUDED.checkin,
		     updated_at = now()`,
		input.ID, caps.Tasks, caps.Daily, caps.Checkin,
	); err != nil {
		return nil, fmt.Errorf("写入渠道能力失败: %w", err)
	}

	return s.GetChannel(ctx, input.ID)
}

// UpdateChannel 更新渠道。
func (s *Store) UpdateChannel(ctx context.Context, id string, input ChannelInput) (*Channel, error) {
	// 动态拼 SET：只更新传了的字段，避免"没传 = 清空"。
	sets := make([]string, 0, 8)
	args := make([]any, 0, 10)
	arg := func(value any) string {
		args = append(args, value)
		return fmt.Sprintf("$%d", len(args))
	}

	if input.Name != "" {
		sets = append(sets, "name = "+arg(input.Name))
	}
	if input.Enabled != nil {
		sets = append(sets, "enabled = "+arg(*input.Enabled))
	}
	if input.Priority != nil {
		sets = append(sets, "priority = "+arg(*input.Priority))
	}
	if input.Weight != nil {
		sets = append(sets, "weight = "+arg(*input.Weight))
	}
	if len(input.Options) > 0 {
		sets = append(sets, "options = "+arg(input.Options))
	}
	if input.ChannelRatio != nil {
		sets = append(sets, "channel_ratio = "+arg(*input.ChannelRatio))
	}
	if input.ConcurrencyLimit != nil {
		sets = append(sets, "concurrency_limit = "+arg(*input.ConcurrencyLimit))
	}
	if input.RateLimitQPS != nil {
		sets = append(sets, "rate_limit_qps = "+arg(*input.RateLimitQPS))
	}
	// nil = 不改；空数组 = 清空（恢复"不限制"）。
	// 两者必须区分：界面上"没动这个字段"和"特意清空"是两件事。
	if input.ModelAllow != nil {
		sets = append(sets, "model_allow = "+arg(emptyToNil(input.ModelAllow)))
	}
	// nil = 不改；空串 = 恢复"自动挑"（存 NULL）。
	if input.ProbeModel != nil {
		sets = append(sets, "probe_model = "+arg(nilIfEmpty(*input.ProbeModel)))
	}

	if len(sets) == 0 {
		return s.GetChannel(ctx, id)
	}

	sets = append(sets, "updated_at = now()")
	query := "UPDATE channels SET " + joinSets(sets) + " WHERE id = " + arg(id)

	result, err := s.pool.Exec(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("更新渠道失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return nil, ErrNotFound
	}

	return s.GetChannel(ctx, id)
}

// DeleteChannel 删除渠道（级联删账号）。
func (s *Store) DeleteChannel(ctx context.Context, id string) error {
	result, err := s.pool.Exec(ctx, "DELETE FROM channels WHERE id = $1", id)
	if err != nil {
		return fmt.Errorf("删除渠道失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// SaveChannelCaps 保存网关探测到的运营能力。
func (s *Store) SaveChannelCaps(ctx context.Context, channelID string, caps ChannelCaps) error {
	_, err := s.pool.Exec(ctx,
		`INSERT INTO channel_caps (channel_id, tasks, daily, checkin)
		 VALUES ($1, $2, $3, $4)
		 ON CONFLICT (channel_id) DO UPDATE SET
		     tasks = EXCLUDED.tasks, daily = EXCLUDED.daily, checkin = EXCLUDED.checkin,
		     updated_at = now()`,
		channelID, caps.Tasks, caps.Daily, caps.Checkin,
	)
	if err != nil {
		return fmt.Errorf("保存渠道能力失败: %w", err)
	}
	return nil
}

// defaultCaps 按适配器给出能力默认值（网关探测失败时的兜底）。
//
// 与网关的 `ops_caps()` 保持一致；两边都改才算改对。
func defaultCaps(adapter string, options json.RawMessage) ChannelCaps {
	switch adapter {
	case "workbuddy":
		var opts struct {
			Realm string `json:"realm"`
		}
		_ = json.Unmarshal(options, &opts)
		isCn := opts.Realm == "" || opts.Realm == "cn"
		/*
		  能力按版本分（对齐参考实现）：

		  国内版有成长任务 + 签到 + 猫猫旅行；
		  国际版**明确不适用国内成长任务**（参考实现在入口处直接返回
		  "国际版不适用国内成长任务中心"），只有每日活跃打卡。

		  所以国际版的 Tasks / Checkin 都是 false，仅 Daily 为 true。
		  早先这里对国际版也给了 Checkin=true，界面上会出现一个
		  点了必然失败的签到按钮。
		*/
		return ChannelCaps{Tasks: isCn, Daily: true, Checkin: isCn}
	case "trae":
		return ChannelCaps{Tasks: false, Daily: false, Checkin: true}
	default:
		return ChannelCaps{}
	}
}

func joinSets(sets []string) string {
	result := ""
	for index, item := range sets {
		if index > 0 {
			result += ", "
		}
		result += item
	}
	return result
}

// ensure pgx is referenced (used by Scan of jsonb via RawMessage).
var _ = pgx.ErrNoRows
