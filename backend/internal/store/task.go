package store

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
)

// TaskChannel 是任务菜单的一项。
type TaskChannel struct {
	ID              string      `json:"id"`
	Name            string      `json:"name"`
	Adapter         string      `json:"adapter"`
	Caps            ChannelCaps `json:"caps"`
	AccountCount    int         `json:"accountCount"`
	PendingAccounts int         `json:"pendingAccounts"`
	// Realm 是 WorkBuddy 的版本（cn / intl）。
	//
	// 为什么要透出：国内版有成长任务 + 签到 + 猫猫旅行，国际版只有每日活跃打卡。
	// 菜单上不标出来，用户看到两个都叫 "workbuddy" 的分组不知道点哪个。
	Realm string `json:"realm"`
}

// ListTaskChannels 返回有运营能力的渠道（SPEC 8.3 的数据源）。
//
// 只返回能力至少有一项为 true 的渠道：全都 false 的渠道点进去是空白页。
func (s *Store) ListTaskChannels(ctx context.Context) ([]TaskChannel, error) {
	const query = `
		SELECT c.id, c.name, c.adapter,
		       COALESCE(c.options->>'realm', 'cn') AS realm,
		       COALESCE(cap.tasks, false), COALESCE(cap.daily, false), COALESCE(cap.checkin, false),
		       COALESCE(a.cnt, 0) AS account_count
		FROM channels c
		LEFT JOIN channel_caps cap ON cap.channel_id = c.id
		LEFT JOIN (
			SELECT channel_id, count(*) AS cnt FROM accounts WHERE enabled GROUP BY channel_id
		) a ON a.channel_id = c.id
		WHERE c.enabled
		  AND (COALESCE(cap.tasks, false) OR COALESCE(cap.daily, false) OR COALESCE(cap.checkin, false))
		ORDER BY c.priority DESC, c.id`

	rows, err := s.db.WithContext(ctx).Raw(query).Rows()
	if err != nil {
		return nil, fmt.Errorf("查询任务渠道失败: %w", err)
	}
	defer rows.Close()

	list := make([]TaskChannel, 0, 4)
	for rows.Next() {
		var channel TaskChannel
		if err := rows.Scan(
			&channel.ID, &channel.Name, &channel.Adapter, &channel.Realm,
			&channel.Caps.Tasks, &channel.Caps.Daily, &channel.Caps.Checkin,
			&channel.AccountCount,
		); err != nil {
			return nil, fmt.Errorf("扫描任务渠道失败: %w", err)
		}
		// 待办账号数需要实时快照才知道，这里给 0；界面在进入渠道页后才有准确值。
		channel.PendingAccounts = 0
		list = append(list, channel)
	}

	return list, rows.Err()
}

// TaskRun 是一次任务执行。
type TaskRun struct {
	ID              int64   `json:"id"`
	BatchID         *string `json:"batchId"`
	ChannelID       string  `json:"channelId"`
	ChannelName     string  `json:"channelName"`
	AccountUID      string  `json:"accountUid"`
	AccountNickname string  `json:"accountNickname"`
	Scope           string  `json:"scope"`
	Status          string  `json:"status"`
	Trigger         string  `json:"trigger"`
	Total           int     `json:"total"`
	DoneCount       int     `json:"doneCount"`
	FailedCount     int     `json:"failedCount"`
	SkippedCount    int     `json:"skippedCount"`
	Earned          int     `json:"earned"`
	Error           *string `json:"error"`
	StartedAt       string  `json:"startedAt"`
	FinishedAt      *string `json:"finishedAt"`
}

// TaskRunItem 是一条执行结果。
type TaskRunItem struct {
	ID         int64  `json:"id"`
	RunID      int64  `json:"runId"`
	Code       string `json:"code"`
	Name       string `json:"name"`
	Kind       string `json:"kind"`
	Status     string `json:"status"`
	Message    string `json:"message"`
	Earned     int    `json:"earned"`
	FinishedAt string `json:"finishedAt"`
}

// TaskAccountRef 是一次执行里的目标账号。
//
// 为什么要带 channelId：账号可以**不属于任何分组**（`channel_id IS NULL`），
// 而 `task_runs.channel_id` 非空。只传 uid 的话，未分配分组的号既写不进
// run 记录，也无法在执行历史里定位。空串表示"未分配"，不是错误。
type TaskAccountRef struct {
	UID       string `json:"uid"`
	ChannelID string `json:"channelId"`
}

