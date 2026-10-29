# relayAI

自用的 AI 反代网关 + 管理台。

从 dynamic-island-web 迁移已有账号及登录凭据，参见 [账号迁移说明](docs/legacy-account-import.md)。

- **网关**（Rust）：对下游暴露 OpenAI Chat Completions / OpenAI Responses / Anthropic Messages 三种协议入口；对上游调度 WorkBuddy / Trae / 自定义渠道商的账号池。
- **管理台**（Go + React）：渠道、账号、密钥、用量、任务、设置。

> 执行以 [docs/SPEC.md](docs/SPEC.md) 为准。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 前端 | React 19 + TypeScript + Vite + Tailwind CSS 4 + shadcn/ui |
| 管理面 | Go 1.24+，标准库 `net/http` + pgx |
| 网关 | Rust 1.80+，axum + reqwest + sqlx |
| 数据库 | PostgreSQL 16+ |
| 图表 | Recharts |
| 图标 | Lucide |

## 目录

```text
relayAI/
├── frontend/                 # React 管理台
│   └── src/
│       ├── api/              # 按业务分组的接口
│       ├── components/       # common（共享）+ ui（shadcn 原语）
│       ├── pages/            # 目录级页面
│       ├── routes/           # 路由与导航元数据
│       └── styles/           # 主题 token
├── backend/                  # Go 管理面
│   ├── backend.go
│   ├── backend.api           # API 契约说明（不用于生成）
│   └── internal/
│       ├── config/           # 配置
│       ├── database/         # 连接池 + 迁移（含 migrations/*.sql）
│       ├── gateway/          # Rust 网关内部接口客户端
│       ├── handler/          # HTTP 路由与处理器
│       ├── httpx/            # 统一响应与错误信封
│       ├── store/            # 数据访问层
│       └── svc/              # 服务上下文
├── gateway/                  # Rust 网关
│   └── src/
│       ├── main.rs
│       ├── secrets.rs        # 主密钥（环境变量）
│       ├── sse.rs            # SSE 帧重组
│       └── gateway/
│           ├── channels/     # 渠道适配器
│           ├── core/         # 调度、选号、矩阵、亲和
│           ├── protocol/     # 三协议编解码 + CIR
│           ├── server/       # /v1/* 与 /internal/*
│           ├── keyauth.rs    # Key 鉴权与隔离
│           ├── storage.rs    # PostgreSQL
│           └── state.rs      # 共享状态
├── scripts/                  # 冒烟测试与验证脚本
└── docs/                     # 规划与规格文档
```

## 环境要求

- Node.js >= 22.12（已验证 22.19.0）
- pnpm 11（`packageManager` 已锁定）
- Go >= 1.24（本机验证 1.25.5）
- Rust >= 1.80
- PostgreSQL >= 16（本机验证 18.2）

国内网络建议：
- Go：`GOPROXY=https://goproxy.cn,direct`
- Rust：`~/.cargo/config.toml` 配镜像源

## Docker 一键启动

安装 Docker Engine / Docker Desktop 和 Docker Compose v2（使用 Linux 容器），在项目根目录执行：

```sh
docker compose up -d
```

首次执行会从源码构建前端、Go 后端、Rust 网关镜像，拉取 PostgreSQL，并自动生成数据库密码、凭据加密主密钥和内部接口密钥；**无需先复制 `.env` 或手动建库**。首次构建需要下载依赖，耗时取决于网络与机器性能。

启动顺序为：生成密钥 → 数据库就绪 → 后端建表并通过健康检查 → 网关就绪 → Nginx。查看启动状态：

```sh
docker compose ps -a
docker compose logs -f --tail=100 backend gateway web
```

`init` 服务正常结束后显示 `Exited (0)`，其余四个服务应显示运行中。打开 **http://localhost:8080**，在登录页注册账号；第一个注册的用户自动成为管理员。

| 用途 | 默认地址 |
| --- | --- |
| 管理台 | `http://localhost:8080` |
| 管理 API | `http://localhost:8080/api/v1` |
| 客户端 Base URL | `http://localhost:8080/v1` |

