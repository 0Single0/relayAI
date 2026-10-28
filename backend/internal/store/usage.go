package store

import (
	"context"
	"encoding/json"
	"fmt"
	"time"
)

// UsageLog 是一条请求记录。
type UsageLog struct {
	ID               int64   `json:"id"`
	Ts               string  `json:"ts"`
	RequestID        string  `json:"requestId"`
	KeyID            *int64  `json:"keyId"`
	KeyName          *string `json:"keyName"`
	ChannelID        string  `json:"channelId"`
	AccountUID       string  `json:"accountUid"`
	Protocol         string  `json:"protocol"`
	Model            string  `json:"model"`
	UpstreamModel    *string `json:"upstreamModel"`
	Stream           bool    `json:"stream"`
	Status           string  `json:"status"`
	IsRejection      bool    `json:"isRejection"`
	Attempts         int     `json:"attempts"`
	PromptTokens     int64   `json:"promptTokens"`
	CompletionTokens int64   `json:"completionTokens"`
	ReasoningTokens  int64   `json:"reasoningTokens"`
	CachedTokens     int64   `json:"cachedTokens"`
	TotalTokens      int64   `json:"totalTokens"`
	UsageEstimated   bool    `json:"usageEstimated"`
	UpstreamCost     float64 `json:"upstreamCost"`
	BilledCost       float64 `json:"billedCost"`
	TTFTMs           *int    `json:"ttftMs"`
	TotalMs          int     `json:"totalMs"`
	ErrorDetail      *string `json:"errorDetail"`
	// TokensPerSecond 是输出速度（token/秒），见 `OutputSpeed`。
	// nil 表示这条记录不参与速度统计（样本太小，算出来没有参考意义）。
	TokensPerSecond *float64 `json:"tokensPerSecond"`
	// SpeedEstimated 为 true 表示这是**近似值**（`≈`）。
	SpeedEstimated bool `json:"speedEstimated"`
	// PriceSnapshot 是计费时的单价与倍率快照。
	//
	// nil 表示这条记录早于快照功能 —— 界面上要说"没有价格快照"，
	// 而不是拿今天的价格去反推，那会算出一个和账单对不上的数。
	PriceSnapshot *UsagePriceSnapshot `json:"priceSnapshot"`
}

// UsagePriceSnapshot 是一条用量的价格快照（元 / 百万 token，未乘倍率）。
type UsagePriceSnapshot struct {
	InputPrice     float64 `json:"inputPrice"`
	OutputPrice    float64 `json:"outputPrice"`
	CacheReadPrice float64 `json:"cacheReadPrice"`
	ModelRatio     float64 `json:"modelRatio"`
	ChannelRatio   float64 `json:"channelRatio"`
}

