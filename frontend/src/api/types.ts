/** 管理面共享类型，与 docs/SPEC.md 第 7 节 API 契约一一对应。 */
import type { CustomModelDefinition } from "./accounts/models";

export type PageResult<T> = {
  list: T[];
  total: number;
  page: number;
  pageSize: number;
};

export type ChannelAdapter = "workbuddy" | "trae" | "custom";

export type OpsCaps = {
  tasks: boolean;
  daily: boolean;
  checkin: boolean;
};

export type ChannelOptions = {
  realm?: "cn" | "intl";
  outbound_identity?: "desktop" | "cli" | "vscode";
  base_url?: string;
  ug_base_url?: string;
  protocol?: "openai-chat" | "openai-responses" | "anthropic";
  require_api_key?: boolean;
};

export type Channel = {
  id: string;
  name: string;
  adapter: ChannelAdapter;
  enabled: boolean;
  priority: number;
  weight: number;
  options: ChannelOptions;
  channelRatio: number;
  concurrencyLimit: number | null;
  rateLimitQps: number | null;
  accountCount: number;
  todayRequests: number;
  /** 对外暴露的模型白名单；空 = 不限制。 */
  modelAllow: string[];
  caps: OpsCaps;
  /**
   * 该渠道名下账号出现过的**平台**集合（去重）。
   *
   * 正常情况下仅包含分组绑定的平台；保留数组以展示已有账号的实际平台。
   */
  platforms: string[];
};

export type Account = {
  id: number;
  channelId: string;
  channelName: string;
  adapter: ChannelAdapter;
  /** 上游平台（决定用哪个适配器）。 */
  platform: string;
  baseUrl?: string;
  protocol?: string;
  options?: ChannelOptions & { models?: Array<string | CustomModelDefinition> };
  uid: string;
  nickname: string;
  enabled: boolean;
  priorityOverride: number | null;
  weightOverride: number | null;
  maxInflight: number | null;
  modelAllow: string[];
  modelRename: Record<string, string>;
  source: string;
  addedAt: string;
  /** 运行态，来自网关；网关不可用时为 null。 */
  runtime: AccountRuntime | null;
  /** 用量汇总（今日 / 累计），来自 PG 聚合；查不到时为全 0 占位。 */
  usage: AccountUsage | null;
  /** 出站代理 id；null = 直连。 */
  proxyId: number | null;
  /** 出站代理名；null = 直连。 */
  proxyName: string | null;
};

/** 单个账号的用量汇总。 */
export type AccountUsage = {
  channelId: string;
  uid: string;
  todayRequests: number;
  todayTokens: number;
  todayCost: number;
  todaySuccessRate: number;
  totalRequests: number;
  totalTokens: number;
  totalCost: number;
  /** 最近一次使用时间；空串 = 从未使用。 */
  lastUsedAt: string;
};

export type AccountRuntime = {
  status: "ready" | "cooldown" | "no_credential" | "disabled" | "unknown";
  cooldownUntil: string | null;
  /**
   * 模型级限流明细（上游按模型限额，而不是整号封禁）。
   *
   * 与 `cooldownUntil` 的区别必须分清：那个是整号冷却，界面上该显示"不可用"；
   * 这个是"号还能用，只是某几个模型在等" —— 显示成不可用会让用户
   * 误以为号废了，去删号重建反而丢了配置。
   */
  modelLimits?: Array<{ model: string; untilMs: number }>;
  inflight: number;
  /**
   * 网关的**生效并发上限**（库里 NULL 表示"用渠道默认"，由网关解析成具体值）。
   *
   * 界面显示 `在飞 N/M` 时必须用这个，不能用 `maxInflight`：
   * 后者是原始值、可能为 null，渲染成 `∞` 等于在说"不限并发"，
   * 与网关实际行为相反。
   */
  maxInflight: number;
  /** 不可用原因（网关的 `blocked_reasons`，人话）。 */
  blockedReasons: string[];
  /**
   * 该账号用**静态凭据**（自定义供应商的 API Key）。
   *
   * 静态凭据不会过期，没有"刷新"这回事。界面据此禁用刷新按钮 ——
   * 给它一个可点的刷新按钮，点下去只会得到一个错误，用户会以为系统坏了。
   */
  staticCredential: boolean;
  /** 额度快照（来自网关内存，不打上游）。 */
  quota: AccountQuota | null;
  lastError: string | null;
  lastOkAt: string | null;
  credentialExpiresAt: string | null;
};

