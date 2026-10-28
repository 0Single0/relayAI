package store

import (
	"context"
	"fmt"
	"math"
	"time"
)

// ChannelHealth 是单个渠道的健康视图（「渠道状态」页的一张卡片）。
//
// 设计原则：**能测到什么就只显示什么**。没有请求过的渠道，
// 成功率与延迟返回 nil（界面显示 `—`），不要填 0 —— 0% 会让人以为渠道挂了。
type ChannelHealth struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Adapter string `json:"adapter"`
	Enabled bool   `json:"enabled"`

	// 账号数来自 PG（唯一事实来源）。
	AccountCount int `json:"accountCount"`
	// 可用账号数来自网关运行态；网关不可用时为 nil。
	ReadyAccounts *int `json:"readyAccounts"`

	// 窗口内统计（默认近 7 天）。
	Requests     int64    `json:"requests"`
	SuccessRate  *float64 `json:"successRate"`
	AvgTtftMs    *int     `json:"avgTtftMs"`
	AvgLatencyMs *int     `json:"avgLatencyMs"`
	TotalTokens  int64    `json:"totalTokens"`
	BilledCost   float64  `json:"billedCost"`

	// 最近若干次请求的状态（旧 → 新），用于卡片底部那条细条。
	// 值为 usage_logs.status 原样（completed / failed / …），
	// 不在这里做"成功/失败"归类：归类是展示决策，属于前端。
	Recent []string `json:"recent"`

	// 最近一次失败的原因原文（没有失败则为 nil）。
	LastError *string `json:"lastError"`
	LastErrorAt *string `json:"lastErrorAt"`

	// ProbeModel 是这个分组**探活时使用的模型**。
	//
	// 空字符串 = 没有指定，由网关按「免费优先」自动挑。
	// 为什么允许指定：有些分组的免费模型并不代表真实能力（比如只挂了推理模型），
	// 用免费档探活会给出"通"的假象，而真实请求仍会失败。
	ProbeModel string `json:"probeModel"`
}

// channelHealthRecentLimit 是卡片细条上的格数。
//
// 60 与参考实现一致：再多就不是"一眼看出刚才有没有抖"，
// 而是变成一张需要仔细看的图，失去了卡片的意义。
const channelHealthRecentLimit = 60

// ListChannelHealth 返回全部渠道的健康统计。
//
// 窗口从 `from` 起算（零值则取近 7 天）。三处查询共用同一个窗口，
// 否则卡片上的成功率与细条格数会对不上，用户会怀疑数据是假的。
func (s *Store) ListChannelHealth(ctx context.Context, from time.Time) ([]ChannelHealth, error) {
	if from.IsZero() {
		from = time.Now().Add(-7 * 24 * time.Hour)
	}

	list, err := s.listChannelBase(ctx)
	if err != nil {
		return nil, err
	}
	if len(list) == 0 {
		return list, nil
	}

	index := make(map[string]int, len(list))
	for position := range list {
		index[list[position].ID] = position
	}

	if err := s.attachChannelUsage(ctx, list, index, from); err != nil {
		return nil, err
	}
	if err := s.attachChannelRecent(ctx, list, index, from); err != nil {
		return nil, err
	}
	if err := s.attachChannelLastError(ctx, list, index); err != nil {
		return nil, err
	}

	return list, nil
}

// listChannelBase 取渠道基本信息与账号数。
func (s *Store) listChannelBase(ctx context.Context) ([]ChannelHealth, error) {
	const query = `
		SELECT c.id, c.name, c.adapter, c.enabled, COALESCE(a.cnt, 0), COALESCE(c.probe_model, '')
		FROM channels c
		LEFT JOIN (
			SELECT channel_id, count(*) AS cnt FROM accounts GROUP BY channel_id
		) a ON a.channel_id = c.id
		ORDER BY c.priority DESC, c.id`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询渠道健康度失败: %w", err)
	}
	defer rows.Close()

	list := make([]ChannelHealth, 0, 8)
	for rows.Next() {
		var item ChannelHealth
		if err := rows.Scan(
			&item.ID, &item.Name, &item.Adapter, &item.Enabled, &item.AccountCount, &item.ProbeModel,
		); err != nil {
			return nil, fmt.Errorf("扫描渠道健康度失败: %w", err)
		}
		item.Recent = []string{}
		list = append(list, item)
	}

	return list, rows.Err()
}

