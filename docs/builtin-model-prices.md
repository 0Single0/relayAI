# 内置模型价格

核对日期：**2026-10-29**。内置表包含 **104 个明确列出的模型 ID**，数据位于 [`backend/internal/store/builtin_prices.go`](../backend/internal/store/builtin_prices.go)。每项保存输入、输出、缓存读取价格、原始币种、官方来源 URL 和采用的计费档位。

管理台“模型广场 → 内置价格填充”调用 `POST /api/v1/models/prices/builtin`，只填已有模型目录中**三项价格均为零**的条目。目录同步本身不触发填价。已设置的价格（包括只填过输入、输出或缓存中的一项）保留；更新这份内置表也不会自动覆盖数据库里的旧价格。

## 单位与范围

- 所有报价均为**每百万 tokens**。CNY 报价原值入库；USD 报价按系统设置的 `usdToCny` 换算，缺省为 7.2。人民币报价不会再次乘汇率。
- 价格是公开 API 参考单价，不是 WorkBuddy 的积分倍率、会员费用、免费额度或协议折扣。
- 现有数据库每个模型仅保存一组输入、输出和缓存读取单价，无法表达完整的长上下文阶梯、峰谷、区域、思考/非思考模式、缓存写入/存储、工具调用费用。本表选择的档位在每项 `Basis` 中说明；它不能还原所有请求的厂商实际账单。
- OpenAI 取 Standard、短上下文；Gemini 取付费 Standard 的基础文本输入档；Claude 取基础 tokens 与缓存命中；DeepSeek 取**高峰价格**，其空闲时段当前为该价格的 50%。GLM、Kimi 优先取国内人民币报价；未在国内页面列出的历史 GLM 条目采用 Z.AI 国际美元报价。
- `cacheRead = 0` 表示该条未引用缓存折扣或该模型不支持缓存，**不表示缓存免费**。现有网关会按输入单价计费。缓存写入与存储费用未纳入这三个字段。
- 本次未引入有到期日的促销报价；MiniMax-M3 的 50% 调整在官方页面明确标为永久调整。

## 官方来源

