// Package store 是管理面的数据访问层。
//
// 分工（SPEC 4.3）：
//   - 配置类（渠道、账号、模型、Key、设置）在这里直连 PG；
//   - 必须贴适配器的操作（登录、刷新、探活、额度、任务）走 gateway 客户端。
//
// 查询统一走 GORM：简单 CRUD 用链式 API，复杂聚合用 `Raw` + 结构体扫描。
// 两者都在同一个 `*gorm.DB` 上，事务、日志、连接池是一套。
package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"relayai/backend/internal/models"

	"gorm.io/gorm"
)

// ErrNotFound 表示记录不存在。
var ErrNotFound = errors.New("记录不存在")

// ErrNotRunning 表示记录存在，但当前状态不允许这个操作。
//
// 与 ErrNotFound 分开：把"已经跑完了所以不能取消"报成"记录不存在"，
// 会让用户去翻 id 是不是写错了，而真正原因在状态。
var ErrNotRunning = errors.New("记录已不在执行中")

// Store 持有 GORM 连接。
type Store struct {
	db *gorm.DB
}

// New 创建仓储。
func New(db *gorm.DB) *Store {
	return &Store{db: db}
}

// DB 暴露底层句柄（统计类查询与事务里用）。
func (s *Store) DB() *gorm.DB {
	return s.db
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
	// 保留实际平台集合，方便识别历史遗留的跨平台归组数据。
	Platforms []string `json:"platforms"`
	// ModelAllow 是该分组**对外暴露**的模型白名单（空 = 不限制）。
	//
	// 与账号级 `model_allow` 的区别：那个是"这个号**能跑**什么"（能力），
	// 这个是"这个分组**愿意卖**什么"（策略）。
	ModelAllow []string `json:"modelAllow"`
	// ProbeModel 是探活使用的对外模型名；空 = 由网关自动挑。
	ProbeModel string `json:"probeModel"`
}

// ChannelCaps 是缓存的运营能力。
type ChannelCaps struct {
	Tasks   bool `json:"tasks"`
	Daily   bool `json:"daily"`
	Checkin bool `json:"checkin"`
}

// channelRow 是 ListChannels 的扫描目标。
//
// 字段顺序与 `Raw` 里的投影一一对应；标签用 `column:` 让 GORM 按列名回填，
// 不依赖结构体字段顺序 —— 后者在改动投影时极易错位。
type channelRow struct {
	ID               string           `gorm:"column:id"`
	Name             string           `gorm:"column:name"`
	Adapter          string           `gorm:"column:adapter"`
	Enabled          bool             `gorm:"column:enabled"`
	Priority         int              `gorm:"column:priority"`
	Weight           int              `gorm:"column:weight"`
	Options          []byte           `gorm:"column:options"`
	ChannelRatio     float64          `gorm:"column:channel_ratio"`
	ConcurrencyLimit *int             `gorm:"column:concurrency_limit"`
	RateLimitQPS     *int             `gorm:"column:rate_limit_qps"`
	AccountCount     int              `gorm:"column:account_count"`
	TodayRequests    int64            `gorm:"column:today_requests"`
	Tasks            bool             `gorm:"column:tasks"`
	Daily            bool             `gorm:"column:daily"`
	Checkin          bool             `gorm:"column:checkin"`
	Platforms        models.TextArray `gorm:"column:platforms"`
	ModelAllow       models.TextArray `gorm:"column:model_allow"`
	ProbeModel       string           `gorm:"column:probe_model"`
}

