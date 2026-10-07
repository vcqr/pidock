# PiDock

[PiDock](https://github.com/vcqr/pidock) 是 [pi coding agent](https://pi.dev) 的桌面驾驶舱与云端镜像。

本地运行 pi agent 的桌面工具（对标 Codex 桌面版），所有会话状态实时同步到远程 server，
在 web 端以账号身份查看并完全控制（发 prompt / steer / abort / 工具审批）。

**完整架构设计、决策理由与踩坑记录见 [docs/DESIGN.md](docs/DESIGN.md)。**

## 项目介绍

### 核心能力

- **桌面端**（Tauri 2 + Vue 3）：多会话管理与项目分组、流式输出、工具实时卡片、
  edit/write 红绿 diff 视图、双主题、设置中心（Providers / 模型 / 扩展 / 技能 / MCP）、
  专家智能体、系统托盘
- **云同步**：桌面事件经 sync-agent 上报，Kafka 管道 → ingest → MongoDB 持久化，
  Redis pub/sub → WebSocket 推送到 web 端，近流式体验
- **Web 端**：登录后按账号查看全部节点与会话，可远程发消息、插话、中止、审批工具调用，
  支持节点卡片墙、详情浮层、头部搜索过滤
- **附件数据面**：大 payload（>256KB）自动附件化，sha256 内容寻址 + 预览，
  全文经 SigV4 presign 直传对象存储（RustFS / S3）
- **发布**：GitHub Actions 多平台矩阵，tag 触发 Windows / macOS / Linux（x64 + ARM64）
  安装包构建，pi-host 以 sidecar 形式打包，安装后不依赖本机 Node/Bun

### 架构

```
Tauri 2 (Rust core + Vue UI)
  ├─ host-supervisor ── stdio JSONL ── pi-host (bun compile 单 exe)
  │                                     └─ 嵌入 @earendil-works/pi-coding-agent
  │                                        多 session / 三通道事件 / 配置面
  └─ sync-agent ── WSS ── axum server ── Kafka ── ingest ── MongoDB
                                          └─ Redis pub/sub ── web WS 推送
```

事件模型分三通道：delta 级（`message_delta`，仅本地 UI）、快照级
（`message_snapshot`，2s 节流推 web 不落库）、消息级（`message_complete` /
`session_meta` 等，落库幂等键 `(session_id, seq, kind)`）。细节见 DESIGN.md。

### 目录结构

```
crates/pidock-protocol/  统一线路协议（Rust serde + ts-rs 导出 TS）
packages/protocol/       协议 payload 权威 TS 定义（host 与 ui 共用）
host/                    pi-host：stdio 守护进程，嵌入 pi SDK
packages/ui/             桌面与 web 共用的 Vue 组件 + DataBus + store
apps/desktop/            Tauri 2 桌面应用（src-tauri 为 Rust supervisor）
apps/web/                web 前端（Vue 3）
apps/server/             axum server（Rust：WS 网关 / Kafka 管道 / REST / 静态托管）
docker/                  kafka + mongo + redis + rustfs + server 编排
docs/                    架构设计文档与代码审查报告
```

## 运行与部署

### 前置条件

- Node.js ≥ 22 与 pnpm 10（`corepack` 可自动管理）
- [Bun](https://bun.sh)（运行 pi-host 源码与编译单文件）
- Rust stable（`rustup`，edition 2021）
- Docker（Windows 建议 WSL 内的 docker）
- 可用的 LLM API Key（pi provider，如 Anthropic / OpenAI，用于真实会话）

### 本地运行（开发模式）

```bash
# 1. 基础设施（容器 healthy 后即用）
cd docker && docker compose up -d kafka mongo redis rustfs && cd ..

# 2. server（:8080；Windows cmd 示例，bash 用 export）
set PIDOCK_MONGO_URI=mongodb://localhost:27017&& set PIDOCK_REDIS_URL=redis://localhost:6379&& set PIDOCK_KAFKA_BROKERS=localhost:9092&& cargo run -p pidock-server

# 3. web（:5174）
pnpm --filter @pidock/web dev

# 4. 桌面应用（开发模式跑 bun 源码 host）
pnpm --filter @pidock/desktop tauri dev
```

桌面端环境变量（默认即可用）：`PIDOCK_HOST_CMD`（默认 bun）、
`PIDOCK_HOST_ARGS`（默认 src/main.ts）、`PIDOCK_HOST_DIR`（默认 `<repo>/host`）。

- web 端登录页填 server 地址（默认 `http://localhost:8080`）注册/登录
- 桌面端 ☁ 面板配置 server 地址开启云同步

### Docker 部署 server（生产形态）

```bash
# server 镜像内置 web 静态资源（PIDOCK_WEB_DIR），单容器同时提供 REST/WS 与 web
pnpm --filter @pidock/web build          # 先构建 web（Dockerfile 会 COPY dist）
cd docker && docker compose up -d --build server
```

可配置项（docker-compose 已给默认值）：`PIDOCK_JWT_SECRET`、
`RUSTFS_ROOT_USER` / `RUSTFS_ROOT_PASSWORD`、`PIDOCK_PIPELINE`（kafka，可选 redis 降级）。

### 打包桌面安装包

```bash
pnpm bundle:app
# 1. pi-host 编译为单 exe → apps/desktop/src-tauri/binaries/pidock-host-<triple>.exe
# 2. tauri build → NSIS 安装包输出 apps/desktop/src-tauri/target/release/bundle/nsis/
```

安装后的桌面端自动 spawn 同目录的 `pidock-host.exe`（sidecar），云同步、附件、
远程控制全部可用。

### 发布

打 tag（`v*`）推送即触发 GitHub Actions Release 工作流，构建 6 平台矩阵
（Windows x64/ARM64、macOS Intel/Apple Silicon、Linux x64/ARM64）并上传产物。

## 开发

### 常用命令

```bash
pnpm install                               # 安装依赖
pnpm build                                 # 全仓构建
pnpm -r typecheck                          # 全仓类型检查

pnpm --filter @pidock/host smoke           # host 全链路冒烟（发起真实 LLM 调用）
pnpm --filter @pidock/host smoke:config    # 配置面冒烟（隔离 agent 目录）
pnpm --filter @pidock/host smoke:extension # jiti 扩展 × bun-compile 验证
pnpm --filter @pidock/host e2e:server      # 端到端验收（server 需在跑）

cargo test -p pidock-protocol              # 协议 crate 测试 + 导出 TS 绑定
cargo run -p pidock-server                 # 启动 server
```

### 协议同步

线路协议权威定义在 `crates/pidock-protocol`（Rust），通过 ts-rs 在 `cargo test` 时
导出 TS 绑定到 `packages/protocol`，host / ui / web / desktop 共用。改协议后跑
`pnpm protocol:gen`（即 `cargo test -p pidock-protocol`）重新生成。

### CI

GitHub Actions 双工作流：

- **CI**（push main / PR）：全仓 typecheck、ui/web 构建、cargo fmt --check、clippy、Rust 测试
- **Release**（tag `v*`）：6 平台矩阵构建并上传安装包

## 许可证

本项目基于 [MIT License](LICENSE) 开源。

```
MIT License

Copyright (c) 2026 vcqr

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