/**
计算输出速度（token/秒）。

口径按需求定死，三档互斥：

| 情况 | 公式 | 门槛 | 是否近似 |
| --- | --- | --- | --- |
| 经路由服务（有 `ttft_ms`） | `completion / (total - ttft)` | 输出 ≥ 100 | 否（思考过程不外露的模型除外） |
| 未经路由（无 `ttft_ms`） | `completion / total` | 输出 ≥ 200 | **是**，带 `≈` |
| 其他 | 不算 | — | — |

为什么门槛不同（100 vs 200）：小样本的除法噪声极大 —— 20 个 token 快 100ms
就能算出 200 tok/s，那不是速度，是计时精度。要求更大的样本才敢拿来做对比。

为什么无 ttft 的要用 `total` 而不是 `total - ttft`：那种记录**没有首字计时**，
等首字的时间被算进了总时长里，所以算出来**偏低**，因此标 `≈` 提示读者别拿它
和精确值直接比。这也是它门槛更高的原因 —— 偏差已经够大了，样本再小就没意义。

为什么从"首字之后"算：用户感受到的流畅度是**吐字阶段**的速度。
把等首字的时间算进分母，会把"首字慢但吐字快"的模型判成慢，那是错的指标。

`reasoning_tokens` 不计入 `completion_tokens`（网关侧已分开统计），
所以思考型模型的分子只含对外可见的输出 —— 这正是"近似"的来源：
它的真实吐字速度被思考阶段拉低了，但我们看不到思考过程。
*/
func computeOutputSpeed(log *UsageLog) {
	if log.IsRejection || log.CompletionTokens <= 0 || log.TotalMs <= 0 {
		return
	}

	// 有首字计时 → 走"经路由"口径。
	//
	// 注意 `TotalMs <= TTFTMs` 的情形（含两者相等）：吐字阶段为 0 或负，
	// 算不出速度，**必须直接返回**而不是掉进下面的近似分支 ——
	// 那会把"首字就结束了"的请求当成"没有首字计时"去用总时长算，
	// 得到一个看起来正常、其实是假的数字。
	if log.TTFTMs != nil && *log.TTFTMs > 0 {
		if log.CompletionTokens < 100 || log.TotalMs <= *log.TTFTMs {
			return
		}
		elapsed := float64(log.TotalMs-*log.TTFTMs) / 1000
		speed := float64(log.CompletionTokens) / elapsed
		log.TokensPerSecond = &speed
		log.SpeedEstimated = false
		return
	}

	// 未经路由：没有首字计时，用总时长，标近似。
	if log.CompletionTokens < 200 {
		return
	}
	elapsed := float64(log.TotalMs) / 1000
	speed := float64(log.CompletionTokens) / elapsed
	log.TokensPerSecond = &speed
	log.SpeedEstimated = true
}

// UsageQuery 是使用记录的筛选条件。
type UsageQuery struct {
	From       string
	To         string
	ChannelID  string
	AccountUID string
	KeyID      *int64
	Model      string
	Status     string
	Page       int
	PageSize   int
}

// ListUsage 分页查询使用记录。
func (s *Store) ListUsage(ctx context.Context, query UsageQuery) ([]UsageLog, int, error) {
	if query.Page < 1 {
		query.Page = 1
	}
	if query.PageSize < 1 || query.PageSize > 200 {
		query.PageSize = 50
	}

	where := "WHERE 1=1"
	args := make([]any, 0, 8)
	add := func(clause string, value any) {
		args = append(args, value)
		where += fmt.Sprintf(clause, len(args))
	}

	if query.From != "" {
		add(" AND u.ts >= $%d", query.From)
	}
	if query.To != "" {
		add(" AND u.ts <= $%d", query.To)
	}
	if query.ChannelID != "" {
		add(" AND u.channel_id = $%d", query.ChannelID)
	}
	if query.AccountUID != "" {
		add(" AND u.account_uid = $%d", query.AccountUID)
	}
	if query.KeyID != nil {
		add(" AND u.key_id = $%d", *query.KeyID)
	}
	if query.Model != "" {
		add(" AND u.model = $%d", query.Model)
	}
	if query.Status != "" {
		add(" AND u.status = $%d", query.Status)
	}

	var total int
	if err := s.pool.QueryRow(ctx, "SELECT count(*) FROM usage_logs u "+where, args...).Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计使用记录失败: %w", err)
	}

	listArgs := append(args, query.PageSize, (query.Page-1)*query.PageSize)
	listQuery := fmt.Sprintf(`
		SELECT u.id, u.ts, u.request_id, u.key_id, k.name,
		       u.channel_id, u.account_uid, u.protocol, u.model, u.upstream_model,
		       u.stream, u.status, u.is_rejection, u.attempts,
		       u.prompt_tokens, u.completion_tokens, u.reasoning_tokens,
		       u.cached_tokens, u.total_tokens, u.usage_estimated,
		       u.upstream_cost::float8, u.billed_cost::float8,
		       u.input_price::float8, u.output_price::float8, u.cache_read_price::float8,
		       u.model_ratio::float8, u.channel_ratio::float8,
		       u.ttft_ms, u.total_ms, u.error_detail
		FROM usage_logs u
		LEFT JOIN api_keys k ON k.id = u.key_id
		%s
		ORDER BY u.ts DESC, u.id DESC
		LIMIT $%d OFFSET $%d`, where, len(listArgs)-1, len(listArgs))

	rows, err := s.pool.Query(ctx, listQuery, listArgs...)
	if err != nil {
		return nil, 0, fmt.Errorf("查询使用记录失败: %w", err)
	}
	defer rows.Close()

	list := make([]UsageLog, 0, query.PageSize)
	for rows.Next() {
		var log UsageLog
		var ts any
		var snapInput, snapOutput, snapCache, snapModelRatio, snapChannelRatio *float64
		if err := rows.Scan(
			&log.ID, &ts, &log.RequestID, &log.KeyID, &log.KeyName,
			&log.ChannelID, &log.AccountUID, &log.Protocol, &log.Model, &log.UpstreamModel,
			&log.Stream, &log.Status, &log.IsRejection, &log.Attempts,
			&log.PromptTokens, &log.CompletionTokens, &log.ReasoningTokens,
			&log.CachedTokens, &log.TotalTokens, &log.UsageEstimated,
			&log.UpstreamCost, &log.BilledCost,
			&snapInput, &snapOutput, &snapCache, &snapModelRatio, &snapChannelRatio,
			&log.TTFTMs, &log.TotalMs, &log.ErrorDetail,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描使用记录失败: %w", err)
		}
		log.Ts = formatTime(ts)
		log.PriceSnapshot = buildPriceSnapshot(snapInput, snapOutput, snapCache, snapModelRatio, snapChannelRatio)
		computeOutputSpeed(&log)
		list = append(list, log)
	}

	return list, total, rows.Err()
}