/** 账号的额度快照。 */
export type AccountQuota = {
  label: string;
  unit: string;
  remaining: number | null;
  total: number | null;
  used: number | null;
  fetchedAt: string;
};

export type ApiKey = {
  id: number;
  name: string;
  keyPrefix: string;
  channelAllow: string[];
  modelAllow: string[];
  rpmLimit: number | null;
  dailyTokenLimit: number | null;
  maxConcurrency: number | null;
  enabled: boolean;
  expiresAt: string | null;
  lastUsedAt: string | null;
  createdAt: string;
  /**
   * 密钥明文，供界面反复查看与复制（对齐 sub2api）。
   *
   * 老数据可能为 null：它们的明文只在创建时出现过一次，找不回来了。
   * 此时界面显示"仅前缀可用，请轮换"，不假装能复制。
   */
  plaintext: string | null;
  /**
   * 用量汇总（今日 + 累计）。
   *
   * 可选：后端聚合失败时会省略这个字段（不阻断密钥列表）。
   * 缺失时界面显示 0 —— 密钥列表本身能正常用更重要。
   */
  usage?: KeyUsage;
};

/** 单个 Key 的用量汇总。 */
export type KeyUsage = {
  todayRequests: number;
  todayTokens: number;
  /** 记账额度（含倍率），单位元。 */
  todayCost: number;
  totalRequests: number;
  totalTokens: number;
  totalCost: number;
  lastUsed: string | null;
};

export type Model = {
  canonicalName: string;
  displayName: string;
  contextWindow: number | null;
  maxOutput: number | null;
  capabilities: {
    vision?: boolean;
    tool_calls?: boolean;
    reasoning?: boolean;
  };
  inputPrice: number;
  outputPrice: number;
  cacheReadPrice: number;
  modelRatio: number;
  enabled: boolean;
  /** 是否已在 models 表里定价；渠道目录里有但这里没有 = 未定价。 */
  priced: boolean;
};

export type UsageStatus =
  | "completed"
  | "failed"
  | "client_aborted"
  | "upstream_aborted"
  | "rejected";

export type UsageLog = {
  id: number;
  ts: string;
  requestId: string;
  keyId: number | null;
  keyName: string | null;
  channelId: string;
  accountUid: string;
  protocol: "chat" | "responses" | "anthropic";
  model: string;
  upstreamModel: string | null;
  stream: boolean;
  status: UsageStatus;
  isRejection: boolean;
  attempts: number;
  promptTokens: number;
  completionTokens: number;
  reasoningTokens: number;
  cachedTokens: number;
  totalTokens: number;
  usageEstimated: boolean;
  upstreamCost: number;
  billedCost: number;
  ttftMs: number | null;
  totalMs: number;
  /**
   * 输出速度（token/秒）。
   *
   * null 表示样本太小、不参与统计（经路由需 ≥100 token，未经路由需 ≥200）。
   */
  tokensPerSecond: number | null;
  /**
   * 速度是否为**近似值**。
   *
   * true 表示这条记录没有首字计时，用总时长估算 —— 等首字的时间被算进了分母，
   * 所以数值偏低。界面上必须标 `≈`，别让人拿它和精确值直接比。
   */
  speedEstimated: boolean;
  errorDetail: string | null;
  /**
   * 计费时的单价与倍率快照。
   *
   * null 表示这条记录早于快照功能 —— 不要用今天的价格去反推，
   * 那会算出一个和账单对不上的数。
   */
  priceSnapshot: UsagePriceSnapshot | null;
};

/** 一条用量的价格快照（元 / 百万 token，未乘倍率）。 */
export type UsagePriceSnapshot = {
  inputPrice: number;
  outputPrice: number;
  cacheReadPrice: number;
  modelRatio: number;
  channelRatio: number;
};

export type UsageSummary = {
  requests: number;
  successRate: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cachedTokens: number;
  upstreamCost: number;
  billedCost: number;
  avgTtftMs: number | null;
};

export type UsageSeriesPoint = {
  bucket: string;
  requests: number;
  totalTokens: number;
  upstreamCost: number;
  /** 该桶输入 Token（用于堆叠趋势图）。 */
  promptTokens: number;
  /** 该桶输出 Token。 */
  completionTokens: number;
  /** 该桶命中的缓存 Token（不计入 totalTokens）。 */
  cachedTokens: number;
  /** 该桶平均首字延迟；没有数据时为 null（0ms 是假的）。 */
  avgTtftMs: number | null;
  /** 该桶平均总耗时；没有数据时为 null。 */
  avgLatencyMs: number | null;
  /** 该桶成功率（0–1）。 */
  successRate: number;
  /** 该桶失败数。 */
  failed: number;
};