// StartTaskRuns 为一批账号创建 run 记录，返回批次 id 与 run 列表。
func (s *Store) StartTaskRuns(
	ctx context.Context,
	refs []TaskAccountRef,
	scope string,
) (string, []map[string]any, error) {
	batchID := newBatchID()
	runs := make([]map[string]any, 0, len(refs))

	for _, ref := range refs {
		const query = `
			INSERT INTO task_runs (batch_id, channel_id, account_uid, scope, status, trigger)
			VALUES ($1, $2, $3, $4, 'running', 'manual')
			RETURNING id`

		var id int64
		if err := s.db.WithContext(ctx).
			Raw(query, batchID, ref.ChannelID, ref.UID, scope).
			Row().Scan(&id); err != nil {
			return "", nil, fmt.Errorf("创建任务记录失败: %w", err)
		}

		runs = append(runs, map[string]any{
			"runId":      id,
			"accountUid": ref.UID,
			"channelId":  ref.ChannelID,
		})
	}

	return batchID, runs, nil
}

// RunningTaskRun 查该账号是否有执行中的 run。
//
// 按 **uid** 定位而不是 (channel_id, uid)：账号换分组后，
// 旧 run 记录的 channel_id 与新分组不同，按分组查会漏掉"其实还在跑"的号。
func (s *Store) RunningTaskRun(ctx context.Context, uid string) (*TaskRun, error) {
	const query = `
		SELECT id, channel_id FROM task_runs
		WHERE account_uid = $1 AND status = 'running'
		ORDER BY started_at DESC LIMIT 1`

	var id int64
	var channelID string
	err := s.db.WithContext(ctx).Raw(query, uid).Row().Scan(&id, &channelID)
	if err != nil {
		return nil, err
	}

	return &TaskRun{ID: id, ChannelID: channelID, AccountUID: uid, Status: "running"}, nil
}

// FailTaskRun 把 run 标成失败（例如"该账号已在执行中"）。
//
// 按 uid 定位最近一条 running：账号换分组后 run 上的 channel_id 是旧的，
// 按分组过滤会把"其实还在跑"的号漏掉。
func (s *Store) FailTaskRun(ctx context.Context, uid, reason string) error {
	_, err := s.exec(ctx,
		`UPDATE task_runs SET status = 'failed', error = $2, finished_at = now()
		 WHERE account_uid = $1 AND status = 'running'`,
		uid, reason,
	)
	return err
}

// CancelTaskRun 请求取消。
//
// 网关负责"当前项结束后停止"（SPEC 11.7）；这里只把状态改成 cancelled，
// 让界面立刻有反馈。已完成的 item 保留（积分已到账，不回滚）。
//
// 返回值的三种情况必须分清，否则用户看到"记录不存在"会去查错方向：
//   - nil：取消成功；
//   - ErrNotFound：这条 run 真的不存在（id 写错）；
//   - ErrNotRunning：run 存在但已经不在执行中（已完成/已取消）。
func (s *Store) CancelTaskRun(ctx context.Context, id int64) error {
	affected, err := s.exec(ctx,
		`UPDATE task_runs SET status = 'cancelled', finished_at = now()
		 WHERE id = $1 AND status = 'running'`, id,
	)
	if err != nil {
		return fmt.Errorf("取消任务失败: %w", err)
	}
	if affected == 0 {
		// 没被更新：要么不存在，要么已经结束了。这两种对用户的意义完全不同。
		var status string
		err := s.db.WithContext(ctx).Raw(`SELECT status FROM task_runs WHERE id = $1`, id).Row().Scan(&status)
		if errors.Is(err, sql.ErrNoRows) {
			return ErrNotFound
		}
		if err != nil {
			return fmt.Errorf("查询任务状态失败: %w", err)
		}
		return fmt.Errorf("%w: 当前状态 %s", ErrNotRunning, status)
	}
	return nil
}

// TaskRunQuery 是执行记录的筛选条件。
type TaskRunQuery struct {
	ChannelID string
	Account   string
	Status    string
	Page      int
	PageSize  int
}