// attachChannelUsage 填窗口内的聚合指标。
//
// 拒绝行必须排除（SPEC 5.5）：它们没有真实归属，混进来会把成功率压低，
// 而用户看到的"渠道可用性"应该是"这个渠道的服务质量"，不是"我 Key 配错了"。
func (s *Store) attachChannelUsage(
	ctx context.Context,
	list []ChannelHealth,
	index map[string]int,
	from time.Time,
) error {
	const query = `
		SELECT channel_id,
		       count(*),
		       avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END)::float8,
		       avg(ttft_ms)::float8,
		       avg(total_ms)::float8,
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(billed_cost), 0)::float8
		FROM usage_logs
		WHERE NOT is_rejection AND ts >= $1
		GROUP BY channel_id`

	rows, err := s.pool.Query(ctx, query, from)
	if err != nil {
		return fmt.Errorf("统计渠道用量失败: %w", err)
	}
	defer rows.Close()

	for rows.Next() {
		var channelID string
		var requests int64
		var successRate, avgTtft, avgLatency *float64
		var totalTokens int64
		var billedCost float64

		if err := rows.Scan(
			&channelID, &requests, &successRate, &avgTtft, &avgLatency,
			&totalTokens, &billedCost,
		); err != nil {
			return fmt.Errorf("扫描渠道用量失败: %w", err)
		}

		position, ok := index[channelID]
		if !ok {
			// 渠道已删除但历史还在：不新建卡片（那会凭空多出一个不存在的渠道）。
			continue
		}

		item := &list[position]
		item.Requests = requests
		item.SuccessRate = successRate
		item.AvgTtftMs = roundToInt(avgTtft)
		item.AvgLatencyMs = roundToInt(avgLatency)
		item.TotalTokens = totalTokens
		item.BilledCost = billedCost
	}

	return rows.Err()
}

// attachChannelRecent 填卡片底部细条的数据。
func (s *Store) attachChannelRecent(
	ctx context.Context,
	list []ChannelHealth,
	index map[string]int,
	from time.Time,
) error {
	const query = `
		SELECT channel_id, status
		FROM (
			SELECT channel_id, status, ts, id,
			       row_number() OVER (
			           PARTITION BY channel_id ORDER BY ts DESC, id DESC
			       ) AS position
			FROM usage_logs
			WHERE NOT is_rejection AND ts >= $1
		) recent
		WHERE position <= $2
		ORDER BY channel_id, ts, id`

	rows, err := s.pool.Query(ctx, query, from, channelHealthRecentLimit)
	if err != nil {
		return fmt.Errorf("查询渠道最近请求失败: %w", err)
	}
	defer rows.Close()

	for rows.Next() {
		var channelID, status string
		if err := rows.Scan(&channelID, &status); err != nil {
			return fmt.Errorf("扫描渠道最近请求失败: %w", err)
		}
		if position, ok := index[channelID]; ok {
			list[position].Recent = append(list[position].Recent, status)
		}
	}

	return rows.Err()
}

// attachChannelLastError 填最近一次失败的原因。
func (s *Store) attachChannelLastError(
	ctx context.Context,
	list []ChannelHealth,
	index map[string]int,
) error {
	const query = `
		SELECT DISTINCT ON (channel_id) channel_id, error_detail, ts
		FROM usage_logs
		WHERE NOT is_rejection
		  AND status <> 'completed'
		  AND error_detail IS NOT NULL
		ORDER BY channel_id, ts DESC`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return fmt.Errorf("查询渠道最近错误失败: %w", err)
	}
	defer rows.Close()

	for rows.Next() {
		var channelID, detail string
		var ts any
		if err := rows.Scan(&channelID, &detail, &ts); err != nil {
			return fmt.Errorf("扫描渠道最近错误失败: %w", err)
		}
		if position, ok := index[channelID]; ok {
			list[position].LastError = &detail
			formatted := formatTime(ts)
			list[position].LastErrorAt = &formatted
		}
	}

	return rows.Err()
}