// GetUsage 按 id 取一条记录。
func (s *Store) GetUsage(ctx context.Context, id int64) (*UsageLog, error) {
	const query = `
		SELECT u.id, u.ts, u.request_id, u.key_id, k.name,
		       u.channel_id, u.account_uid, u.protocol, u.model, u.upstream_model,
		       u.stream, u.status, u.is_rejection, u.attempts,
		       u.prompt_tokens, u.completion_tokens, u.reasoning_tokens,
		       u.cached_tokens, u.total_tokens, u.usage_estimated,
		       u.upstream_cost::float8, u.billed_cost::float8,
		       u.input_price::float8, u.output_price::float8, u.cache_read_price::float8,
		       u.model_ratio::float8, u.channel_ratio::float8,
		       u.ttft_ms, u.total_ms, u.error_detail
		FROM usage_logs u
		LEFT JOIN api_keys k ON k.id = u.key_id
		WHERE u.id = $1`

	var log UsageLog
	var ts any
	var snapInput, snapOutput, snapCache, snapModelRatio, snapChannelRatio *float64
	err := s.pool.QueryRow(ctx, query, id).Scan(
		&log.ID, &ts, &log.RequestID, &log.KeyID, &log.KeyName,
		&log.ChannelID, &log.AccountUID, &log.Protocol, &log.Model, &log.UpstreamModel,
		&log.Stream, &log.Status, &log.IsRejection, &log.Attempts,
		&log.PromptTokens, &log.CompletionTokens, &log.ReasoningTokens,
		&log.CachedTokens, &log.TotalTokens, &log.UsageEstimated,
		&log.UpstreamCost, &log.BilledCost,
			&snapInput, &snapOutput, &snapCache, &snapModelRatio, &snapChannelRatio,
		&log.TTFTMs, &log.TotalMs, &log.ErrorDetail,
	)
	if err != nil {
		return nil, ErrNotFound
	}
	log.Ts = formatTime(ts)
	log.PriceSnapshot = buildPriceSnapshot(snapInput, snapOutput, snapCache, snapModelRatio, snapChannelRatio)
	computeOutputSpeed(&log)

	return &log, nil
}

