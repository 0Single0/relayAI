# relayAI Frontend

React 19 + TypeScript + Vite 8 + Tailwind CSS 4 + shadcn/ui + Lucide + react-router-dom.

项目级说明、目录结构和联调方式见仓库根目录 [README.md](../README.md) 与 [AGENTS.md](../AGENTS.md)。

## Commands

```powershell
pnpm install
pnpm dev        # http://127.0.0.1:5173
pnpm lint
pnpm typecheck
pnpm build
pnpm preview    # http://127.0.0.1:4173
```

开发与预览环境都会把 `/api` 代理到 `http://127.0.0.1:8888`。API 地址由 `.env` 中的 `VITE_BASE_URL` 控制，默认 `/api/v1`。

## Structure

```text
src/
├─ api/          # 按业务分组的接口
├─ components/
│  ├─ common/    # AppShell、MyBox 等应用级组件
│  └─ ui/        # shadcn/ui 组件
├─ hooks/
├─ pages/        # 目录级页面
├─ routes/       # 路由与导航元数据
├─ store/
├─ styles/       # Tailwind 主题与全局样式
└─ utils/        # request.ts、auth.ts
```
