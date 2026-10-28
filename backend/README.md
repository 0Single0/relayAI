# relayAI Backend

go-zero REST API 服务，模块名 `relayai/backend`，默认监听 `0.0.0.0:8888`。

## Run

```powershell
go run backend.go
```

健康检查：

```text
GET http://127.0.0.1:8888/api/v1/health
```

## Layout

```text
backend/
├─ backend.api          # API 定义（syntax v1）
├─ backend.go           # 服务入口
├─ etc/                 # 本地配置
└─ internal/
   ├─ config/
   ├─ handler/
   ├─ logic/
   ├─ svc/
   └─ types/
```

## Adding an endpoint

1. 在 `backend.api` 中新增 type 和 route，使用 RESTful 路径与 HTTP 方法。
2. 运行 `goctl api format --dir .` 校验并格式化。
3. 运行 `goctl api go --api backend.api --dir .` 重新生成 handler、logic、types 和 routes。
4. 在生成的 logic 文件中实现业务逻辑。
5. 运行 `gofmt -w .`、`go vet ./...`、`go build ./...`。

当前未接入数据库或任何外部服务。