export type TaskChannel = {
  id: string;
  name: string;
  adapter: ChannelAdapter;
  caps: OpsCaps;
  accountCount: number;
  /** 今日还有待办的账号数。 */
  pendingAccounts: number;
  /**
   * WorkBuddy 版本：`cn` 国内版 / `intl` 国际版。
   *
   * 两者的任务内容完全不同：国内版有成长任务 + 签到 + 猫猫旅行，
   * 国际版只有每日活跃打卡。菜单与页面据此区分。
   */
  realm: string;
};

export type TaskScope = "all" | "daily" | "tasks" | "checkin";

export type TaskRunStatus =
  | "running"
  | "done"
  | "partial"
  | "failed"
  | "cancelled";

export type TaskRun = {
  id: number;
  batchId: string | null;
  channelId: string;
  channelName: string;
  accountUid: string;
  accountNickname: string;
  scope: TaskScope;
  status: TaskRunStatus;
  trigger: "manual" | "schedule";
  total: number;
  doneCount: number;
  failedCount: number;
  skippedCount: number;
  earned: number;
  error: string | null;
  startedAt: string;
  finishedAt: string | null;
};

export type TaskRunItem = {
  id: number;
  runId: number;
  code: string;
  name: string;
  kind: "daily" | "task" | "checkin";
  status: "done" | "skipped" | "failed" | "not_in_window";
  message: string;
  earned: number;
  finishedAt: string;
};

export type TaskInfo = {
  code: string;
  name: string;
  status: "not_accepted" | "in_progress" | "claimable" | "done" | "unsupported";
  progress: string;
  reward: number;
  automatable: boolean;
  note: string;
};

export type DailyAction = {
  code: string;
  name: string;
  action: string;
  doneToday: boolean;
  automatable: boolean;
  note: string;
};

export type TaskAccountLive = {
  uid: string;
  /**
   * 该账号**当前所属分组**；空串 = 还没分配（待分配）。
   *
   * 任务页按版本取号，同屏可能出现来自不同分组、甚至没分组的号，
   * 所以执行/取消都要用账号自己的分组，而不是页面级的"代表分组"。
   */
  channelId: string;
  nickname: string;
  enabled: boolean;
  credentialOk: boolean;
  running: boolean;
  runId: number | null;
  error: string | null;
  tasks: TaskInfo[];
  daily: DailyAction[];
  /** 成长体系快照；网关超时或上游不可用时为 null。 */
  growth: GrowthSnapshot | null;
};

/** 成长体系快照（加油站 / 连登 / 抽奖）。整块可能在网关超时时为 null。 */
export type GrowthSnapshot = {
  /** 加油站：当前能量余额。 */
  energy: number | null;
  /** 查询失败或响应缺少有效数值时的原因。 */
  energyError?: string | null;
  /** 连登：连续天数。 */
  streakDays: number | null;
  streakError?: string | null;
  /** 连登：补签卡余额。 */
  makeupCards: number | null;
  /** 连登：7 / 14 / 28 天档位状态。 */
  tier7d: string | null;
  tier14d: string | null;
  tier28d: string | null;
  /** 抽奖：剩余次数。 */
  lotteryChances: number | null;
};

export type TaskChannelLive = {
  channelId: string;
  fetchedAt: string;
  accounts: TaskAccountLive[];
};

export type GatewayStatus = {
  running: boolean;
  addr: string | null;
  error: string | null;
  uptimeMs: number;
  version: string;
  concurrency: { global: number; available: number };
  matrix: { routes: number; live: number; builtin: number; ageMs: number };
  stats: { requests: number; failures: number; retries: number };
  channels: Array<{
    id: string;
    adapter: string;
    accounts: number;
    ready: number;
  }>;
  warnings: string[];
};

export type Settings = {
  taskRetentionDays: number;
  taskScheduleEnabled: boolean;
  taskScheduleHours: number[];
  taskPerAccountGapMs: number;
  taskDailyGapMs: number;
  taskReportGapMs: number;
  gatewayPort: number;
  gatewayBind: string;
  logRetentionDays: number;
  /** 告警 webhook 地址；空串 = 不发通知（SPEC 9.13）。 */
  webhookUrl: string;
  /** 定时探活开关：对每个启用的账号发一句 hello 确认凭据可用。 */
  probeEnabled: boolean;
  /** 探活间隔（分钟），后端钳制在 5–1440。 */
  probeIntervalMinutes: number;
};

