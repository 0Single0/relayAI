package store

import (
	"context"
	"fmt"
	"time"
)

// OpsMetrics 是运维页的性能指标（按时间窗口聚合）。
//
// 与仪表盘的区别：仪表盘看"今天用了多少"，这里看"现在跑得怎么样"。
// 所以口径是**短窗口 + 分位 + 逐时趋势**，而不是累计值。
type OpsMetrics struct {
	// Window 是统计窗口（分钟）。
	WindowMinutes int `json:"windowMinutes"`
	From          string `json:"from"`
	To            string `json:"to"`

	// ---- 延迟 ----
	// TTFT 是首字延迟（Time To First Token）。用户感受到的"卡不卡"主要是它。
	Ttft OpsPercentiles `json:"ttft"`
	// Total 是端到端耗时。
	Total OpsPercentiles `json:"total"`

	// ---- 吞吐 ----
	// OTPS 是输出速度（output tokens per second），从首字之后算。
	Otps OpsPercentiles `json:"otps"`

	// ---- 计数 ----
	Requests   int64 `json:"requests"`
	Completed  int64 `json:"completed"`
	Failed     int64 `json:"failed"`
	Rejections int64 `json:"rejections"`
	// Retried 是发生过重试的请求数（attempts > 1）。
	Retried int64 `json:"retried"`

	// ---- 用量 ----
	PromptTokens     int64 `json:"promptTokens"`
	CompletionTokens int64 `json:"completionTokens"`
	CachedTokens     int64 `json:"cachedTokens"`

	// SpeedSample 是参与 OTPS 统计的样本数（门槛过滤后的）。
	// 单独给：OTPS 的样本通常远少于请求数，读者要知道它的可信度。
	SpeedSample int64 `json:"speedSample"`

	// Trend 是逐时趋势（用于画曲线）。
	Trend []OpsTrendPoint `json:"trend"`
}

// OpsPercentiles 是一组分位（毫秒或 token/s，取决于字段用途）。
type OpsPercentiles struct {
	P50    *float64 `json:"p50"`
	P95    *float64 `json:"p95"`
	P99    *float64 `json:"p99"`
	Avg    *float64 `json:"avg"`
	Sample int64    `json:"sample"`
}

// OpsTrendPoint 是一个时间桶的聚合。
type OpsTrendPoint struct {
	// Label 是本地时间的 `MM-DD HH:00`。
	Label      string   `json:"label"`
	Requests   int64    `json:"requests"`
	Completed  int64    `json:"completed"`
	Failed     int64    `json:"failed"`
	Retried    int64    `json:"retried"`
	TtftP50    *float64 `json:"ttftP50"`
	TtftP95    *float64 `json:"ttftP95"`
	TtftP99    *float64 `json:"ttftP99"`
	TotalP50   *float64 `json:"totalP50"`
	TotalP95   *float64 `json:"totalP95"`
	OtpsP50    *float64 `json:"otpsP50"`
	OtpsP95    *float64 `json:"otpsP95"`
	// 逐桶 token 用量：用来画吞吐曲线（输入/输出分开）。
	PromptTokens     int64 `json:"promptTokens"`
	CompletionTokens int64 `json:"completionTokens"`
	CachedTokens     int64 `json:"cachedTokens"`
}