| 来源 | 本表采用范围 |
| --- | --- |
| [OpenAI API Pricing](https://developers.openai.com/api/docs/pricing)（完整表也可读取该地址的 `.md` 版本） | GPT、o 系列 Standard；单独保留 `gpt-4o-2024-05-13` 的不同价格 |
| [Claude Pricing](https://platform.claude.com/docs/en/about-claude/pricing) | Fable、Mythos、Opus、Sonnet、Haiku；基础输入、输出与缓存读取 |
| [Gemini Developer API Pricing](https://ai.google.dev/gemini-api/docs/pricing) | Gemini 2.5、3.1 Pro Preview、3.5 Flash-Lite；付费基础档 |
| [DeepSeek Models & Pricing](https://api-docs.deepseek.com/quick_start/pricing/) | Flash / V4.1-Flash-1017、V4-Pro-0813；峰时价格及官方注明的 V4-Flash 兼容名称 |
| [智谱 BigModel API 定价](https://docs.bigmodel.cn/cn/guide/start/pricing) | GLM 5.x 与视觉模型；国内人民币、短输入档 |
| [Z.AI Pricing](https://docs.z.ai/guides/overview/pricing) | GLM 4.5 / 4.6 / 4.7 等条目的国际美元价 |
| [Kimi 模型推理价格](https://platform.kimi.com/docs/pricing/chat) | K3、K3.1 Pro、K2.6、K2.7 Code；国内人民币价。页面表格为动态组件，核对其页面内嵌表格数据；`.md` 版本会省略表格内容 |
| [MiniMax Pricing Overview](https://platform.minimax.io/docs/pricing/overview) | M3、M3.1-Flash；Standard，M3 输入不超过 512K |
| [腾讯 TokenHub 模型价格](https://cloud.tencent.com/document/product/1823/130055) | 腾讯 Hy3、Hy4 preview；另为 Kimi K2.8 Preview、MiniMax M2.7、Qwen3.5 提供明确标注的**广州地域托管参考价** |
| [阿里云百炼模型价格](https://help.aliyun.com/zh/model-studio/model-pricing) | Qwen3.6/3.7 Plus、Qwen Plus；北京地域基础档、原价；Qwen Plus 采用非思考模式 |

TokenHub 中非腾讯模型的条目是该官方平台的托管报价，**不能当成模型原厂直供价格**，也不能推导为 WorkBuddy 的实际结算价格。`SourceURL` 和 `Basis` 均保留这个区别。相同模型在国际站、不同地域和托管平台可能有不同价格；全局单价字段无法同时表达这些差异。

## WorkBuddy 国内目录的对应情况

当前代码的 14 个国内优先展示 ID 中，11 个有对应报价。下面保留官方原始币种，不混合换算；单位均为每百万 tokens。

| 模型 ID | 币种 | 输入 | 输出 | 缓存读取 | 采用范围 |
| --- | --- | ---: | ---: | ---: | --- |
| `hy3` | CNY | 1 | 4 | 0.25 | 腾讯 TokenHub 广州 |
| `deepseek-v4.1-flash` | USD | 0.30 | 1.20 | 0.006 | DeepSeek V4.1-Flash-1017，峰时参考 |
| `glm-5.3` | CNY | 8 | 28 | 2 | BigModel 国内 |
| `glm-5.3-flash` | CNY | 0.8 | 2.8 | 0.23 | BigModel 国内 |
| `glm-5.2` | CNY | 8 | 28 | 2 | BigModel 国内 |
| `glm-5.1` | CNY | 6 | 24 | 1.3 | BigModel 国内，输入小于 32K |
| `glm-5v-turbo` | CNY | 5 | 22 | 1.2 | BigModel 国内，输入小于 32K |
| `minimax-m3` | USD | 0.30 | 1.20 | 0.06 | MiniMax Standard，输入不超过 512K |
| `kimi-k2.8-preview` | CNY | 6.5 | 27 | 1.7 | TokenHub 广州托管参考价 |
| `kimi-k2.6` | CNY | 6.5 | 27 | 1.1 | Kimi 国内 |
| `deepseek-v4-pro` | USD | 1.32 | 3.96 | 0.044 | DeepSeek V4-Pro-0813，峰时 |

以下三个 ID 没有确认官方型号对应关系，保持未定价：

- `hy4-preview-f`：WorkBuddy 路由带 `-f`，不能直接认定等于 TokenHub 的 `hy4-preview`。`hy4-preview` 自身已有 CNY 6 / 18 / 0.3 的报价。
- `kimi-k3-1`：官方当前列出的是 `kimi-k3.1-pro`，不能仅凭相似名称确定为同一型号。
- `kimi-k2.7`：官方当前列出的是 `kimi-k2.7-code`，未证实该路由的对应关系。

快照中的 `glm-5.0-turbo`、`*-volc`、`*-lkeap`、`*-taiji`、`default-*` 等 ID 也不进行猜测映射。旧表中的 DeepSeek V3/R1、`deepseek-chat` / `deepseek-reasoner`、`kimi-k2` / `kimi-k2.5`、`qwen3-max/plus/turbo`、Claude 3.5/3.7 Sonnet 等未在本次官方页面中重新证实的条目不再自动填入；已经保存的数据库价格保持原值。

## 精确匹配与验证

内置填充与网关计费均只按完整模型 ID 匹配，忽略大小写和首尾空格。日期版本也需要明确列入或单独配置，不能用前缀推算：例如 `gpt-4o-2024-05-13` 的官方输入/输出是 USD 5 / 15，而 `gpt-4o` 是 USD 2.5 / 10。

回归测试覆盖币种换算、关键官方报价、版本隔离、人工部分定价保护和重复填充。数据库集成测试使用 `TEST_DATABASE_URL_PRICING`，仅创建/删除随机命名的独立 schema，不清空应用的 `public` schema。

```powershell
cd backend
go test ./internal/store -run TestBuiltinPrice -v

cd ../gateway
cargo test gateway::core::billing::tests
```

## 部署与使用

更新服务器上的源码后，在部署目录执行：

```bash
sudo docker compose up -d --build --no-deps backend gateway
```

然后在管理台打开“模型广场”，目录为空时先“从渠道同步”，再执行“内置价格填充”。这只更新未定价条目；已设置的旧价需要通过现有价格编辑操作明确修改。无需重新导入账号。