// ListTaskRuns 分页查询执行记录。
func (s *Store) ListTaskRuns(ctx context.Context, query TaskRunQuery) ([]TaskRun, int, error) {
	if query.Page < 1 {
		query.Page = 1
	}
	if query.PageSize < 1 || query.PageSize > 100 {
		query.PageSize = 20
	}

	where := "WHERE 1=1"
	args := make([]any, 0, 4)
	if query.ChannelID != "" {
		args = append(args, query.ChannelID)
		where += fmt.Sprintf(" AND r.channel_id = $%d", len(args))
	}
	if query.Account != "" {
		args = append(args, query.Account)
		where += fmt.Sprintf(" AND r.account_uid = $%d", len(args))
	}
	if query.Status != "" {
		args = append(args, query.Status)
		where += fmt.Sprintf(" AND r.status = $%d", len(args))
	}

	var total int
	if err := s.db.WithContext(ctx).Raw("SELECT count(*) FROM task_runs r "+where, args...).Row().Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计任务记录失败: %w", err)
	}

	listArgs := append(args, query.PageSize, (query.Page-1)*query.PageSize)
	listQuery := fmt.Sprintf(`
		SELECT r.id, r.batch_id, r.channel_id, COALESCE(c.name, r.channel_id),
		       r.account_uid, COALESCE(a.nickname, r.account_uid),
		       r.scope, r.status, r.trigger,
		       r.total, r.done_count, r.failed_count, r.skipped_count, r.earned, r.error,
		       r.started_at, r.finished_at
		FROM task_runs r
		LEFT JOIN channels c ON c.id = r.channel_id
		-- 昵称按 uid 连：号可能已经换过分组，用 channel_id 连会退化成显示 uid。
		LEFT JOIN accounts a ON a.uid = r.account_uid
		%s
		ORDER BY r.started_at DESC
		LIMIT $%d OFFSET $%d`, where, len(listArgs)-1, len(listArgs))

	rows, err := s.db.WithContext(ctx).Raw(listQuery, listArgs...).Rows()
	if err != nil {
		return nil, 0, fmt.Errorf("查询任务记录失败: %w", err)
	}
	defer rows.Close()

	list := make([]TaskRun, 0, query.PageSize)
	for rows.Next() {
		var run TaskRun
		var startedAt any
		var finishedAt any
		if err := rows.Scan(
			&run.ID, &run.BatchID, &run.ChannelID, &run.ChannelName,
			&run.AccountUID, &run.AccountNickname,
			&run.Scope, &run.Status, &run.Trigger,
			&run.Total, &run.DoneCount, &run.FailedCount, &run.SkippedCount, &run.Earned, &run.Error,
			&startedAt, &finishedAt,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描任务记录失败: %w", err)
		}
		run.StartedAt = formatTime(startedAt)
		run.FinishedAt = nullableTime(finishedAt)
		list = append(list, run)
	}

	return list, total, rows.Err()
}

// GetTaskRun 取单次执行与明细。
func (s *Store) GetTaskRun(ctx context.Context, id int64) (*TaskRun, []TaskRunItem, error) {
	const runQuery = `
		SELECT r.id, r.batch_id, r.channel_id, COALESCE(c.name, r.channel_id),
		       r.account_uid, COALESCE(a.nickname, r.account_uid),
		       r.scope, r.status, r.trigger,
		       r.total, r.done_count, r.failed_count, r.skipped_count, r.earned, r.error,
		       r.started_at, r.finished_at
		FROM task_runs r
		LEFT JOIN channels c ON c.id = r.channel_id
		-- 昵称按 uid 连：号可能已经换过分组，用 channel_id 连会退化成显示 uid。
		LEFT JOIN accounts a ON a.uid = r.account_uid
		WHERE r.id = $1`

	var run TaskRun
	var startedAt, finishedAt any
	err := s.db.WithContext(ctx).Raw(runQuery, id).Row().Scan(
		&run.ID, &run.BatchID, &run.ChannelID, &run.ChannelName,
		&run.AccountUID, &run.AccountNickname,
		&run.Scope, &run.Status, &run.Trigger,
		&run.Total, &run.DoneCount, &run.FailedCount, &run.SkippedCount, &run.Earned, &run.Error,
		&startedAt, &finishedAt,
	)
	if err != nil {
		return nil, nil, ErrNotFound
	}
	run.StartedAt = formatTime(startedAt)
	run.FinishedAt = nullableTime(finishedAt)

	const itemQuery = `
		SELECT id, run_id, code, name, kind, status, message, earned, finished_at
		FROM task_run_items WHERE run_id = $1 ORDER BY id`

	rows, err := s.queryRows(ctx, itemQuery, id)
	if err != nil {
		return nil, nil, fmt.Errorf("查询任务明细失败: %w", err)
	}
	defer rows.Close()

	items := make([]TaskRunItem, 0, 16)
	for rows.Next() {
		var item TaskRunItem
		var finishedAt any
		if err := rows.Scan(
			&item.ID, &item.RunID, &item.Code, &item.Name, &item.Kind,
			&item.Status, &item.Message, &item.Earned, &finishedAt,
		); err != nil {
			return nil, nil, fmt.Errorf("扫描任务明细失败: %w", err)
		}
		item.FinishedAt = formatTime(finishedAt)
		items = append(items, item)
	}

	return &run, items, rows.Err()
}