// GetOpsMetrics 取运维性能指标。
//
// `windowMinutes` 决定窗口；`bucketMinutes` 决定趋势粒度。
// 两个都做钳制：窗口太短没有样本，太长看不出"现在"；粒度太细会画出锯齿。
func (s *Store) GetOpsMetrics(
	ctx context.Context,
	windowMinutes, bucketMinutes int,
) (*OpsMetrics, error) {
	if windowMinutes < 15 {
		windowMinutes = 15
	}
	if windowMinutes > 24*60 {
		windowMinutes = 24 * 60
	}
	if bucketMinutes < 1 {
		bucketMinutes = 1
	}
	if bucketMinutes > 60 {
		bucketMinutes = 60
	}

	to := time.Now()
	from := to.Add(-time.Duration(windowMinutes) * time.Minute)

	out := &OpsMetrics{
		WindowMinutes: windowMinutes,
		From:          from.Format(time.RFC3339),
		To:            to.Format(time.RFC3339),
	}

	// ---- 计数与用量 ----
	const countQuery = `
		SELECT
		    count(*)                                                                   AS requests,
		    count(*) FILTER (WHERE status = 'completed' AND NOT is_rejection)           AS completed,
		    count(*) FILTER (WHERE status <> 'completed' AND NOT is_rejection)          AS failed,
		    count(*) FILTER (WHERE is_rejection)                                        AS rejections,
		    count(*) FILTER (WHERE attempts > 1)                                        AS retried,
		    COALESCE(sum(prompt_tokens), 0)                                             AS prompt_tokens,
		    COALESCE(sum(completion_tokens), 0)                                         AS completion_tokens,
		    COALESCE(sum(cached_tokens), 0)                                             AS cached_tokens
		FROM usage_logs
		WHERE ts >= $1 AND ts <= $2`

	if err := s.pool.QueryRow(ctx, countQuery, from, to).Scan(
		&out.Requests, &out.Completed, &out.Failed, &out.Rejections, &out.Retried,
		&out.PromptTokens, &out.CompletionTokens, &out.CachedTokens,
	); err != nil {
		return nil, fmt.Errorf("查询运维计数失败: %w", err)
	}

	// ---- TTFT 分位 ----
	ttft, err := s.percentileSet(ctx, from, to, "ttft_ms")
	if err != nil {
		return nil, err
	}
	out.Ttft = ttft

	// ---- 总耗时分位 ----
	total, err := s.percentileSet(ctx, from, to, "total_ms")
	if err != nil {
		return nil, err
	}
	out.Total = total

	// ---- OTPS 分位 ----
	//
	// 口径与使用记录页的速度列**完全一致**，否则同一份数据在两张页面上
	// 给出不同的数字，用户会不知道信哪个。
	//   有 ttft：completion / ((total - ttft) / 1000)，要求 completion >= 100
	//   无 ttft：completion / (total / 1000)，要求 completion >= 200
	// 拒绝行与零时长排除。
	const otpsQuery = `
		WITH speed AS (
		    SELECT
		        CASE
		            WHEN ttft_ms IS NOT NULL AND ttft_ms > 0 AND total_ms > ttft_ms
		                THEN completion_tokens::float8 / ((total_ms - ttft_ms)::float8 / 1000)
		            WHEN ttft_ms IS NULL AND total_ms > 0
		                THEN completion_tokens::float8 / (total_ms::float8 / 1000)
		            ELSE NULL
		        END AS value
		    FROM usage_logs
		    WHERE ts >= $1 AND ts <= $2
		      AND NOT is_rejection
		      AND completion_tokens > 0
		      AND total_ms > 0
		      AND (
		          (ttft_ms IS NOT NULL AND ttft_ms > 0 AND total_ms > ttft_ms AND completion_tokens >= 100)
		          OR
		          (ttft_ms IS NULL AND completion_tokens >= 200)
		      )
		)
		SELECT count(*),
		       percentile_cont(0.5) WITHIN GROUP (ORDER BY value),
		       percentile_cont(0.95) WITHIN GROUP (ORDER BY value),
		       percentile_cont(0.99) WITHIN GROUP (ORDER BY value),
		       avg(value)
		FROM speed
		WHERE value IS NOT NULL`

	var sample int64
	var p50, p95, p99, avg *float64
	if err := s.pool.QueryRow(ctx, otpsQuery, from, to).Scan(
		&sample, &p50, &p95, &p99, &avg,
	); err != nil {
		return nil, fmt.Errorf("查询 OTPS 失败: %w", err)
	}
	out.SpeedSample = sample
	out.Otps = OpsPercentiles{P50: p50, P95: p95, P99: p99, Avg: avg, Sample: sample}

	// ---- 逐时趋势 ----
	trend, err := s.opsTrend(ctx, from, to, bucketMinutes)
	if err != nil {
		return nil, err
	}
	out.Trend = trend

	return out, nil
}

// percentileSet 算某一列的分位（P50/P95/P99 + 平均）。
//
// 只统计**成功**请求：失败的请求没有完整的 ttft/total 语义
// （比如 429 在拿到首字前就结束了），混进来会把延迟画成"很健康"。
func (s *Store) percentileSet(
	ctx context.Context,
	from, to time.Time,
	column string,
) (OpsPercentiles, error) {
	// 列名来自内部常量，不接受外部输入，所以可以拼进 SQL。
	query := fmt.Sprintf(`
		SELECT count(*),
		       percentile_cont(0.5) WITHIN GROUP (ORDER BY %[1]s),
		       percentile_cont(0.95) WITHIN GROUP (ORDER BY %[1]s),
		       percentile_cont(0.99) WITHIN GROUP (ORDER BY %[1]s),
		       avg(%[1]s)
		FROM usage_logs
		WHERE ts >= $1 AND ts <= $2
		  AND NOT is_rejection
		  AND status = 'completed'
		  AND %[1]s IS NOT NULL
		  AND %[1]s > 0`, column)

	var out OpsPercentiles
	if err := s.pool.QueryRow(ctx, query, from, to).Scan(
		&out.Sample, &out.P50, &out.P95, &out.P99, &out.Avg,
	); err != nil {
		return out, fmt.Errorf("查询 %s 分位失败: %w", column, err)
	}
	return out, nil
}