// ListChannels 返回全部渠道，含账号数与今日调用量。
//
// 为什么仍是 `Raw`：它一次查询里联合了 4 张表的聚合
// （账号数、今日请求数、能力缓存、平台去重数组），
// GORM 的链式 API 要拆成 4 次查询再在内存里合并 —— 那既慢又要处理空值对齐。
// 保留 SQL 但交给 GORM 执行与扫描，统一在同一个机制下。
func (s *Store) ListChannels(ctx context.Context) ([]Channel, error) {
	const query = `
		SELECT c.id, c.name, c.adapter, c.enabled, c.priority, c.weight, c.options,
		       c.channel_ratio::float8 AS channel_ratio,
		       c.concurrency_limit, c.rate_limit_qps,
		       COALESCE(a.cnt, 0) AS account_count,
		       COALESCE(u.cnt, 0) AS today_requests,
		       COALESCE(cap.tasks, false) AS tasks,
		       COALESCE(cap.daily, false) AS daily,
		       COALESCE(cap.checkin, false) AS checkin,
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

	var rows []channelRow
	if err := s.db.WithContext(ctx).Raw(query).Scan(&rows).Error; err != nil {
		return nil, fmt.Errorf("查询渠道失败: %w", err)
	}

	list := make([]Channel, 0, len(rows))
	for _, row := range rows {
		item := Channel{
			ID:               row.ID,
			Name:             row.Name,
			Adapter:          row.Adapter,
			Enabled:          row.Enabled,
			Priority:         row.Priority,
			Weight:           row.Weight,
			ChannelRatio:     row.ChannelRatio,
			ConcurrencyLimit: row.ConcurrencyLimit,
			RateLimitQPS:     row.RateLimitQPS,
			AccountCount:     row.AccountCount,
			TodayRequests:    row.TodayRequests,
			Caps:             ChannelCaps{Tasks: row.Tasks, Daily: row.Daily, Checkin: row.Checkin},
			Platforms:        orEmptyText(row.Platforms),
			ModelAllow:       orEmptyText(row.ModelAllow),
			ProbeModel:       row.ProbeModel,
		}
		// Options 以 json.RawMessage 对外，内部扫成 []byte 再转。
		if len(row.Options) > 0 {
			item.Options = json.RawMessage(row.Options)
		} else {
			item.Options = json.RawMessage(`{}`)
		}
		list = append(list, item)
	}

	return list, nil
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
	// ProbeModel 是旧版单值探活模型；保留兼容。
	ProbeModel *string `json:"probeModel"`
	// ProbeModels 是本分组要探活的模型列表；空 = 探对外暴露的全部。
	//
	// nil = 不改；空数组 = 清空（回到"探全部"）。
	ProbeModels *[]string `json:"probeModels"`
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

	var parsed models.JSONB
	if err := json.Unmarshal(options, &parsed); err != nil {
		return nil, fmt.Errorf("渠道 options 不是合法 JSON: %w", err)
	}

	record := models.Channels{
		ID:               input.ID,
		Name:             input.Name,
		Adapter:          input.Adapter,
		Enabled:          enabled,
		Priority:         priority,
		Weight:           weight,
		Options:          parsed,
		ChRatio:          ratio,
		ConcurrencyLimit: input.ConcurrencyLimit,
		RateLimitQPS:     input.RateLimitQPS,
		// nil 存 NULL（= 不限制）；空数组也当 NULL —— 两者语义相同，
		// 存成空数组会让"未配置"和"配置了空"出现两种表示。
		ModelAllow: models.TextArray(emptyToNil(input.ModelAllow)),
	}

	// 渠道 + 能力缓存必须一起成功：只建渠道不建能力，
	// 界面会显示"没有任务能力"，而实际只是没写缓存。
	err := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := tx.Create(&record).Error; err != nil {
			return fmt.Errorf("新建渠道失败: %w", err)
		}
		caps := defaultCaps(input.Adapter, options)
		return saveCaps(tx, input.ID, caps)
	})
	if err != nil {
		return nil, err
	}

	return s.GetChannel(ctx, input.ID)
}

// UpdateChannel 更新渠道。
func (s *Store) UpdateChannel(ctx context.Context, id string, input ChannelInput) (*Channel, error) {
	if input.Adapter != "" || len(input.Options) > 0 {
		current, err := s.GetChannel(ctx, id)
		if err != nil {
			return nil, err
		}
		if input.Adapter != "" && input.Adapter != current.Adapter {
			return nil, fmt.Errorf("%w：分组创建后不可切换平台", ErrInvalidInput)
		}
		if len(input.Options) > 0 && PlatformIdentity(current.Adapter, current.Options) != PlatformIdentity(current.Adapter, input.Options) {
			return nil, fmt.Errorf("%w：分组创建后不可切换平台版本", ErrInvalidInput)
		}
	}
	// 只写传了的字段：GORM 的 `Updates(map)` 天然跳过零值问题，
	// 用 map 而不是 struct —— struct 的零值会被忽略，无法区分"不改"与"改成 0"。
	updates := map[string]any{}

	if input.Name != "" {
		updates["name"] = input.Name
	}
	if input.Enabled != nil {
		updates["enabled"] = *input.Enabled
	}
	if input.Priority != nil {
		updates["priority"] = *input.Priority
	}
	if input.Weight != nil {
		updates["weight"] = *input.Weight
	}
	if len(input.Options) > 0 {
		var parsed models.JSONB
		if err := json.Unmarshal(input.Options, &parsed); err != nil {
			return nil, fmt.Errorf("渠道 options 不是合法 JSON: %w", err)
		}
		updates["options"] = parsed
	}
	if input.ChannelRatio != nil {
		updates["channel_ratio"] = *input.ChannelRatio
	}
	if input.ConcurrencyLimit != nil {
		updates["concurrency_limit"] = *input.ConcurrencyLimit
	}
	if input.RateLimitQPS != nil {
		updates["rate_limit_qps"] = *input.RateLimitQPS
	}
	// nil = 不改；空数组 = 清空（恢复"不限制"）。
	// 两者必须区分：界面上"没动这个字段"和"特意清空"是两件事。
	if input.ModelAllow != nil {
		updates["model_allow"] = models.TextArray(emptyToNil(input.ModelAllow))
	}
	// nil = 不改；空串 = 恢复"自动挑"（存 NULL）。
	if input.ProbeModel != nil {
		updates["probe_model"] = nilIfEmpty(*input.ProbeModel)
	}
	// nil = 不改；空数组 = 清空（回到"探全部"）。
	if input.ProbeModels != nil {
		updates["probe_models"] = models.TextArray(emptyToNil(*input.ProbeModels))
	}

	if len(updates) == 0 {
		return s.GetChannel(ctx, id)
	}

	result := s.db.WithContext(ctx).
		Model(&models.Channels{}).
		Where("id = ?", id).
		Updates(updates)
	if result.Error != nil {
		return nil, fmt.Errorf("更新渠道失败: %w", result.Error)
	}
	if result.RowsAffected == 0 {
		return nil, ErrNotFound
	}

	return s.GetChannel(ctx, id)
}

// DeleteChannel 删除分组。
//
// **不删账号**：`accounts.channel_id` 的外键是 `ON DELETE SET NULL`
// （见 `database.explicitMigrations` 里的 `20261030_account_survives_channel_delete`）。
// 组里的号会变成"待分配"——它们仍然带着自己的凭据、历史用量和登录态，
// 只是不再属于任何计价桶、不参与对外调度。
//
// 为什么必须这样：账号是一份上游凭据，分组只是计价桶。
// "这个分组我不要了"和"把这些号也删掉"是两件事，级联删除把它们绑成一件，
// 而且不可撤销 —— 一次误点就丢掉了所有登录态。
func (s *Store) DeleteChannel(ctx context.Context, id string) error {
	result := s.db.WithContext(ctx).Where("id = ?", id).Delete(&models.Channels{})
	if result.Error != nil {
		return fmt.Errorf("删除渠道失败: %w", result.Error)
	}
	if result.RowsAffected == 0 {
		return ErrNotFound
	}
	return nil
}

// SaveChannelCaps 保存网关探测到的运营能力。
func (s *Store) SaveChannelCaps(ctx context.Context, channelID string, caps ChannelCaps) error {
	if err := saveCaps(s.db.WithContext(ctx), channelID, caps); err != nil {
		return fmt.Errorf("保存渠道能力失败: %w", err)
	}
	return nil
}

// saveCaps 写入能力缓存（upsert）。
//
// 用 `clause.OnConflict` 而不是先删后插：
// 后者在并发保存时会出现"删掉了但没插进去"的窗口。
func saveCaps(tx *gorm.DB, channelID string, caps ChannelCaps) error {
	record := models.ChannelCaps{
		ChannelID: channelID,
		Tasks:     caps.Tasks,
		Daily:     caps.Daily,
		Checkin:   caps.Checkin,
	}
	return tx.Clauses(onConflictUpdateAll()).
		Create(&record).Error
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