// roundToInt 把可能为 nil 的平均值四舍五入成整数毫秒。
//
// nil 必须保持 nil：`avg()` 在没有非空行时返回 NULL，
// 把它写成 0 会让"没测过"变成"0 毫秒"，那是假的。
func roundToInt(value *float64) *int {
	if value == nil {
		return nil
	}
	rounded := int(*value + 0.5)
	return &rounded
}

// ---------------------------------------------------------------------------
// 仪表盘总览
// ---------------------------------------------------------------------------

// DashboardTotals 是一段区间内的累计指标。
type DashboardTotals struct {
	Requests         int64    `json:"requests"`
	SuccessRate      float64  `json:"successRate"`
	PromptTokens     int64    `json:"promptTokens"`
	CompletionTokens int64    `json:"completionTokens"`
	TotalTokens      int64    `json:"totalTokens"`
	CachedTokens     int64    `json:"cachedTokens"`
	UpstreamCost     float64  `json:"upstreamCost"`
	BilledCost       float64  `json:"billedCost"`
	AvgTtftMs        *int     `json:"avgTtftMs"`
	AvgLatencyMs     *int     `json:"avgLatencyMs"`
}

// ProtocolBucket 是按协议拆分的用量（三种入口各占多少）。
type ProtocolBucket struct {
	Protocol string `json:"protocol"`
	Requests int64  `json:"requests"`
	Tokens   int64  `json:"tokens"`
}

// DashboardOverview 是仪表盘一次请求拿到的全部数据。
//
// 为什么合成一个接口：仪表盘要 5 张图 + 8 个指标卡，拆成 8 次请求会让
// 它们各自落在不同的时间点，"今日请求 120" 和 "今日 token 3.2M" 可能来自
// 相隔两秒的两个快照，看起来就像数据错了。一个接口 = 一个快照。
type DashboardOverview struct {
	// 窗口（趋势图的横轴范围）。
	From string `json:"from"`
	To   string `json:"to"`
	// Today 是自然日（本地时区零点起），不是"近 24 小时"：
	// 用户说"今日"指的就是今天，滚动 24 小时会和日历对不上。
	Today DashboardTotals `json:"today"`
	// Total 是全量累计（不设下限），用于"累计 token"这类指标。
	Total DashboardTotals `json:"total"`

	Series     []SummarySeriesPoint `json:"series"`
	ByChannel  []SummaryBucket      `json:"byChannel"`
	ByModel    []SummaryBucket      `json:"byModel"`
	ByProtocol []ProtocolBucket     `json:"byProtocol"`

	// TodayByHour 是今日逐小时的请求数与 token（"今天什么时候最忙"）。
	TodayByHour []SummarySeriesPoint `json:"todayByHour"`

	// Rates 是今日的吞吐速率。
	//
	// 为什么单独算而不是让前端用总数除时间：RPM/TPM 的分母是"已经过去的时间"，
	// 而不是 24 小时。凌晨统计时用 86400 秒做分母会把速率稀释成 1/N，
	// 看起来像系统没在干活。这里用"今天零点到当前时刻"的秒数。
	Rates RateMetrics `json:"rates"`

	// Errors 是窗口内的错误分布（按错误类型分组）。
	//
	// 排障时最需要的是"哪类错误最多"，而不是"成功率 97%"。
	// 成功率只能说明有问题，错误分布才指出问题在哪。
	Errors []ErrorBucket `json:"errors"`

	// Percentiles 是窗口内的延迟分位（P50 / P95 / P99）。
	//
	// 为什么不用平均值：平均值会被长尾拉偏 —— 99 个快请求 + 1 个超时
	// 算出来的平均看起来正常，但用户实际感受到的是那 1 个卡死。
	Percentiles LatencyPercentiles `json:"percentiles"`
}

// ErrorBucket 是一类错误的出现次数。
type ErrorBucket struct {
	// Kind 是错误类型（`rate_limit` / `auth` / `timeout` / …）。
	Kind string `json:"kind"`
	// Count 是次数。
	Count int64 `json:"count"`
	// Sample 是一条代表性错误信息（截断，脱敏后的）。
	Sample string `json:"sample"`
}