// opsTrend 按时间桶聚合趋势。
func (s *Store) opsTrend(
	ctx context.Context,
	from, to time.Time,
	bucketMinutes int,
) ([]OpsTrendPoint, error) {
	const query = `
		SELECT
		    to_char(date_bin, 'MM-DD HH24:MI') AS label,
		    count(*)                                        AS requests,
		    count(*) FILTER (WHERE status = 'completed')     AS completed,
		    count(*) FILTER (WHERE status <> 'completed')    AS failed,
		    count(*) FILTER (WHERE attempts > 1)             AS retried,
		    percentile_cont(0.5)  WITHIN GROUP (ORDER BY ttft_ms) FILTER (WHERE ttft_ms IS NOT NULL AND ttft_ms > 0),
		    percentile_cont(0.95) WITHIN GROUP (ORDER BY ttft_ms) FILTER (WHERE ttft_ms IS NOT NULL AND ttft_ms > 0),
		    percentile_cont(0.99) WITHIN GROUP (ORDER BY ttft_ms) FILTER (WHERE ttft_ms IS NOT NULL AND ttft_ms > 0),
		    percentile_cont(0.5)  WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE total_ms > 0),
		    percentile_cont(0.95) WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE total_ms > 0),
		    -- OTPS 的 P50/P95：与汇总口径一致，只取达标样本
		    percentile_cont(0.5) WITHIN GROUP (ORDER BY
		        CASE
		            WHEN ttft_ms IS NOT NULL AND ttft_ms > 0 AND total_ms > ttft_ms AND completion_tokens >= 100
		                THEN completion_tokens::float8 / ((total_ms - ttft_ms)::float8 / 1000)
		            WHEN ttft_ms IS NULL AND total_ms > 0 AND completion_tokens >= 200
		                THEN completion_tokens::float8 / (total_ms::float8 / 1000)
		        END
		    ) FILTER (WHERE completion_tokens > 0),
		    percentile_cont(0.95) WITHIN GROUP (ORDER BY
		        CASE
		            WHEN ttft_ms IS NOT NULL AND ttft_ms > 0 AND total_ms > ttft_ms AND completion_tokens >= 100
		                THEN completion_tokens::float8 / ((total_ms - ttft_ms)::float8 / 1000)
		            WHEN ttft_ms IS NULL AND total_ms > 0 AND completion_tokens >= 200
		                THEN completion_tokens::float8 / (total_ms::float8 / 1000)
		        END
		    ) FILTER (WHERE completion_tokens > 0),
		    COALESCE(sum(prompt_tokens), 0),
		    COALESCE(sum(completion_tokens), 0),
		    COALESCE(sum(cached_tokens), 0)
		FROM (
		    SELECT *, to_timestamp(
		        floor(extract(epoch FROM ts) / ($3::int * 60)) * ($3::int * 60)
		    ) AS date_bin
		    FROM usage_logs
		    WHERE ts >= $1 AND ts <= $2 AND NOT is_rejection
		) t
		GROUP BY date_bin
		ORDER BY date_bin`

	rows, err := s.pool.Query(ctx, query, from, to, bucketMinutes)
	if err != nil {
		return nil, fmt.Errorf("查询运维趋势失败: %w", err)
	}
	defer rows.Close()

	list := make([]OpsTrendPoint, 0, 32)
	for rows.Next() {
		var item OpsTrendPoint
		if err := rows.Scan(
			&item.Label, &item.Requests, &item.Completed, &item.Failed, &item.Retried,
			&item.TtftP50, &item.TtftP95, &item.TtftP99,
			&item.TotalP50, &item.TotalP95,
			&item.OtpsP50, &item.OtpsP95,
			&item.PromptTokens, &item.CompletionTokens, &item.CachedTokens,
		); err != nil {
			return nil, fmt.Errorf("扫描运维趋势失败: %w", err)
		}
		list = append(list, item)
	}

	return list, rows.Err()
}