页面和接口通过同一个 Nginx 端口访问，支持流式响应。数据库、Go 后端、Rust 网关只在 Compose 网络内访问，网关的 `/internal/*` 不通过 Nginx 对外转发。

### 数据挂载与备份

全部业务数据和部署密钥均绑定挂载到**宿主机项目目录**：

```text
data/
├── postgres/             # PostgreSQL：账号、分组、用户、设置、密钥、用量等
└── secrets/
    ├── postgres_password # 自动生成的数据库密码
    ├── master_key        # 账号凭据加密主密钥
    └── internal_token    # 后端与网关共享密钥
```

`docker compose down`、重新构建或删除容器均保留这些文件；再次启动会复用原有密钥。`data/` 已加入 Git 和镜像构建忽略列表。若发现已有数据库却缺失密钥，初始化会报错，避免生成新密钥后无法解密旧账号。

完整备份时先执行 `docker compose down`，再复制整个 `data/` 目录；恢复时在服务停止的情况下放回整个目录，再执行 `docker compose up -d`。**数据库和 `secrets/` 必须一起备份，主密钥丢失会导致账号凭据无法解密。** 不要在数据库运行时直接复制 `postgres/` 文件作为备份。文件备份恢复需保持 PostgreSQL 主版本一致，本配置固定使用 16。

容器日志输出到 `docker compose logs`，每个服务最多保留 3 个 10 MB 日志文件。后端配置绑定挂载自 `deploy/docker/backend.yaml`，修改后执行 `docker compose restart backend`。

### 修改端口、部署到服务器

可选：在根目录 `.env` 中添加以下配置（已有 `.env` 时只追加需要的项目）：

```dotenv
RELAYAI_PORT=8080
# 管理台生成客户端配置时使用的对外根地址，不带 /v1。
# 本机默认 http://localhost:<RELAYAI_PORT>；远程部署时改成实际 IP 或域名。
GATEWAY_PUBLIC_BASE_URL=http://192.168.1.10:8080
```

然后执行 `docker compose up -d` 应用配置。若前面还有 HTTPS 反向代理，将 `GATEWAY_PUBLIC_BASE_URL` 设为实际的 `https://域名`。

Compose 使用 `data/secrets/` 中的独立密钥和容器内数据库，不读取本地开发用的 `DATABASE_URL`、`RELAYAI_MASTER_KEY` 等环境变量。首次容器部署使用一个新库；原来本地 PostgreSQL 的数据需要另行迁移，迁移账号凭据时必须同时保留原加密主密钥。

更新源码后重新构建、启动：

```sh
docker compose up -d --build
```

### 镜像拉取失败（Docker Hub EOF / 超时）

`Get "https://registry-1.docker.io/v2/": EOF` 表示这次镜像仓库请求的连接中断，发生在应用启动之前。其他项目能启动时，也可能是使用了本机已缓存的镜像，不代表本次使用的不同镜像标签能拉取成功。先确认本机已有镜像，再单独检查本项目使用的标签：

```sh
sudo docker image ls
sudo docker pull postgres:16-alpine
curl -I --connect-timeout 10 --max-time 20 https://registry-1.docker.io/v2/
```

若服务器已经有其他项目使用的 PostgreSQL 16 镜像，可在根目录 `.env` 中指定同一标签，例如 `POSTGRES_IMAGE=postgres:16.9-bookworm`。这只复用镜像，relayAI 仍使用独立的 `data/postgres/` 数据目录，不连接其他项目的数据库。

Compose 已为本地构建的服务设置 `pull_policy: build`，避免尝试从镜像仓库下载项目自己的镜像。三个 Dockerfile 使用 BuildKit 内置语法，减少额外拉取 `docker/dockerfile` 前端镜像的需要。

Rust、Go、Node 依赖下载和 Docker 镜像拉取是不同的网络请求。Compose 默认分别使用 rsproxy、goproxy.cn、npmmirror；可在 `.env` 中覆盖构建参数：

