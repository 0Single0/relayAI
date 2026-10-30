# Claude Code 的 WorkBuddy 渠道校验兼容修复

## 问题与原因

Claude Code 通过 Anthropic Messages 接口调用 WorkBuddy 时，可能收到网关转换后的 HTTP 429，错误类型为 `channel_blocked_error`。上游原始错误是 `11128 / Illegal API invocation from an unapproved channel`，不能仅凭网关返回的 429 将它归为使用额度或频率限制。

2026-10-30，在本机同一个 WorkBuddy 国内账号、同一模型 `deepseek-v4.1-flash` 上进行了对照：

| system 内容 | 修复前 | 修复后 |
| --- | --- | --- |
| 普通助手提示词 | HTTP 200 | HTTP 200 |
| `You are Claude Code, Anthropic's official CLI for Claude.` | HTTP 429，渠道校验拦截 | HTTP 200 |
| `You are Claude Code.` | HTTP 200 | HTTP 200 |
| 计费头 + 普通助手提示词 | HTTP 200 | HTTP 200 |
| 计费头 + 上述 Claude Code 原始身份声明 | HTTP 429，渠道校验拦截 | HTTP 200 |

与用户提供的本地 `workbuddy2api-hub-main` 参考项目对比后确认：参考项目的 `wb_proxy.py / SANITIZE_REWRITES` 会处理不带 `tool` 的 `official CLI for Claude` 写法，而我们的兼容规则此前只处理带 `tool` 的 `official CLI tool for Claude` 写法。未命中的原文被转发给上游，稳定触发了本次拦截。

这些结果只能说明已验证账号与请求的行为；上游其他形式的渠道校验仍可能需要独立排查。

## 修复内容

- WorkBuddy 请求卫生管道同时处理两种 Claude Code 身份声明，保留 `You are Claude Code` 及后续指令。
- 按计费头和 `cc_*` 键值对清理相关文本，覆盖大小写及截断的头前缀；不再因为出现 `cc_` 就删除整行剩余内容，避免误删后续指令及 `cc_helpers.py` 等正常标识符。
- 渠道校验错误提示改为检查请求兼容性和出站身份，不再承诺“随机拦截、重试即可”。保留原有请求级错误分类、有限重试标记及上游原文。
- 将请求卫生管道的单元测试移入 `gateway/src/gateway/channels/workbuddy/hygiene/tests.rs`，保持源文件低于 1000 行。

修复没有修改出站请求头或账号选择策略。本次对照请求已在相同出站身份下验证恢复。

## 验证

在本机运行的修复版网关 `http://127.0.0.1:18788` 上验证：

- 上表五组真实上游请求均返回 HTTP 200；最后一次对照均回复 `OK`。
- 实际 Claude Code 2.1.296 使用默认 system、隔离的设置来源及临时渠道密钥，普通对话成功回复 `OK`。
- 同一客户端调用 `Read` 读取临时文本文件，第二轮成功返回文件内容，验证工具调用和工具结果回传。
- 测试使用的临时 API Key 在结束后删除，没有修改用户的 Claude Code 配置。
- `cargo test --quiet`：288 项通过、12 项需要外部条件的测试默认跳过。
- 网关开发版本编译成功；前端 lint、typecheck、build 及后端 `go vet ./...`、`go build ./...` 通过。

新增回归测试覆盖 Anthropic Messages 解码到 WorkBuddy 出站报文的完整转换，检查身份改写、计费头清理、中文指令保留、工具定义保留、原始顶层 system 不再透传，以及重复清理的幂等性。

远程或 Docker 部署需重新构建并重启网关才能使用新规则：

```sh
docker compose up -d --build gateway
```
