// Package gateway 是 Rust 网关内部接口的客户端。
//
// 管理面里有一批操作必须贴着适配器才能做（登录、刷新、探活、额度、任务），
// 这些走本包；其余 CRUD 直连 PG，不经过网关。
package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"relayai/backend/internal/config"
)

// Client 是内部接口客户端。
type Client struct {
	baseURL string
	// publicBaseURL 是**对外公布**的网关地址，给客户端填 base URL 用。
	//
	// 与 `baseURL` 必须分开：那个是后端到网关的内部地址（通常是 127.0.0.1），
	// 用户在自己的电脑上填 127.0.0.1 是打不到这台机器的。
	// 部署时用 `GATEWAY_BASE_URL` 指到真实域名；本地开发两者可以相同。
	publicBaseURL string
	token         string
	http          *http.Client
}

// New 创建客户端。baseURL 形如 http://127.0.0.1:18788。
func New(conf config.GatewayConf) *Client {
	timeout := time.Duration(conf.TimeoutMs) * time.Millisecond
	if timeout <= 0 {
		timeout = 15 * time.Second
	}

	public := conf.PublicBaseURL
	if public == "" {
		public = conf.BaseURL
	}

	return &Client{
		baseURL:       conf.BaseURL,
		publicBaseURL: strings.TrimRight(public, "/"),
		token:         conf.InternalToken,
		http:          &http.Client{Timeout: timeout},
	}
}

// PublicBaseURL 返回对外公布的网关根地址（不带结尾斜杠）。
//
// 给前端拼客户端接入信息用。**不要用 `window.location.origin` 代替**：
// 管理台在 5173，网关在 18788，两者不是同一个进程。
func (c *Client) PublicBaseURL() string {
	return c.publicBaseURL
}

// APIError 是网关返回的错误。
type APIError struct {
	Status int
	Kind   string
	Msg    string
}

func (e *APIError) Error() string {
	if e.Kind != "" {
		return fmt.Sprintf("%s（%s）", e.Msg, e.Kind)
	}
	return e.Msg
}

// get 发起 GET 并解析 JSON。
func (c *Client) get(ctx context.Context, path string, out any) error {
	return c.do(ctx, http.MethodGet, path, nil, out)
}

// post 发起 POST 并解析 JSON。
func (c *Client) post(ctx context.Context, path string, body any, out any) error {
	return c.do(ctx, http.MethodPost, path, body, out)
}

func (c *Client) do(ctx context.Context, method, path string, body any, out any) error {
	var reader io.Reader
	if body != nil {
		payload, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("序列化请求体失败: %w", err)
		}
		reader = bytes.NewReader(payload)
	}

	request, err := http.NewRequestWithContext(ctx, method, c.baseURL+path, reader)
	if err != nil {
		return fmt.Errorf("构造请求失败: %w", err)
	}

	request.Header.Set("X-Internal-Token", c.token)
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}

	response, err := c.http.Do(request)
	if err != nil {
		return fmt.Errorf("调用网关失败: %w", err)
	}
	defer response.Body.Close()

	payload, err := io.ReadAll(io.LimitReader(response.Body, 4<<20))
	if err != nil {
		return fmt.Errorf("读取网关响应失败: %w", err)
	}

	if response.StatusCode >= 400 {
		var envelope struct {
			Error string `json:"error"`
			Kind  string `json:"kind"`
		}
		_ = json.Unmarshal(payload, &envelope)

		msg := envelope.Error
		if msg == "" {
			msg = fmt.Sprintf("网关返回 HTTP %d", response.StatusCode)
		}
		return &APIError{Status: response.StatusCode, Kind: envelope.Kind, Msg: msg}
	}

	if out == nil {
		return nil
	}

	if err := json.Unmarshal(payload, out); err != nil {
		return fmt.Errorf("解析网关响应失败: %w", err)
	}

	return nil
}

// AccountKey 把 channelId/uid 编码成内部接口用的路径段。
//
// uid 里可能出现斜杠，所以整段做 URL 编码，网关侧再解回 channel 与 uid。
func AccountKey(channelID, uid string) string {
	return url.PathEscape(channelID + "/" + uid)
}