```dotenv
CARGO_MIRROR=sparse+https://rsproxy.cn/index/
GOPROXY=https://goproxy.cn,direct
NPM_REGISTRY=https://registry.npmmirror.com
```

如需使用官方依赖源，设置 `CARGO_MIRROR=`、`GOPROXY=https://proxy.golang.org,direct`、`NPM_REGISTRY=https://registry.npmjs.org`。修改这些配置后执行 `sudo docker compose up -d --build`。

Docker Hub 的 `/v2/` 返回 HTTP 401 是正常的鉴权挑战，说明这次 HTTP 请求已到达仓库；EOF 或超时则需要检查服务器网络、DNS、Docker 的代理或镜像加速配置。即使 `curl` 成功，也应以 `docker pull` 是否成功为准，因为 Docker 守护进程和当前终端的代理配置可能不同。

确认缺失的基础镜像确实无法拉取后，再检查 Docker 的镜像加速配置。国内服务器可使用云厂商为该服务器提供的加速地址。例如腾讯云 CVM 可先检查 `https://mirror.ccs.tencentyun.com/v2/` 是否可达，再将该地址加入 `/etc/docker/daemon.json` 的 `registry-mirrors` 数组。保留文件中其他配置，校验后重启 Docker，再重试 `sudo docker pull postgres:16-alpine` 和 `sudo docker compose up -d`。重启 Docker 会影响该机器上的其他容器。

镜像加速需要覆盖 `postgres` 及 Dockerfile 使用的 Go、Rust、Node、Debian、Nginx 基础镜像，只替换 PostgreSQL 镜像地址不足以完成构建。项目自己的 `relayai-*:local` 镜像已设置 `pull_policy: build`，由本地源码构建。

`connect: permission denied` 通常表示当前用户无权访问 Docker socket。可先使用 `sudo docker ...`；需要免 sudo 时，由管理员将当前用户加入 `docker` 组并重新登录。这个权限问题与镜像仓库的 EOF 需要分别处理。

## 本地开发启动

### 1. 建库

```powershell
psql -U postgres -c "CREATE DATABASE relayai WITH ENCODING 'UTF8' TEMPLATE template0;"
```

表结构由 Go 侧迁移器自动创建（`AutoMigrate: true`），不需要手工执行 SQL。

### 2. 配置环境变量

复制 `.env.example` 为 `.env`，填入：

```dotenv
DATABASE_URL=postgresql://postgres:<密码>@localhost:5432/relayai
RELAYAI_MASTER_KEY=<32 字节 base64>
GATEWAY_INTERNAL_TOKEN=<任意随机串>
GATEWAY_PORT=18788
GATEWAY_BASE_URL=http://127.0.0.1:18788
```

生成主密钥：

```powershell
[Convert]::ToBase64String((1..32 | ForEach-Object { Get-Random -Max 256 }))
```

> **主密钥丢失 = 所有账号凭据作废**，必须重新登录。备份它。

### 3. 启动网关（终端 1）

```powershell
cd gateway
cargo run
```

看到 `已监听 http://127.0.0.1:18788` 即成功。端口被占用时会自动回退（18788 → 18789 → …），
**此时要把 `GATEWAY_BASE_URL` 改成实际端口**，否则管理面会打到别人身上。

> 为什么不是 8788 / 8790：这两个端口在本机已被其他常驻服务占用，用它们必然触发回退。

### 4. 启动管理面（终端 2）

```powershell
cd backend
go run backend.go
```

看到 `Starting server at 0.0.0.0:8888` 即成功。

### 5. 启动前端（终端 3）

```powershell
pnpm --dir frontend install
pnpm --dir frontend dev
```

访问 `http://localhost:5173/`。

## 验证

```powershell
# 前端
pnpm --dir frontend lint
pnpm --dir frontend typecheck
pnpm --dir frontend build

# 管理面
cd backend
go vet ./...
go build ./...

# 网关
cd gateway
cargo build
cargo test
```

端到端脚本（需要三个服务都在跑）：