// LatencyPercentiles 是延迟分位（毫秒）。
type LatencyPercentiles struct {
	// 首字延迟分位；没有数据时为 null。
	TtftP50 *int `json:"ttftP50"`
	TtftP95 *int `json:"ttftP95"`
	TtftP99 *int `json:"ttftP99"`
	// 总耗时分位。
	TotalP50 *int `json:"totalP50"`
	TotalP95 *int `json:"totalP95"`
	TotalP99 *int `json:"totalP99"`
	// 样本量：分位数的可信度取决于它。
	Sample int64 `json:"sample"`
}

// RateMetrics 是吞吐速率（今日口径）。
type RateMetrics struct {
	// RPM 是每分钟请求数。
	RPM float64 `json:"rpm"`
	// TPM 是每分钟 Token 数。
	TPM float64 `json:"tpm"`
	// PeakRPM 是今日单小时最高请求数折算的每分钟速率。
	// 平均值会被空闲时段拉低，峰值才反映"最忙时扛住了多少"。
	PeakRPM float64 `json:"peakRpm"`
	// 今日平均首字延迟（毫秒）；没有数据时为 null。
	AvgTtftMs *int `json:"avgTtftMs"`
	// 今日平均总耗时（毫秒）；没有数据时为 null。
	AvgLatencyMs *int `json:"avgLatencyMs"`
	// ElapsedSeconds 是今日已过去的秒数，用于解释 RPM 的分母。
	ElapsedSeconds int64 `json:"elapsedSeconds"`
}

// GetDashboardOverview 汇总仪表盘数据。
func (s *Store) GetDashboardOverview(
	ctx context.Context,
	from, to time.Time,
	bucket string,
) (*DashboardOverview, error) {
	todayStart := startOfLocalDay(to)

	overview := &DashboardOverview{
		From: from.Format(time.RFC3339),
		To:   to.Format(time.RFC3339),
	}

	today, err := s.totalsBetween(ctx, todayStart, to)
	if err != nil {
		return nil, err
	}
	overview.Today = *today

	// 全量：不传下限。用零值时间当"无限早"，PG 能正确处理。
	total, err := s.totalsBetween(ctx, time.Time{}, to)
	if err != nil {
		return nil, err
	}
	overview.Total = *total

	series, err := s.GetUsageSeries(ctx, SeriesQuery{
		From:   from.Format(time.RFC3339),
		To:     to.Format(time.RFC3339),
		Bucket: bucket,
	})
	if err != nil {
		return nil, err
	}
	overview.Series = series

	todaySeries, err := s.GetUsageSeries(ctx, SeriesQuery{
		From:   todayStart.Format(time.RFC3339),
		To:     to.Format(time.RFC3339),
		Bucket: "hour",
	})
	if err != nil {
		return nil, err
	}
	overview.TodayByHour = todaySeries

	byChannel, err := s.bucketsBetween(ctx, "channel_id", todayStart, to)
	if err != nil {
		return nil, err
	}
	overview.ByChannel = byChannel

	byModel, err := s.bucketsBetween(ctx, "model", todayStart, to)
	if err != nil {
		return nil, err
	}
	overview.ByModel = byModel

	byProtocol, err := s.protocolBuckets(ctx, todayStart, to)
	if err != nil {
		return nil, err
	}
	overview.ByProtocol = byProtocol

	overview.Rates = computeRates(todayStart, to, today, todaySeries)

	// 错误分布与延迟分位：都基于窗口内的非拒绝记录。
	// 拒绝行（被 Key 隔离挡下的）不算上游故障，混进来会把错误率虚高。
	errors, err := s.errorBuckets(ctx, from, to)
	if err != nil {
		return nil, err
	}
	overview.Errors = errors

	percentiles, err := s.latencyPercentiles(ctx, from, to)
	if err != nil {
		return nil, err
	}
	overview.Percentiles = percentiles

	return overview, nil
}