// buildPriceSnapshot 把可空的快照列组装成结构体。
//
// 全部为 NULL 时返回 nil（这条记录早于快照功能）——
// 只要有一个非空就认为有快照：五列是同一时刻一起写的，不会只写一半。
func buildPriceSnapshot(
	input, output, cacheRead, modelRatio, channelRatio *float64,
) *UsagePriceSnapshot {
	if input == nil && output == nil && cacheRead == nil && modelRatio == nil && channelRatio == nil {
		return nil
	}
	deref := func(value *float64) float64 {
		if value == nil {
			return 0
		}
		return *value
	}
	return &UsagePriceSnapshot{
		InputPrice:     deref(input),
		OutputPrice:    deref(output),
		CacheReadPrice: deref(cacheRead),
		ModelRatio:     deref(modelRatio),
		ChannelRatio:   deref(channelRatio),
	}
}

// UsageSummary 是汇总数据。
type UsageSummary struct {
	Requests         int64   `json:"requests"`
	SuccessRate      float64 `json:"successRate"`
	PromptTokens     int64   `json:"promptTokens"`
	CompletionTokens int64   `json:"completionTokens"`
	TotalTokens      int64   `json:"totalTokens"`
	CachedTokens     int64   `json:"cachedTokens"`
	UpstreamCost     float64 `json:"upstreamCost"`
	BilledCost       float64 `json:"billedCost"`
	AvgTTFTMs        *int    `json:"avgTtftMs"`
}

// SummaryBucket 是按维度聚合的一行。
type SummaryBucket struct {
	Key          string  `json:"key"`
	Label        string  `json:"label"`
	Requests     int64   `json:"requests"`
	TotalTokens  int64   `json:"totalTokens"`
	UpstreamCost float64 `json:"upstreamCost"`
	BilledCost   float64 `json:"billedCost"`
	SuccessRate  float64 `json:"successRate"`
}

// SummaryQuery 是汇总的筛选条件。
type SummaryQuery struct {
	From    string
	To      string
	GroupBy string
}

// GetUsageSummary 返回总量与按维度聚合。
//
// **拒绝行一律排除**（SPEC 5.5）：它们没有真实的模型/渠道归属，
// 混进来会凭空多出 `channel_id = '-'` 的分组并把成功率拉低。
func (s *Store) GetUsageSummary(ctx context.Context, query SummaryQuery) (*UsageSummary, []SummaryBucket, error) {
	where := "WHERE NOT is_rejection"
	args := make([]any, 0, 4)
	if query.From != "" {
		args = append(args, query.From)
		where += fmt.Sprintf(" AND ts >= $%d", len(args))
	}
	if query.To != "" {
		args = append(args, query.To)
		where += fmt.Sprintf(" AND ts <= $%d", len(args))
	}

	var summary UsageSummary
	var avgTTFT *float64
	err := s.pool.QueryRow(ctx, fmt.Sprintf(`
		SELECT count(*),
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END), 0),
		       COALESCE(sum(prompt_tokens), 0),
		       COALESCE(sum(completion_tokens), 0),
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(cached_tokens), 0),
		       COALESCE(sum(upstream_cost), 0)::float8,
		       COALESCE(sum(billed_cost), 0)::float8,
		       avg(ttft_ms)::float8
		FROM usage_logs %s`, where), args...,
	).Scan(
		&summary.Requests, &summary.SuccessRate,
		&summary.PromptTokens, &summary.CompletionTokens, &summary.TotalTokens,
		&summary.CachedTokens, &summary.UpstreamCost, &summary.BilledCost,
		&avgTTFT,
	)
	if err != nil {
		return nil, nil, fmt.Errorf("汇总失败: %w", err)
	}
	if avgTTFT != nil {
		rounded := int(*avgTTFT)
		summary.AvgTTFTMs = &rounded
	}

	column, ok := summaryColumn(query.GroupBy)
	if !ok {
		return &summary, []SummaryBucket{}, nil
	}

	bucketQuery := fmt.Sprintf(`
		SELECT %s AS key,
		       count(*),
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(upstream_cost), 0)::float8,
		       COALESCE(sum(billed_cost), 0)::float8,
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END), 0)
		FROM usage_logs %s
		GROUP BY 1
		ORDER BY 2 DESC
		LIMIT 50`, column, where)

	rows, err := s.pool.Query(ctx, bucketQuery, args...)
	if err != nil {
		return nil, nil, fmt.Errorf("分组统计失败: %w", err)
	}
	defer rows.Close()

	buckets := make([]SummaryBucket, 0, 16)
	for rows.Next() {
		var bucket SummaryBucket
		var key any
		if err := rows.Scan(
			&key, &bucket.Requests, &bucket.TotalTokens,
			&bucket.UpstreamCost, &bucket.BilledCost, &bucket.SuccessRate,
		); err != nil {
			return nil, nil, fmt.Errorf("扫描分组失败: %w", err)
		}
		bucket.Key = formatBucketKey(key, query.GroupBy)
		bucket.Label = bucket.Key
		buckets = append(buckets, bucket)
	}

	return &summary, buckets, rows.Err()
}

