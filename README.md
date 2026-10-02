# PiDock

Desktop cockpit & cloud mirror for the [pi coding agent](https://pi.dev).

本地跑 pi agent 的桌面工具（对标 Codex 桌面版），所有会话状态实时同步到远程 server，
通过 web 端以账号身份查看与完全控制（发 prompt / steer / abort / 工具审批）。

**完整架构设计、决策理由与踩坑记录见 [docs/DESIGN.md](docs/DESIGN.md)。**

## 架构（M1 已落地部分标 ✔）

```
Tauri 2 (Rust core + Vue UI)
  ├─ host-supervisor ── stdio JSONL ── pi-host (bun compile 单 exe)
  │                                     └─ 嵌入 @earendil-works/pi-coding-agent
  │                                        多 session / 三通道事件 / 配置面
  └─ sync-agent ── WSS ── axum server ── Kafka ── ingest ── MongoDB
                                          └─ Redis pub/sub ── web WS 推送
```

## 目录

```
crates/pidock-protocol/  统一线路协议（Rust serde + ts-rs 导出 TS）
packages/protocol/       协议 payload 权威 TS 定义（host 与 ui 共用）
host/                    pi-host：stdio 守护进程，嵌入 pi SDK
  src/pool.ts            多 session 池 + 事件映射 + 快照节流 + 回放
  scripts/smoke.ts       M1 冒烟（spawn 子进程全链路 + 真模型调用）
  scripts/extension-smoke.ts  jiti 扩展 × bun-compile 验证
packages/ui/             桌面与 web 共用的 Vue 组件 + DataBus + store
apps/desktop/            Tauri 2 桌面应用
  src/bus.ts             DataBus 的 Tauri IPC 适配器
  src-tauri/             Rust：host supervisor（spawn/stdio 桥/进程树清理）
apps/web|server/         web 前端 / axum server（M4+）
docker/                  kafka + mongo + redis + rustfs + server（M4+）
```

## 事件模型

- **delta 级**（仅本地 UI）：`message_delta`（thinking/text token 流）
- **快照**（仅推送不落库）：`message_snapshot`（2s 内联节流，web 近流式体验）
- **消息级**（上云）：`message_complete` / `session_meta` / `compaction_summary`，
  `persist: true`，`seq` = pi session JSONL 条目序号（幂等键 `(session_id, seq, kind)`）
- **状态**（临时）：`agent_state_changed` / `tool_execution_*` / `queue_changed` /
  `auto_retry` / `compaction_lifecycle`

大 payload（>256KB）自动附件化：sha256 内容寻址 + 16KB 预览，全文走对象存储（RustFS/S3）。

## M6 状态（数据面与打磨）

- [x] 附件数据面：host 截断时全文落盘（`~/.pi/agent/pidock/attachments/<sha256>`）→
      sync-agent 检测附件引用 → server SigV4 presign → 桌面直传 RustFS →
      web 凭 presigned GET 查看全文（ToolCard「查看全文」按钮，归属校验 403）
- [x] compaction 重同步：pi 压缩会话后 JSONL 变短 → host 发 `session_resynced` →
      server 清空该会话事件 → 重排水序回填；ingest 单 worker 保序
- [x] e2e 扩到 20/20（新增：presign 内容寻址、presigned PUT 上传、下载往返、
      越权 403、压缩重建）
- [x] 修复：SigV4 密钥派生漏了 "AWS4" 前缀（签名全 403 的根因）
- [x] server 可选托管 web 静态资源（`PIDOCK_WEB_DIR`），Docker 镜像内置
- [ ] NSIS 安装包（pi-host sidecar 打包进安装器）

## M3 状态（设置中心）

- [x] host `config.*` 全套命令：settings 读写（白名单键）、providers 列表 +
      API Key 存取（auth.json）、models 列表 + 默认模型、extensions/skills
      发现与启停（pi 的 `-path` glob 排除机制，写入 settings.json）、mcp.json
      读写（pi 的 MCP 由扩展提供，配置文件按扩展约定）
- [x] 桌面 SettingsView（模型与密钥 / 扩展 / 技能 / MCP 四个 tab）
- [x] config 冒烟 20/20 PASS（`scripts/config-smoke.ts`，隔离 agent 目录）
- [x] 修复：Windows 下 npm 垫片命令解析（bun.cmd → cmd /C 包装）；应用退出
      进程挂起（Exit 后强制 process.exit）

## M1 验证状态（2026-09-29）

- [x] smoke 12/12 PASS：spawn → session.create → 真模型流式 → 持久事件 → 回放 → close
- [x] `bun build --compile` 单文件 `target/pidock-host.exe`（93MB）smoke PASS
- [x] jiti TS 扩展在编译产物内加载执行 PASS（`scripts/extension-smoke.ts`）
- [x] console.log 重定向 stderr（防扩展污染协议通道）

## M2 状态（桌面最小闭环）

- [x] packages/ui：DataBus 接口 + reactive store（delta/snapshot/complete 合并、
      工具实时卡片、历史回放）+ 组件（SessionSidebar/ChatView/MessageItem/
      ToolCard/Composer/StatePill）
- [x] apps/desktop 前端：Tauri IPC DataBus 适配器 + App 布局，vue-tsc 通过，vite 构建通过
- [x] apps/desktop/src-tauri：Supervisor（spawn pi-host、请求-响应 oneshot 桥、
      `pidock:event` 事件桥、退出时 taskkill /T /F 进程树清理），cargo check/build 通过
- [x] host 增强：message_complete 携带 turnId，前端流式→落库精确合并

启动桌面应用（开发模式）：

```bash
pnpm --filter @pidock/desktop tauri dev
```

环境变量（默认即可用）：`PIDOCK_HOST_CMD`（默认 bun）、`PIDOCK_HOST_ARGS`（默认
src/main.ts）、`PIDOCK_HOST_DIR`（默认 <repo>/host）。生产模式用
`bun build ./src/main.ts --compile --outfile target/pidock-host.exe` 打单文件后
由 supervisor 以 sidecar 方式拉起（打包进安装包在 M6）。

## UI 2.0

- **双主题**：右上角 ☀/☾ 切换，全量 CSS 变量（`packages/ui/src/styles/tokens.css`），Naive UI 桥接
- **消息渲染**：markdown-it + highlight.js（16 语言）+ DOMPurify；代码块带语言标签与复制按钮；
  edit/write 工具渲染为红绿 diff 视图；消息一键复制；思考过程折叠
- **WorkBuddy/ZCode 风格布局**：侧栏「新建会话」主按钮 + 会话按项目分组折叠 + 搜索 +
  相对时间戳（刚刚/N分钟前/昨天）；无会话时**居中问候语 + 大输入卡片 + 快捷指令 chips**
- Composer 卡片化：模型标签、圆形发送按钮、离线置灰

## 本地开发启动（不依赖任何会话）

```bash
# 基础设施（WSL docker，容器 healthy 后即用）
wsl -u root -e bash -lc "cd /mnt/d/project/rust-project/pi-desktop/docker && docker compose up -d kafka mongo redis rustfs"

# server（Kafka 管道）
set PIDOCK_MONGO_URI=mongodb://localhost:27017&& set PIDOCK_REDIS_URL=redis://localhost:6379&& set PIDOCK_KAFKA_BROKERS=localhost:9092&& set PIDOCK_PIPELINE=kafka&& cargo run -p pidock-server

# web（:5174）
pnpm --filter @pidock/web dev

# 桌面（:dev 模式跑 bun 源码 host）
pnpm --filter @pidock/desktop tauri dev

# 端到端验收（server 需在跑）
pnpm --filter @pidock/host e2e:server
```

## 安装包（发布形态）

```bash
pnpm bundle:app
# 1. pi-host 编译为单 exe → apps/desktop/src-tauri/binaries/pidock-host-<triple>.exe
# 2. tauri build → NSIS 安装包输出 apps/desktop/src-tauri/target/release/bundle/nsis/
```

安装后的桌面端：release 模式自动 spawn 同目录的 `pidock-host.exe`（sidecar），
不再依赖本机 Node/Bun；云同步、附件、远程控制全部可用（server 地址在 ☁ 面板配置）。

## 常用命令

```bash
pnpm install
pnpm --filter @pidock/host typecheck        # host 类型检查
pnpm --filter @pidock/host smoke            # M1 全链路冒烟（发起真实 LLM 调用）
pnpm --filter @pidock/host smoke:extension  # jiti × bun-compile 验证
pnpm --filter @pidock/host smoke:config     # M3 配置面冒烟（隔离 agent 目录）
cargo test -p pidock-protocol               # 协议 crate 测试 + 导出 TS 绑定
pnpm --filter @pidock/desktop typecheck     # 桌面前端类型检查（vue-tsc）
pnpm --filter @pidock/desktop tauri dev     # 启动桌面应用
```