// TaskRunStatusOf 只取 run 的状态与终态时间（SSE 轮询用）。
//
// 单独一个方法而不是复用 GetTaskRun：SSE 每几百毫秒查一次，
// 只要一列就够，别每次都把明细连表读一遍。
func (s *Store) TaskRunStatusOf(ctx context.Context, id int64) (string, error) {
	var status string
	err := s.db.WithContext(ctx).Raw(`SELECT status FROM task_runs WHERE id = $1`, id).Row().Scan(&status)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNotFound
	}
	if err != nil {
		return "", fmt.Errorf("查询任务状态失败: %w", err)
	}
	return status, nil
}

// TaskRunItemsAfter 取 `id > afterID` 的明细，按 id 升序。
//
// 用自增 id 而不是时间戳做游标：同一秒内可能落多条（网关是逐项推送的），
// 时间戳会漏，id 是唯一的。
func (s *Store) TaskRunItemsAfter(ctx context.Context, runID, afterID int64) ([]TaskRunItem, error) {
	const query = `
		SELECT id, run_id, code, name, kind, status, message, earned, finished_at
		FROM task_run_items WHERE run_id = $1 AND id > $2 ORDER BY id`

	rows, err := s.db.WithContext(ctx).Raw(query, runID, afterID).Rows()
	if err != nil {
		return nil, fmt.Errorf("查询任务明细失败: %w", err)
	}
	defer rows.Close()

	items := make([]TaskRunItem, 0, 8)
	for rows.Next() {
		var item TaskRunItem
		var finishedAt any
		if err := rows.Scan(
			&item.ID, &item.RunID, &item.Code, &item.Name, &item.Kind,
			&item.Status, &item.Message, &item.Earned, &finishedAt,
		); err != nil {
			return nil, fmt.Errorf("扫描任务明细失败: %w", err)
		}
		item.FinishedAt = formatTime(finishedAt)
		items = append(items, item)
	}

	return items, rows.Err()
}

// TaskRunProgress 是 SSE 推给前端的进度快照。
//
// 单独一个类型而不是复用 TaskRun：复用会让 `channelId` / `scope` / `trigger`
// 这些没查的字段以空字符串出现在帧里，前端看到 `"batchId":null` 会以为数据坏了。
type TaskRunProgress struct {
	ID           int64   `json:"id"`
	Status       string  `json:"status"`
	Total        int     `json:"total"`
	DoneCount    int     `json:"doneCount"`
	FailedCount  int     `json:"failedCount"`
	SkippedCount int     `json:"skippedCount"`
	Earned       int     `json:"earned"`
	Error        *string `json:"error"`
}

// TaskRunCounters 取 run 的计数（SSE 每次推进都要报最新进度）。
func (s *Store) TaskRunCounters(ctx context.Context, id int64) (*TaskRunProgress, error) {
	const query = `
		SELECT id, status, total, done_count, failed_count, skipped_count, earned, error
		FROM task_runs WHERE id = $1`

	var progress TaskRunProgress
	err := s.db.WithContext(ctx).Raw(query, id).Row().Scan(
		&progress.ID, &progress.Status, &progress.Total, &progress.DoneCount,
		&progress.FailedCount, &progress.SkippedCount, &progress.Earned, &progress.Error,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("查询任务进度失败: %w", err)
	}

	return &progress, nil
}

// newBatchID 生成批次 id。
//
// 必须是**标准 UUID 字符串**：`task_runs.batch_id` 是 PostgreSQL 的 `uuid` 列，
// 早期实现拼的是"时间戳-随机十六进制"（如 `1792637561118199000-460a8e278cbf1b86`），
// 长度和连字符位置都不符合 UUID 格式，插入直接报
// `invalid input syntax for type uuid`，整个「发起执行」接口 500。
//
// 这里按 RFC 4122 v4 生成：16 字节 crypto/rand，设置版本位（第 7 字节高 4 位 = 4）
// 与变体位（第 9 字节高 2 位 = 10），再按 8-4-4-4-12 分组输出小写十六进制。
// 随机源必须用 crypto/rand：批次 id 会出现在接口响应和日志里，不能可预测。
func newBatchID() string {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		// crypto/rand 失败在实践里只可能是系统调用不可用。
		// 这里**不降级成可预测的伪随机**：退化成时间戳会让批次 id 冲突，
		// 而冲突的表现是两次执行被并成一批，比直接报错更难查。
		panic(fmt.Sprintf("生成批次 id 失败: %v", err))
	}

	raw[6] = (raw[6] & 0x0f) | 0x40 // version 4
	raw[8] = (raw[8] & 0x3f) | 0x80 // variant 10

	return fmt.Sprintf("%x-%x-%x-%x-%x", raw[0:4], raw[4:6], raw[6:8], raw[8:10], raw[10:16])
}

// ensure json is used (Capabilities uses json.RawMessage).
var _ = json.RawMessage{}