// errorBuckets 统计窗口内的错误分布（按 error_detail 的类型前缀分组）。
//
// 错误类型来自网关的 `Kind`，存在 `error_detail` 的 `[Kind] message` 形式里。
// 用 `split_part` 取出方括号里的类型：这样不必新增一列，
// 也不必在 SQL 里维护一张"类型 → 中文"的映射表（那是前端的事）。
func (s *Store) errorBuckets(ctx context.Context, from, to time.Time) ([]ErrorBucket, error) {
	const query = `
		SELECT kind, count(*) AS count, min(sample) AS sample
		FROM (
		    SELECT COALESCE(NULLIF(split_part(split_part(error_detail, ']', 1), '[', 2), ''), 'unknown') AS kind,
		           left(regexp_replace(COALESCE(error_detail, ''), '\[[^\]]*\]\s*', ''), 160) AS sample
		    FROM usage_logs
		    WHERE ts >= $1 AND ts <= $2
		      AND NOT is_rejection
		      AND status <> 'completed'
		      AND error_detail IS NOT NULL
		) t
		GROUP BY kind
		ORDER BY count DESC
		LIMIT 8`

	rows, err := s.pool.Query(ctx, query, from, to)
	if err != nil {
		return nil, fmt.Errorf("查询错误分布失败: %w", err)
	}
	defer rows.Close()

	list := make([]ErrorBucket, 0, 8)
	for rows.Next() {
		var item ErrorBucket
		if err := rows.Scan(&item.Kind, &item.Count, &item.Sample); err != nil {
			return nil, fmt.Errorf("扫描错误分布失败: %w", err)
		}
		list = append(list, item)
	}

	return list, rows.Err()
}

// latencyPercentiles 算延迟分位。
//
// 用 `percentile_cont` 而不是自己排序：PG 的窗口实现是 C 层，
// 十万行也比把数据捞到 Go 里排一遍快得多，而且不用考虑内存。
func (s *Store) latencyPercentiles(ctx context.Context, from, to time.Time) (LatencyPercentiles, error) {
	const query = `
		SELECT count(*),
		       percentile_cont(0.5) WITHIN GROUP (ORDER BY ttft_ms),
		       percentile_cont(0.95) WITHIN GROUP (ORDER BY ttft_ms),
		       percentile_cont(0.99) WITHIN GROUP (ORDER BY ttft_ms),
		       percentile_cont(0.5) WITHIN GROUP (ORDER BY total_ms),
		       percentile_cont(0.95) WITHIN GROUP (ORDER BY total_ms),
		       percentile_cont(0.99) WITHIN GROUP (ORDER BY total_ms)
		FROM usage_logs
		WHERE ts >= $1 AND ts <= $2
		  AND NOT is_rejection
		  AND status = 'completed'
		  AND total_ms > 0`

	var out LatencyPercentiles
	var p50t, p95t, p99t, p50l, p95l, p99l *float64
	if err := s.pool.QueryRow(ctx, query, from, to).Scan(
		&out.Sample, &p50t, &p95t, &p99t, &p50l, &p95l, &p99l,
	); err != nil {
		return out, fmt.Errorf("查询延迟分位失败: %w", err)
	}

	round := func(value *float64) *int {
		if value == nil {
			return nil
		}
		result := int(math.Round(*value))
		return &result
	}
	out.TtftP50 = round(p50t)
	out.TtftP95 = round(p95t)
	out.TtftP99 = round(p99t)
	out.TotalP50 = round(p50l)
	out.TotalP95 = round(p95l)
	out.TotalP99 = round(p99l)

	return out, nil
}

// computeRates 计算今日吞吐速率。
//
// 分母用"零点到现在"的秒数，最少按 60 秒算：刚过零点时如果只过了 5 秒，
// 一个请求就能算出 12 RPM，那个数字毫无意义。
func computeRates(
	todayStart, to time.Time,
	today *DashboardTotals,
	todaySeries []SummarySeriesPoint,
) RateMetrics {
	elapsed := int64(to.Sub(todayStart).Seconds())
	if elapsed < 60 {
		elapsed = 60
	}

	minutes := float64(elapsed) / 60
	rates := RateMetrics{
		RPM:            float64(today.Requests) / minutes,
		TPM:            float64(today.TotalTokens) / minutes,
		AvgTtftMs:      today.AvgTtftMs,
		AvgLatencyMs:   today.AvgLatencyMs,
		ElapsedSeconds: elapsed,
	}

	// 峰值：取今日单小时最高请求数，折算成每分钟。
	var peak int64
	for _, point := range todaySeries {
		if point.Requests > peak {
			peak = point.Requests
		}
	}
	rates.PeakRPM = float64(peak) / 60

	return rates
}