// SummarySeriesPoint 是趋势图上的一个点。
type SummarySeriesPoint struct {
	Bucket       string  `json:"bucket"`
	Requests     int64   `json:"requests"`
	TotalTokens  int64   `json:"totalTokens"`
	UpstreamCost float64 `json:"upstreamCost"`
	// 输入/输出分开给：Token 趋势图要堆叠显示，只有 total 看不出
	// "是长上下文还是长回复"，而这两种情况的优化方向完全不同。
	PromptTokens     int64 `json:"promptTokens"`
	CompletionTokens int64 `json:"completionTokens"`
	// 该桶命中的缓存 Token。命中率高说明前缀复用得好，
	// 但它不计入 total_tokens，所以必须单独给，否则趋势图会漏一块。
	CachedTokens int64 `json:"cachedTokens"`
	// 该桶内的平均首字延迟（毫秒）；没有数据时为 null。
	// 用指针而不是 0：0ms 是"快得不像话"，null 才是"这个时段没请求"。
	AvgTtftMs *int `json:"avgTtftMs"`
	// 该桶内的平均总耗时（毫秒）；没有数据时为 null。
	AvgLatencyMs *int `json:"avgLatencyMs"`
	// 该桶内的成功率（0–1）。
	SuccessRate float64 `json:"successRate"`
	// 该桶内失败请求数（含上游断流、客户端断开）。
	Failed int64 `json:"failed"`
}

// SeriesQuery 是趋势查询条件。
type SeriesQuery struct {
	From   string
	To     string
	Model  string
	Bucket string
}

// GetUsageSeries 返回按小时/天聚合的趋势。
func (s *Store) GetUsageSeries(ctx context.Context, query SeriesQuery) ([]SummarySeriesPoint, error) {
	unit := "hour"
	if query.Bucket == "day" {
		unit = "day"
	}

	where := "WHERE NOT is_rejection"
	args := make([]any, 0, 4)
	if query.From != "" {
		args = append(args, query.From)
		where += fmt.Sprintf(" AND ts >= $%d", len(args))
	}
	if query.To != "" {
		args = append(args, query.To)
		where += fmt.Sprintf(" AND ts <= $%d", len(args))
	}
	if query.Model != "" {
		args = append(args, query.Model)
		where += fmt.Sprintf(" AND model = $%d", len(args))
	}

	sql := fmt.Sprintf(`
		SELECT date_trunc('%s', ts) AS bucket,
		       count(*),
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(upstream_cost), 0)::float8,
		       COALESCE(sum(prompt_tokens), 0),
		       COALESCE(sum(completion_tokens), 0),
		       COALESCE(sum(cached_tokens), 0),
		       avg(ttft_ms)::float8,
		       avg(total_ms)::float8,
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END), 0)::float8,
		       COALESCE(sum(CASE WHEN status = 'completed' THEN 0 ELSE 1 END), 0)
		FROM usage_logs %s
		GROUP BY 1
		ORDER BY 1`, unit, where)

	rows, err := s.pool.Query(ctx, sql, args...)
	if err != nil {
		return nil, fmt.Errorf("查询趋势失败: %w", err)
	}
	defer rows.Close()

	points := make([]SummarySeriesPoint, 0, 24)
	for rows.Next() {
		var point SummarySeriesPoint
		var bucket time.Time
		var avgTtft, avgLatency *float64
		if err := rows.Scan(
			&bucket, &point.Requests, &point.TotalTokens, &point.UpstreamCost,
			&point.PromptTokens, &point.CompletionTokens, &point.CachedTokens,
			&avgTtft, &avgLatency, &point.SuccessRate, &point.Failed,
		); err != nil {
			return nil, fmt.Errorf("扫描趋势失败: %w", err)
		}
		point.Bucket = bucket.Format(time.RFC3339)
		point.AvgTtftMs = roundToInt(avgTtft)
		point.AvgLatencyMs = roundToInt(avgLatency)
		points = append(points, point)
	}

	return points, rows.Err()
}