/**
 * 分组健康度（「分组状态」页的一张卡片）。
 *
 * 可为 null 的字段一律**不要**在渲染前补成 0：`null` 表示"没测过"，
 * `0` 表示"测了，结果是 0"，两者在告警语境下完全相反。
 *
 * 口径：卡片上**所有**指标都来自定时探活（`channel_probes`），
 * 不是真实请求（`usage_logs`）。一次探活 = 下方状态条的一格，
 * 所以这排格子长得多快直接反映探活间隔，不会再随流量起伏。
 */
export type ChannelHealth = {
  id: string;
  name: string;
  adapter: string;
  enabled: boolean;
  accountCount: number;
  /** 网关运行态；网关不可用时为 null。 */
  readyAccounts: number | null;
  /** 窗口内的**探活次数**（按分组 × 模型逐条累加），不是请求数。 */
  probes: number;
  /** 窗口内的探活成功率（0–1）；没有探活记录时为 null。 */
  successRate: number | null;
  /** 探活的平均端到端耗时。 */
  avgLatencyMs: number | null;
  /**
   * 最近 60 次探活状态，旧 → 新，值原样来自 `channel_probes.status`
   * （`ok` / `limited` / `auth` / `quota` / `error`）。
   *
   * 一格 = 一次探活，不受上方统计窗口影响：选「24 小时」时格子不会突然变少。
   */
  recent: string[];
  /** 最近一次探活时刻（RFC3339）；从未探过为 null。 */
  lastProbeAt: string | null;
  /** 最近一次**探活失败**的原因；没有失败为 null。 */
  lastError: string | null;
  lastErrorAt: string | null;
  /**
   * 探活实际使用的对外模型名；空 = 由网关按「免费优先」自动挑。
   *
   * 允许指定是因为"免费模型通了"不等于"真实模型能通"。
   */
  probeModel: string;
  /**
   * 这个分组**全部**要探的模型（`probe_models` 与 `probe_model` 的并集）。
   *
   * 卡片要列全：只显示一个会让人以为另外几个没在探，
   * 也就解释不了"探活次数为什么比轮数多"。
   */
  probeModels: string[];
};

/** 一段区间内的累计指标。 */
export type DashboardTotals = {
  requests: number;
  successRate: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cachedTokens: number;
  upstreamCost: number;
  billedCost: number;
  avgTtftMs: number | null;
  avgLatencyMs: number | null;
};

export type ProtocolBucket = {
  protocol: string;
  requests: number;
  tokens: number;
};

export type RateMetrics = {
  /** 每分钟请求数（今日口径）。 */
  rpm: number;
  /** 每分钟 Token 数。 */
  tpm: number;
  /** 今日单小时最高请求数折算的每分钟速率。 */
  peakRpm: number;
  avgTtftMs: number | null;
  avgLatencyMs: number | null;
  /** 今日已过去秒数，用于解释 RPM 分母。 */
  elapsedSeconds: number;
};

/**
 * 仪表盘快照。
 *
 * 一个接口返回全部数据是刻意的：拆成多次请求会让"今日请求数"和
 * "今日 token"落在不同时间点，两个数字对不上，看起来像数据错了。
 */
export type DashboardOverview = {
  from: string;
  to: string;
  /** 自然日（本地零点起）。 */
  today: DashboardTotals;
  /** 全量累计。 */
  total: DashboardTotals;
  series: UsageSeriesPoint[];
  byChannel: SummaryBucket[];
  byModel: SummaryBucket[];
  byProtocol: ProtocolBucket[];
  todayByHour: UsageSeriesPoint[];
  rates: RateMetrics;
  /** 窗口内的错误分布（按类型分组，最多 8 类）。 */
  errors: ErrorBucket[];
  /** 窗口内的延迟分位。 */
  percentiles: LatencyPercentiles;
};

/** 一类错误的出现次数。 */
export type ErrorBucket = {
  kind: string;
  count: number;
  /** 一条代表性错误信息（已截断）。 */
  sample: string;
};

/** 延迟分位（毫秒）。 */
export type LatencyPercentiles = {
  ttftP50: number | null;
  ttftP95: number | null;
  ttftP99: number | null;
  totalP50: number | null;
  totalP95: number | null;
  totalP99: number | null;
  /** 样本量：分位的可信度取决于它。 */
  sample: number;
};

export type SummaryBucket = {
  key: string;
  label: string;
  requests: number;
  totalTokens: number;
  upstreamCost: number;
  billedCost: number;
  successRate: number;
};