// totalsBetween 统计区间内的累计指标。`from` 为零值时表示不限下限。
func (s *Store) totalsBetween(ctx context.Context, from, to time.Time) (*DashboardTotals, error) {
	where := "WHERE NOT is_rejection"
	args := make([]any, 0, 2)
	if !from.IsZero() {
		args = append(args, from)
		where += fmt.Sprintf(" AND ts >= $%d", len(args))
	}
	args = append(args, to)
	where += fmt.Sprintf(" AND ts <= $%d", len(args))

	var totals DashboardTotals
	var avgTtft, avgLatency *float64

	err := s.pool.QueryRow(ctx, fmt.Sprintf(`
		SELECT count(*),
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END), 0)::float8,
		       COALESCE(sum(prompt_tokens), 0),
		       COALESCE(sum(completion_tokens), 0),
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(cached_tokens), 0),
		       COALESCE(sum(upstream_cost), 0)::float8,
		       COALESCE(sum(billed_cost), 0)::float8,
		       avg(ttft_ms)::float8,
		       avg(total_ms)::float8
		FROM usage_logs %s`, where), args...,
	).Scan(
		&totals.Requests, &totals.SuccessRate,
		&totals.PromptTokens, &totals.CompletionTokens, &totals.TotalTokens,
		&totals.CachedTokens, &totals.UpstreamCost, &totals.BilledCost,
		&avgTtft, &avgLatency,
	)
	if err != nil {
		return nil, fmt.Errorf("统计总览失败: %w", err)
	}

	totals.AvgTtftMs = roundToInt(avgTtft)
	totals.AvgLatencyMs = roundToInt(avgLatency)

	return &totals, nil
}

// bucketsBetween 按列聚合。列名来自内部常量，不拼接外部输入。
func (s *Store) bucketsBetween(
	ctx context.Context,
	column string,
	from, to time.Time,
) ([]SummaryBucket, error) {
	where := "WHERE NOT is_rejection"
	args := make([]any, 0, 2)
	if !from.IsZero() {
		args = append(args, from)
		where += fmt.Sprintf(" AND ts >= $%d", len(args))
	}
	args = append(args, to)
	where += fmt.Sprintf(" AND ts <= $%d", len(args))

	query := fmt.Sprintf(`
		SELECT COALESCE(%s, 'unknown') AS key,
		       count(*),
		       COALESCE(sum(total_tokens), 0),
		       COALESCE(sum(upstream_cost), 0)::float8,
		       COALESCE(sum(billed_cost), 0)::float8,
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END), 0)::float8
		FROM usage_logs %s
		GROUP BY 1
		ORDER BY 2 DESC
		LIMIT 20`, column, where)

	rows, err := s.pool.Query(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("分组统计失败: %w", err)
	}
	defer rows.Close()

	buckets := make([]SummaryBucket, 0, 8)
	for rows.Next() {
		var bucket SummaryBucket
		var key any
		if err := rows.Scan(
			&key, &bucket.Requests, &bucket.TotalTokens,
			&bucket.UpstreamCost, &bucket.BilledCost, &bucket.SuccessRate,
		); err != nil {
			return nil, fmt.Errorf("扫描分组失败: %w", err)
		}
		bucket.Key = formatBucketKey(key, column)
		bucket.Label = bucket.Key
		buckets = append(buckets, bucket)
	}

	return buckets, rows.Err()
}

// protocolBuckets 按协议拆分（chat / anthropic / responses）。
func (s *Store) protocolBuckets(
	ctx context.Context,
	from, to time.Time,
) ([]ProtocolBucket, error) {
	where := "WHERE NOT is_rejection"
	args := make([]any, 0, 2)
	if !from.IsZero() {
		args = append(args, from)
		where += fmt.Sprintf(" AND ts >= $%d", len(args))
	}
	args = append(args, to)
	where += fmt.Sprintf(" AND ts <= $%d", len(args))

	rows, err := s.pool.Query(ctx, fmt.Sprintf(`
		SELECT protocol, count(*), COALESCE(sum(total_tokens), 0)
		FROM usage_logs %s
		GROUP BY 1
		ORDER BY 2 DESC`, where), args...)
	if err != nil {
		return nil, fmt.Errorf("协议分组失败: %w", err)
	}
	defer rows.Close()

	buckets := make([]ProtocolBucket, 0, 3)
	for rows.Next() {
		var bucket ProtocolBucket
		if err := rows.Scan(&bucket.Protocol, &bucket.Requests, &bucket.Tokens); err != nil {
			return nil, fmt.Errorf("扫描协议分组失败: %w", err)
		}
		buckets = append(buckets, bucket)
	}

	return buckets, rows.Err()
}