// summaryColumn 把分组名映射到 SQL 列。
func summaryColumn(groupBy string) (string, bool) {
	switch groupBy {
	case "channel":
		return "channel_id", true
	case "model":
		return "model", true
	case "key":
		return "COALESCE(key_id::text, 'anonymous')", true
	case "day":
		return "to_char(date_trunc('day', ts), 'YYYY-MM-DD')", true
	default:
		return "", false
	}
}

// formatBucketKey 处理可能为 NULL 的分组键。
func formatBucketKey(value any, groupBy string) string {
	switch typed := value.(type) {
	case string:
		return typed
	case nil:
		return "unknown"
	default:
		return fmt.Sprintf("%v", typed)
	}
}

// Settings 是管理面设置。
type Settings struct {
	TaskRetentionDays   int    `json:"taskRetentionDays"`
	TaskScheduleEnabled bool   `json:"taskScheduleEnabled"`
	TaskScheduleHours   []int  `json:"taskScheduleHours"`
	TaskPerAccountGapMs int    `json:"taskPerAccountGapMs"`
	TaskDailyGapMs      int    `json:"taskDailyGapMs"`
	TaskReportGapMs     int    `json:"taskReportGapMs"`
	GatewayPort         int    `json:"gatewayPort"`
	GatewayBind         string `json:"gatewayBind"`
	LogRetentionDays    int    `json:"logRetentionDays"`
	// WebhookURL 是告警通知地址（SPEC 9.13）。空 = 不发通知。
	WebhookURL string `json:"webhookUrl"`
	// ProbeEnabled 是"定时探活"开关。
	//
	// 探活 = 每隔 N 分钟对每个启用的账号发一条**最小**请求（一句 hello），
	// 用来尽早发现"凭据过期/被封/额度耗尽"，不等真实用户请求失败才知道。
	// 关掉它则只在有真实流量时才会发现故障。
	ProbeEnabled bool `json:"probeEnabled"`
	// ProbeIntervalMinutes 是探活间隔（分钟）。
	//
	// 下限 5 分钟：探活会真实消耗上游额度，间隔太短等于自己烧钱。
	ProbeIntervalMinutes int `json:"probeIntervalMinutes"`
	// PayloadEnabled 是"留存请求/响应内容"开关。**默认关闭**。
	//
	// 这是隐私敏感功能：留存的是用户完整对话内容。
	// 打开后可以逐条回放，用于复现"模型没按格式回"这类问题。
	PayloadEnabled bool `json:"payloadEnabled"`
	// USDToCNY 是美元兑人民币汇率，用于把内置/OpenRouter 的美元价格折算成元。
	//
	// 做成设置项而不是常量：写死一个数字，半年后整张价格表都会偏。
	USDToCNY float64 `json:"usdToCny"`
	// PayloadRetentionDays 是内容留存天数，独立于 `LogRetentionDays`。
	//
	// 内容比元数据敏感得多，通常要更短。
	PayloadRetentionDays int `json:"payloadRetentionDays"`
}