// ---- 状态 ----

// Status 是网关运行状态。
type Status struct {
	Running     bool   `json:"running"`
	Addr        string `json:"addr"`
	Error       string `json:"error"`
	Version     string `json:"version"`
	UptimeMs    int64  `json:"uptimeMs"`
	Auth        string `json:"auth"`
	Concurrency struct {
		Global    int `json:"global"`
		Available int `json:"available"`
	} `json:"concurrency"`
	Matrix struct {
		Routes     int `json:"routes"`
		Live       int `json:"live"`
		Builtin    int `json:"builtin"`
		AgeMs      int64 `json:"ageMs"`
		TTLSeconds int `json:"ttlSeconds"`
	} `json:"matrix"`
	Stats struct {
		Requests int64 `json:"requests"`
		Failures int64 `json:"failures"`
		Retries  int64 `json:"retries"`
	} `json:"stats"`
	Channels []struct {
		ID       string `json:"id"`
		Adapter  string `json:"adapter"`
		Accounts int    `json:"accounts"`
		Ready    int    `json:"ready"`
	} `json:"channels"`
	// Accounts 是账号级运行态（在飞、并发上限、是否可调度）。
	//
	// 必须由网关给：`max_inflight` 在库里可以是 NULL（"用渠道默认"），
	// 而"默认"具体是多少只有网关知道。让前端自己去猜 NULL = 无限，
	// 就会显示成 `0/∞` —— 那是在说"这个账号不限并发"，与真实行为相反。
	Accounts []AccountRuntime `json:"accounts"`
	Warnings []string         `json:"warnings"`
}

// AccountRuntime 是一个账号的实时运行态。
type AccountRuntime struct {
	ChannelID string `json:"channelId"`
	UID       string `json:"uid"`
	// Platform 是该账号的平台（workbuddy / trae / custom / fake）。
	Platform string `json:"platform"`
	Enabled  bool   `json:"enabled"`
	Inflight int    `json:"inflight"`
	// MaxInflight 是**生效值**（已把 NULL 解析成渠道默认），不会是 0。
	MaxInflight        int      `json:"maxInflight"`
	Ready              bool     `json:"ready"`
	BlockedReasons     []string `json:"blockedReasons"`
	CooldownUntilMs    int64    `json:"cooldownUntilMs"`
	RequiresCredential bool     `json:"requiresCredential"`
	HasCredential      bool     `json:"hasCredential"`
	// StaticCredential 表示该账号是静态凭据（自定义供应商的 API Key）。
	// 静态凭据没有"刷新"这回事，前端据此禁用刷新按钮。
	StaticCredential bool `json:"staticCredential"`
	// Quota 是额度快照（网关内存里那份，不打上游）。
	Quota *AccountQuota `json:"quota"`
}

// AccountQuota 是账号的额度快照。
type AccountQuota struct {
	Label       string   `json:"label"`
	Unit        string   `json:"unit"`
	Remaining   *float64 `json:"remaining"`
	Total       *float64 `json:"total"`
	Used        *float64 `json:"used"`
	FetchedAtMs int64    `json:"fetchedAtMs"`
}

