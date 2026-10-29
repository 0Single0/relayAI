# 从 dynamic-island-web 迁移账号

本工具迁移 WorkBuddy 账号、登录凭据、国内/国际版本参数、账号启用状态、优先级、权重及并发覆盖。历史用量、下游 API Key、会话亲和、任务记录和代理不在本工具范围内。源库只读；存在代理绑定、重复 UID、无法解密的凭据或不支持的平台时中止，不静默丢弃账号。

导入时还会补齐缺失的 `channel_caps` 运营能力记录，否则任务渠道接口会过滤掉导入的分组。重复导入也会补齐缺失记录，但保留已有能力配置和账号凭据。预览结果的 `newCapabilityRecords` 表示待补齐的能力记录数量。

凭据在导入进程内用旧主密钥解密，再用目标部署现有的 `RELAYAI_MASTER_KEY` 加密。保留目标库所有已有账号；同 UID 只跳过，不覆盖凭据或分组。同名渠道的平台、区域或出站身份不一致时中止。整个写入过程使用事务，失败回滚，不自动修改表结构。

## 准备导入包

需要本机 Node.js 22.13+ 和 Go 1.25+。在项目根目录导出（将路径换成实际文件位置；输出文件必须尚不存在）：

```powershell
node scripts/export-legacy-accounts.mjs --database <state.db路径> --config <config.json路径> --out <导入包目录>/accounts.json
```

把旧主密钥文件复制到导入包目录，命名为 `gateway_master-key`。不要把原始配置文件放进包，因为其中可能含下游 API Key。编译 `backend/cmd/import-legacy` 的 Linux 静态二进制，分别命名为 `import-legacy-linux-amd64` 和 `import-legacy-linux-arm64`，把 `scripts/run-legacy-import.sh` 复制为包内 `run-import.sh`。

本机可先检查源数据，无需连接数据库：

```powershell
go -C backend run ./cmd/import-legacy -bundle <accounts.json路径> -source-key-file <旧主密钥路径> -validate-only
```

## 服务器执行

将导入包上传到服务器任意私有目录并解压。在 **relayAI 的 Compose 部署目录**执行脚本；脚本通过后端容器内已有的部署入口读取数据库连接与主密钥，服务器不需要安装 Node.js、Go 或 SQLite。

```bash
# 只预览：检查目标密钥、渠道冲突，统计新增和跳过数量，不写库。
sh /绝对路径/导入包/run-import.sh --preview

# 导入：先再次预览，再备份目标 PostgreSQL，事务写入，通知网关重新加载。
sh /绝对路径/导入包/run-import.sh --apply
```

脚本支持当前项目默认的 `backend`、`postgres`、`gateway` Compose 服务和 `relayai` 数据库用户，支持 Linux amd64/arm64。自定义部署应先适配脚本。备份保存在当前部署目录的 `data/legacy-import-backups/<时间>-<PID>/relayai-before-import.dump`，位于已忽略的 `data/` 目录内。重复运行会跳过同 UID 账号；报告中的 `newAccounts` 在预览时表示预计新增数量，在实际写入成功后表示已提交数量。

导入包包含账号密文和旧主密钥，应按完整账号凭据保管，不提交 Git 或公开分享。脚本退出时清除容器里的临时导入文件，服务器及本机的导入包由操作者在完成后清理。

若输出提示账号已经提交但网关重载失败，执行 `docker compose restart gateway` 后再检查账号页。迁移成功仅代表凭据完整入库，实际登录有效性需要在账号页执行刷新或探活确认。旧服务与新服务同时刷新同一账号可能轮换刷新令牌；迁移后应安排切换，避免两边同时使用这批账号。

旧 `scripts/import-legacy-sqlite.mjs` 面向早期数据库结构，且会丢弃凭据，不适合本次迁移。

## 修复早期导入后的任务页空白

若账号管理已有账号，但任务页显示“全部账号（0）”，早期导入工具可能没有创建 `channel_caps`。可以重新运行修正后的导入包，或在部署目录执行修复 SQL：

```bash
sudo docker compose exec -T postgres psql -U relayai -d relayai -v ON_ERROR_STOP=1 < scripts/repair-legacy-channel-caps.sql
```

该 SQL 仅为存在 WorkBuddy 账号的 `wb-cn` / `wb-intl` 分组补齐缺失能力记录，不修改已有记录；国内版开启成长任务、每日动作与签到，国际版开启每日动作与签到，与现有适配器声明一致。执行后刷新任务页，无需重建镜像或重新导入账号。能力声明只决定页面入口，真实任务及签到是否可用仍取决于上游返回。

账号导入后还需要在管理台“模型广场”点击“从渠道同步”，再刷新账号页。网关重载不会立即把目录写入管理台的 `models` 表；这与任务能力记录是两条独立流程。