```powershell
# 管理面 API（22 项）
node scripts/smoke-api.mjs

# 三协议（12 项）
node scripts/test-protocols.mjs http://127.0.0.1:18788

# Key 隔离（15 项）
node scripts/test-key-isolation.mjs http://127.0.0.1:18788 http://127.0.0.1:8888/api/v1

# 前端路由（28 项：14 路由 × 2 宽度）
node scripts/audit-routes.mjs http://localhost:5173
```

## 用起来

### 建渠道

管理台「渠道管理 → 新建渠道」。三种类型：

| 类型 | 说明 | 需要 |
| --- | --- | --- |
| WorkBuddy | 登录态反代（国内/国际） | 账号登录 |
| Trae | 两段式会话协议 | 账号登录（要粘贴回调地址） |
| 自定义供应商 | OpenAI / Anthropic 兼容端点 | 手填 API Key |

### 加账号

渠道详情页「添加账号」。WorkBuddy 走授权页轮询；Trae 需要把浏览器地址栏的回调地址粘回来；
自定义供应商直接填 Key。

### 建密钥

「密钥管理 → 新建密钥」。**明文只显示一次**，关掉就再也看不到。

密钥可绑：
- **渠道白名单**：这个 Key 只能用哪些渠道（空 = 不限）
- **模型白名单**：只能用哪些模型（空 = 不限）

两个都空 = 完全放开（升级前的行为）。

### 客户端接入

```
Base URL: http://127.0.0.1:18788/v1
API Key:  sk-...（管理台创建的那串）
```

三种协议都支持：

| 客户端类型 | 端点 |
| --- | --- |
| OpenAI 兼容（Cursor / Cherry Studio / 脚本） | `POST /v1/chat/completions` |
| Anthropic 兼容（Claude Code） | `POST /v1/messages` |
| OpenAI Responses（Codex） | `POST /v1/responses` |

模型清单：`GET /v1/models`（**按 Key 白名单过滤**）。

## 请求约定

- 所有非流式请求统一走 `frontend/src/utils/request.ts` 的 `APIRequest`。
- 响应信封 `{ code, success, msg, data }`，`code` 为 `0` 或 `200` 视为成功。
- 网关的错误信封按协议形状返回（OpenAI / Anthropic 各一套）。
- 403 与 503 严格区分：403 = Key 无权（改配置），503 = 暂时没容量（重试或补账号）。

## 路由与页面

| 路由 | 页面 |
| --- | --- |
| `/dashboard` | 仪表盘 |
| `/channels` `/channels/:id` | 渠道管理与详情 |
| `/accounts` `/accounts/:id` | 账号管理与详情 |
| `/status` | 渠道状态 |
| `/tasks` `/tasks/:channelId` | 任务总览与渠道任务 |
| `/usage` `/usage/:id` | 使用记录与详情 |
| `/stats` | 统计报表 |
| `/keys` | 密钥管理 |
| `/settings` | 设置 |

导航元数据在 `src/routes/navigation.ts`；「任务」组的二级菜单由
`GET /api/v1/tasks/channels` 动态决定（只列出有运营能力的渠道）。

## 当前边界

- **自用定位**：无用户注册、无充值、无多租户。
- **无向量能力**：不提供 `/v1/embeddings`。
- 视觉参考 FastGPT，只取 design token，布局仍是 `Aside + Header + Main`（见 [docs/visual-reference.md](docs/visual-reference.md)）。
- 不展示伪造数据；网关不可用时界面显示"状态未知"而不是编造默认值。

## 文档

| 文档 | 内容 |
| --- | --- |
| [SPEC.md](docs/SPEC.md) | **实施规格书**：不变量、数据模型、API 契约、页面规格、验收清单 |
| [product-plan.md](docs/product-plan.md) | 产品定位、导航信息架构、计费口径 |
| [migration-plan.md](docs/migration-plan.md) | 参考项目盘点、双服务边界、PG 迁移 |
| [task-module.md](docs/task-module.md) | 运营任务：菜单结构、执行语义 |
| [key-isolation.md](docs/key-isolation.md) | Key 的渠道 / 模型隔离 |
| [visual-reference.md](docs/visual-reference.md) | 视觉参考登记与 design token |