// GetStatus 读网关状态。
func (c *Client) GetStatus(ctx context.Context) (*Status, error) {
	var out Status
	if err := c.get(ctx, "/internal/status", &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Decision 是一条调度决策。
type Decision struct {
	Trace         string          `json:"trace"`
	Ts            int64           `json:"ts"`
	Model         string          `json:"model"`
	Stream        bool            `json:"stream"`
	Session       string          `json:"session"`
	Candidates    int             `json:"candidates"`
	Rejected      []string        `json:"rejected"`
	Attempts      int             `json:"attempts"`
	ViaAffinity   bool            `json:"via_affinity"`
	Channel       string          `json:"channel"`
	Account       string          `json:"account"`
	UpstreamModel string          `json:"upstream_model"`
	Error         json.RawMessage `json:"error"`
}

// GetDecisions 读最近的调度决策。
func (c *Client) GetDecisions(ctx context.Context, limit int) ([]Decision, error) {
	var out []Decision
	path := fmt.Sprintf("/internal/decisions?limit=%d", limit)
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// ---- 渠道 ----

// OpsCaps 是渠道的运营能力。
type OpsCaps struct {
	Tasks   bool `json:"tasks"`
	Daily   bool `json:"daily"`
	Checkin bool `json:"checkin"`
}

// GetChannelCaps 读渠道的运营能力（纯函数，不打上游）。
func (c *Client) GetChannelCaps(ctx context.Context, channelID string) (*OpsCaps, error) {
	var out OpsCaps
	path := "/internal/channels/" + url.PathEscape(channelID) + "/caps"
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// UpstreamModel 是渠道目录里的一条模型。
type UpstreamModel struct {
	ID             string   `json:"id"`
	Credits        string   `json:"credits"`
	ContextLength  *int64   `json:"context_length"`
	MaxOutputToken *int64   `json:"max_output_tokens"`
	Vendor         string   `json:"vendor"`
	Capabilities   struct {
		Vision    bool `json:"vision"`
		ToolCalls bool `json:"tool_calls"`
		Reasoning bool `json:"reasoning"`
	} `json:"capabilities"`
}

// GetChannelModels 拉渠道的实时模型目录。
func (c *Client) GetChannelModels(ctx context.Context, channelID string) ([]UpstreamModel, error) {
	var out []UpstreamModel
	path := "/internal/channels/" + url.PathEscape(channelID) + "/models"
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// ProbeResult 是探活结果。
type ProbeResult struct {
	OK        bool   `json:"ok"`
	Model     string `json:"model"`
	LatencyMs int64  `json:"latencyMs"`
	Reply     string `json:"reply"`
	Error     string `json:"error"`
}

// ProbeChannel 用该渠道第一个账号发一条最小请求。
func (c *Client) ProbeChannel(ctx context.Context, channelID string) (*ProbeResult, error) {
	var out ProbeResult
	path := "/internal/channels/" + url.PathEscape(channelID) + "/probe"
	if err := c.post(ctx, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- 账号 ----

// LoginTicket 是发起登录后的票据。
type LoginTicket struct {
	State         string `json:"state"`
	AuthURL       string `json:"authUrl"`
	NeedsCallback bool   `json:"needsCallback"`
	HasAuthPage   bool   `json:"hasAuthPage"`
	Hint          string `json:"hint"`
}

// StartLogin 发起登录。
//
// `platform` 必传：网关按平台选适配器（WorkBuddy 与 Trae 的授权流程不同），
// 并把新账号的 `platform` 写成它。
func (c *Client) StartLogin(ctx context.Context, channelID, platform, realm string) (*LoginTicket, error) {
	var out LoginTicket
	path := "/internal/channels/" + url.PathEscape(channelID) + "/login/start"
	body := map[string]any{"platform": platform, "login_platform": "windows"}
	if realm != "" {
		body["realm"] = realm
	}
	if err := c.post(ctx, path, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// LoginPollResult 是登录轮询结果。
type LoginPollResult struct {
	State   string `json:"state"`
	Message string `json:"message"`
	UID     string `json:"uid"`
}

// PollLogin 轮询登录结果。
func (c *Client) PollLogin(ctx context.Context, channelID, state, platform string) (*LoginPollResult, error) {
	var out LoginPollResult
	path := "/internal/channels/" + url.PathEscape(channelID) + "/login/poll"
	if err := c.post(ctx, path, map[string]any{"state": state, "platform": platform}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// CompleteLogin 用粘贴的回调地址完成登录。
func (c *Client) CompleteLogin(
	ctx context.Context,
	channelID, state, callback, platform string,
) (*LoginPollResult, error) {
	var out LoginPollResult
	path := "/internal/channels/" + url.PathEscape(channelID) + "/login/complete"
	body := map[string]any{"state": state, "callback": callback, "platform": platform}
	if err := c.post(ctx, path, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// RefreshResult 是凭据刷新结果。
type RefreshResult struct {
	OK        bool   `json:"ok"`
	ExpiresAt *int64 `json:"expiresAt"`
	Message   string `json:"message"`
}

// RefreshCredential 强制刷新账号凭据。
func (c *Client) RefreshCredential(ctx context.Context, channelID, uid string) (*RefreshResult, error) {
	var out RefreshResult
	path := "/internal/accounts/" + AccountKey(channelID, uid) + "/refresh"
	if err := c.post(ctx, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// AccountProbe 是账号探活结果。
type AccountProbe struct {
	OK        bool   `json:"ok"`
	LatencyMs int64  `json:"latencyMs"`
	Reply     string `json:"reply"`
	Error     string `json:"error"`
}

// ProbeAccount 用该账号的真实凭据打一次上游。
func (c *Client) ProbeAccount(ctx context.Context, channelID, uid, model string) (*AccountProbe, error) {
	var out AccountProbe
	path := "/internal/accounts/" + AccountKey(channelID, uid) + "/probe"
	// 指定模型时走查询参数；空则让网关按"免费优先"自动挑。
	if model != "" {
		path += "?model=" + url.QueryEscape(model)
	}
	if err := c.post(ctx, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Quota 是额度快照。
type Quota struct {
	Label     string     `json:"label"`
	Unit      string     `json:"unit"`
	Remaining *float64   `json:"remaining"`
	Total     *float64   `json:"total"`
	Used      *float64   `json:"used"`
	Details   [][]string `json:"details"`
	FetchedAt int64      `json:"fetched_at_ms"`
}

// GetQuota 读账号额度快照（没有则返回 nil）。
func (c *Client) GetQuota(ctx context.Context, channelID, uid string) (*Quota, error) {
	var out *Quota
	path := "/internal/accounts/" + AccountKey(channelID, uid) + "/quota"
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// ---- 任务 ----

// TaskInfo 是任务清单里的一条。
type TaskInfo struct {
	Code        string `json:"code"`
	Name        string `json:"name"`
	Status      string `json:"status"`
	Progress    string `json:"progress"`
	Reward      int    `json:"reward"`
	Automatable bool   `json:"automatable"`
	Note        string `json:"note"`
}

// DailyAction 是一条每日动作。
type DailyAction struct {
	Code        string `json:"code"`
	Name        string `json:"name"`
	Action      string `json:"action"`
	DoneToday   bool   `json:"doneToday"`
	Automatable bool   `json:"automatable"`
	Note        string `json:"note"`
}

// TaskAccountLive 是单个账号的任务快照。
type TaskAccountLive struct {
	UID          string        `json:"uid"`
	Nickname     string        `json:"nickname"`
	Enabled      bool          `json:"enabled"`
	CredentialOK bool          `json:"credentialOk"`
	Running      bool          `json:"running"`
	RunID        *int64        `json:"runId"`
	Error        string        `json:"error"`
	Tasks        []TaskInfo    `json:"tasks"`
	Daily        []DailyAction `json:"daily"`
	// Growth 是成长体系快照（加油站 / 连登 / 抽奖）。
	// 指针：网关超时或上游不可用时为 null，界面据此显示"不可用"而不是一堆 0。
	Growth *GrowthSnapshot `json:"growth"`
}

// GrowthSnapshot 是成长体系的只读快照。
//
// 每个字段都用指针：上游某个接口失败时该字段为 null，
// 与"真实的 0"区分开 —— 把"查不到"显示成 0 会误导。
type GrowthSnapshot struct {
	Energy         *int64  `json:"energy"`
	StreakDays     *int64  `json:"streakDays"`
	MakeupCards    *int64  `json:"makeupCards"`
	Tier7d         *string `json:"tier7d"`
	Tier14d        *string `json:"tier14d"`
	Tier28d        *string `json:"tier28d"`
	LotteryChances *int64  `json:"lotteryChances"`
}

// TaskChannelLive 是一个渠道的任务快照。
type TaskChannelLive struct {
	ChannelID string            `json:"channelId"`
	FetchedAt int64             `json:"fetchedAt"`
	Accounts  []TaskAccountLive `json:"accounts"`
}

// CatalogCapabilities 是网关导出的能力字段。
//
// **能力来源单独一个字段**：`snapshot` 表示这几位来自内置对照表（我们按厂商
// 官方文档维护），`none` 表示完全没有依据。两者都不该被压成"未知"。
type CatalogCapabilities struct {
	Vision    bool   `json:"vision"`
	Tools     bool   `json:"tools"`
	Reasoning bool   `json:"reasoning"`
	// Source 是能力字段的来源：live / snapshot / none。
	//
	// 只有 none 才该显示"能力未知"：snapshot 是我们按官方对照表维护的，
	// 有依据（只是可能过时），界面上要说清这一点，而不是说"不知道"。
	Source string `json:"source"`
}

// CatalogModel 是网关导出的一个对外模型（含它在各渠道下的路由）。
type CatalogModel struct {
	Name             string              `json:"name"`
	ContextWindows   []int64             `json:"contextWindows"`
	ReasoningEfforts []string            `json:"reasoningEfforts"`
	MaxOutput        *int64              `json:"maxOutput"`
	Capabilities     CatalogCapabilities `json:"capabilities"`
	Channels         []struct {
		ChannelID     string `json:"channelId"`
		UpstreamModel string `json:"upstreamModel"`
		Priority      int    `json:"priority"`
		Weight        int    `json:"weight"`
		Enabled       bool   `json:"enabled"`
		Free          bool   `json:"free"`
		Source        string `json:"source"`
	} `json:"channels"`
}

// GetCatalog 拉网关内存里的模型目录。
//
// 模型清单**只存在于网关的矩阵里**（从渠道适配器实时拉取），
// 从来没落进 `models` 表。管理面的模型页与公开广场都读那张表，
// 所以必须由管理面主动同步一次。
func (c *Client) GetCatalog(ctx context.Context) ([]CatalogModel, error) {
	var out struct {
		Models []CatalogModel `json:"models"`
	}
	if err := c.get(ctx, "/internal/catalog", &out); err != nil {
		return nil, err
	}
	return out.Models, nil
}

// PriceEntry 是一条模型价格（元 / 百万 token）。
type PriceEntry struct {
	Model     string  `json:"model"`
	Input     float64 `json:"input"`
	Output    float64 `json:"output"`
	CacheRead float64 `json:"cache_read"`
}

// SetPrices 把模型价格表推给网关。
//
// 网关不查 `models` 表，价格是它内存里的一份副本；
// 改了价格必须推一次，否则它会继续按旧价记账 ——
// 那种错误不报错、只让账单慢慢偏掉。
func (c *Client) SetPrices(ctx context.Context, entries []PriceEntry) error {
	body := map[string]any{"prices": entries}
	return c.post(ctx, "/internal/prices", body, nil)
}

// SealSecret 让网关用主密钥加密一段文本，返回密文。
//
// 为什么 Go 不自己加密：主密钥只在网关进程里（这是有意的 ——
// 管理面被攻破时不应该连上游凭据一起暴露）。所以任何需要落库的机密
// 都必须经过网关这一道。
func (c *Client) SealSecret(ctx context.Context, plaintext string) (string, error) {
	var out struct {
		Cipher string `json:"cipher"`
	}
	if err := c.post(ctx, "/internal/secrets/seal", map[string]any{"plaintext": plaintext}, &out); err != nil {
		return "", err
	}
	if out.Cipher == "" {
		return "", fmt.Errorf("网关未返回密文")
	}
	return out.Cipher, nil
}

// GetTaskLive 拉渠道任务快照（N 次上游请求，慢）。
func (c *Client) GetTaskLive(ctx context.Context, channelID string) (*TaskChannelLive, error) {
	var out TaskChannelLive
	path := "/internal/tasks/channels/" + url.PathEscape(channelID) + "/live"
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// StartTaskRun 触发一次执行（后台跑）。
//
// `runID` 是 Go 侧刚插入 `task_runs` 得到的行 id，网关据此把每条明细回写
// `task_run_items` 并把最终状态写回 `task_runs`（SPEC 4.4：执行历史落 PG）。
// 不传这个字段，网关就只把进度留在内存里，刷新页面后历史页永远是空的。
func (c *Client) StartTaskRun(
	ctx context.Context,
	channelID, uid, scope string,
	codes []string,
	runID int64,
) (bool, string, error) {
	var out struct {
		Started bool   `json:"started"`
		Reason  string `json:"reason"`
	}
	body := map[string]any{
		"channel_id": channelID,
		"uid":        uid,
		"scope":      scope,
		// 指定任务码时网关只跑这些；为空则按 scope 跑整批。
		"codes":  codes,
		"run_id": runID,
	}
	if err := c.post(ctx, "/internal/tasks/run", body, &out); err != nil {
		return false, "", err
	}
	return out.Started, out.Reason, nil
}

// CancelTaskRun 请求网关停止某个账号正在执行的任务（SPEC 11.7）。
//
// 返回 false 表示网关那边已经没有执行中的任务（可能刚好跑完）。
// 状态在 PG 里的改写由 Store.CancelTaskRun 负责，两边都要做：
// 只改库 = 执行线程不知道要停；只通知网关 = 界面看不到状态变化。
func (c *Client) CancelTaskRun(ctx context.Context, channelID, uid string) (bool, string, error) {
	var out struct {
		Accepted bool   `json:"accepted"`
		Reason   string `json:"reason"`
	}
	body := map[string]any{"channel_id": channelID, "uid": uid}
	if err := c.post(ctx, "/internal/tasks/cancel", body, &out); err != nil {
		return false, "", err
	}
	return out.Accepted, out.Reason, nil
}

// TaskProgress 是执行进度。
type TaskProgress struct {
	Running bool           `json:"running"`
	Results []TaskRunItem  `json:"results"`
	Done    int            `json:"done"`
	Earned  int            `json:"earned"`
	Error   string         `json:"error"`
}

// TaskRunItem 是一条执行结果。
type TaskRunItem struct {
	Code    string `json:"code"`
	Name    string `json:"name"`
	Status  string `json:"status"`
	Message string `json:"message"`
	Earned  int    `json:"earned"`
}

// GetTaskProgress 读执行进度。
func (c *Client) GetTaskProgress(ctx context.Context, channelID, uid string) (*TaskProgress, error) {
	var out TaskProgress
	path := fmt.Sprintf(
		"/internal/tasks/progress?channel_id=%s&uid=%s",
		url.QueryEscape(channelID),
		url.QueryEscape(uid),
	)
	if err := c.get(ctx, path, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ClaimTask 单独领取一项已达标任务的奖励。
func (c *Client) ClaimTask(ctx context.Context, channelID, uid, code string) (*TaskRunItem, error) {
	var out TaskRunItem
	body := map[string]any{"channel_id": channelID, "uid": uid, "code": code}
	if err := c.post(ctx, "/internal/tasks/claim", body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Reload 通知网关重新加载渠道与账号（管理面改配置后调用）。
func (c *Client) Reload(ctx context.Context) error {
	return c.post(ctx, "/internal/state/reload", nil, nil)
}

// CreateAccountInput 是新建账号的输入。
//
// 平台在**账号**上：`Platform` 必填，它决定用哪个适配器。
// 渠道只负责"这个号算在哪个计价分组里"。
type CreateAccountInput struct {
	ChannelID string          `json:"channelId"`
	UID       string          `json:"uid"`
	Nickname  string          `json:"nickname"`
	APIKey    string          `json:"apiKey"`
	Platform  string          `json:"platform"`
	BaseURL   string          `json:"baseUrl"`
	Protocol  string          `json:"protocol"`
	Options   json.RawMessage `json:"options"`
}

// CreateAccount 用静态凭据建号。
//
// 为什么走网关而不是 Go 直接写库：凭据要用主密钥加密，而主密钥只在网关进程里
// （SPEC I1：明文不落库、不进日志）。Go 侧不持有主密钥是刻意的隔离。
//
// `platform` / `baseUrl` / `protocol` / `options` 全是**账号级**上游配置：
// 一份计价策略（渠道）下可以挂多个不同上游、不同平台的 Key。
func (c *Client) CreateAccount(ctx context.Context, input CreateAccountInput) error {
	body := map[string]any{
		"channel_id": input.ChannelID,
		"uid":        input.UID,
		"nickname":   input.Nickname,
		"api_key":    input.APIKey,
		"platform":   input.Platform,
		"base_url":   input.BaseURL,
		"protocol":   input.Protocol,
		"options":    input.Options,
	}
	return c.post(ctx, "/internal/accounts", body, nil)
}