// GetSettings 读设置（缺失的键用默认值补齐）。
func (s *Store) GetSettings(ctx context.Context) (*Settings, error) {
	rows, err := s.pool.Query(ctx, "SELECT key, value FROM settings")
	if err != nil {
		return nil, fmt.Errorf("读取设置失败: %w", err)
	}
	defer rows.Close()

	values := make(map[string]json.RawMessage)
	for rows.Next() {
		var key string
		var value json.RawMessage
		if err := rows.Scan(&key, &value); err != nil {
			return nil, fmt.Errorf("扫描设置失败: %w", err)
		}
		values[key] = value
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}

	settings := defaultSettings()
	read := func(key string, target any) {
		if raw, ok := values[key]; ok {
			_ = json.Unmarshal(raw, target)
		}
	}

	read("task.retention_days", &settings.TaskRetentionDays)
	read("task.schedule_enabled", &settings.TaskScheduleEnabled)
	read("task.schedule_hours", &settings.TaskScheduleHours)
	read("task.per_account_gap_ms", &settings.TaskPerAccountGapMs)
	read("task.daily_gap_ms", &settings.TaskDailyGapMs)
	read("task.report_gap_ms", &settings.TaskReportGapMs)
	read("gateway.port", &settings.GatewayPort)
	read("gateway.bind", &settings.GatewayBind)
	read("log.retention_days", &settings.LogRetentionDays)
	read("notify.webhook_url", &settings.WebhookURL)
	read("probe.enabled", &settings.ProbeEnabled)
	read("probe.interval_minutes", &settings.ProbeIntervalMinutes)
	read("payload.enabled", &settings.PayloadEnabled)
	read("payload.retention_days", &settings.PayloadRetentionDays)
	read("pricing.usd_to_cny", &settings.USDToCNY)

	return settings, nil
}

// SaveSettings 保存设置。
func (s *Store) SaveSettings(ctx context.Context, settings *Settings) error {
	pairs := map[string]any{
		"task.retention_days":     settings.TaskRetentionDays,
		"task.schedule_enabled":   settings.TaskScheduleEnabled,
		"task.schedule_hours":     settings.TaskScheduleHours,
		"task.per_account_gap_ms": settings.TaskPerAccountGapMs,
		"task.daily_gap_ms":       settings.TaskDailyGapMs,
		"task.report_gap_ms":      settings.TaskReportGapMs,
		"gateway.port":            settings.GatewayPort,
		"gateway.bind":            settings.GatewayBind,
		"log.retention_days":      settings.LogRetentionDays,
		"notify.webhook_url":      settings.WebhookURL,
		"probe.enabled":           settings.ProbeEnabled,
		"probe.interval_minutes":  settings.ProbeIntervalMinutes,
		"payload.enabled":         settings.PayloadEnabled,
		"payload.retention_days":  settings.PayloadRetentionDays,
		"pricing.usd_to_cny":      settings.USDToCNY,
	}

	for key, value := range pairs {
		payload, err := json.Marshal(value)
		if err != nil {
			return fmt.Errorf("序列化设置 %s 失败: %w", key, err)
		}
		if _, err := s.pool.Exec(ctx,
			`INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, now())
			 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()`,
			key, payload,
		); err != nil {
			return fmt.Errorf("保存设置 %s 失败: %w", key, err)
		}
	}

	return nil
}

func defaultSettings() *Settings {
	return &Settings{
		TaskRetentionDays:   90,
		TaskScheduleEnabled: true,
		TaskScheduleHours:   []int{9, 21},
		TaskPerAccountGapMs: 800,
		TaskDailyGapMs:      1200,
		TaskReportGapMs:     1100,
		GatewayPort:         18788,
		GatewayBind:         "127.0.0.1",
		LogRetentionDays:    30,
		WebhookURL:          "",
		// 探活默认开：故障早发现的价值大于那点额度消耗（每次只发一句 hello）。
		ProbeEnabled:         true,
		// 5 分钟：最短的合法间隔。故障发现得快，代价是每号每 5 分钟一句 hello。
		ProbeIntervalMinutes: 5,
		// 内容留存默认**关闭**：隐私敏感，必须显式打开。
		PayloadEnabled:       false,
		PayloadRetentionDays: 7,
		USDToCNY:             7.2,
	}
}