// AccountUsage 是**单个账号**的用量汇总（今日 / 累计）。
//
// 为什么要单独一个类型：账号表要能一眼看到"这个号今天用了多少、还能撑多久"。
// 只显示状态（正常/冷却）回答不了这个问题。sub2api 的 `today_stats` /
// `capacity` 两列就是干这个的。
type AccountUsage struct {
	ChannelID string `json:"channelId"`
	UID       string `json:"uid"`
	// 今日
	TodayRequests    int64   `json:"todayRequests"`
	TodayTokens      int64   `json:"todayTokens"`
	TodayCost        float64 `json:"todayCost"`
	TodaySuccessRate float64 `json:"todaySuccessRate"`
	// 累计
	TotalRequests int64   `json:"totalRequests"`
	TotalTokens   int64   `json:"totalTokens"`
	TotalCost     float64 `json:"totalCost"`
	// 最近一次使用时间（RFC3339，空 = 从未使用）
	LastUsedAt string `json:"lastUsedAt"`
}

// ListAccountUsage 批量取账号用量。
//
// 一次查完所有账号再用 map 合并，而不是每行查一次：
// 账号列表 50 行时后者就是 50 次往返，页面会明显发顿。
func (s *Store) ListAccountUsage(ctx context.Context) (map[string]AccountUsage, error) {
	const query = `
		SELECT channel_id, account_uid,
		       count(*) FILTER (WHERE ts >= date_trunc('day', now()))                        AS today_requests,
		       COALESCE(sum(total_tokens) FILTER (WHERE ts >= date_trunc('day', now())), 0)   AS today_tokens,
		       COALESCE(sum(billed_cost) FILTER (WHERE ts >= date_trunc('day', now())), 0)::float8 AS today_cost,
		       COALESCE(avg(CASE WHEN status = 'completed' THEN 1.0 ELSE 0.0 END)
		                FILTER (WHERE ts >= date_trunc('day', now())), 0)::float8            AS today_success,
		       count(*)                                                                       AS total_requests,
		       COALESCE(sum(total_tokens), 0)                                                 AS total_tokens,
		       COALESCE(sum(billed_cost), 0)::float8                                          AS total_cost,
		       max(ts)                                                                        AS last_used
		FROM usage_logs
		WHERE NOT is_rejection
		GROUP BY channel_id, account_uid`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询账号用量失败: %w", err)
	}
	defer rows.Close()

	result := make(map[string]AccountUsage, 64)
	for rows.Next() {
		var item AccountUsage
		var lastUsed any
		if err := rows.Scan(
			&item.ChannelID, &item.UID,
			&item.TodayRequests, &item.TodayTokens, &item.TodayCost, &item.TodaySuccessRate,
			&item.TotalRequests, &item.TotalTokens, &item.TotalCost,
			&lastUsed,
		); err != nil {
			return nil, fmt.Errorf("扫描账号用量失败: %w", err)
		}
		if lastUsed != nil {
			item.LastUsedAt = formatTime(lastUsed)
		}
		result[item.ChannelID+"/"+item.UID] = item
	}

	return result, rows.Err()
}

// startOfLocalDay 返回本地时区的当天零点。
//
// 不用 PG 的 `date_trunc('day', now())`：那用的是数据库会话时区，
// 与应用时区未必一致，跨时区部署时会差几小时，"今日"就对不上了。
func startOfLocalDay(at time.Time) time.Time {
	local := at.In(time.Local)
	return time.Date(local.Year(), local.Month(), local.Day(), 0, 0, 0, 0, time.Local)
}
