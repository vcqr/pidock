# PiDock 设计文档

> 桌面端 + 云同步 + Web 远程控制的 pi coding agent 工作台。
> 本文档记录最终架构、关键决策的理由、协议规范与踩坑记录。
> 快速上手见根目录 [README.md](../README.md)。

## 1. 目标

- 本地跑 pi agent 的桌面工具（对标 Codex 桌面版）
- 本地 agent 的**所有状态**实时同步到远程 server（基于账号）
- Web 端与桌面端**界面一模一样**，且具备**完全控制**能力
  （发 prompt / steer / abort / 查看全文）
- 多机器支持；桌面离线时 web 只读（仅在线可控）

## 2. 总体架构（最终版）

```
┌─ 桌面 (Windows, Tauri 2) ────────────────────────────────┐
│  Vue UI（packages/ui，与 web 共用）+ 设置中心 + ☁ 同步面板 │
│  Rust Core                                                │
│   ├─ host-supervisor：spawn pi-host（dev=bun / prod=sidecar）│
│   ├─ sync-agent：WSS 上行事件 / 下行命令 / 附件直传         │
│   └─ pidock-protocol（serde + ts-rs 导出 TS 绑定）          │
└────┬──────────────────────┬───────────────────────────────┘
     │ stdio JSONL 统一协议   │ HTTPS：presign 上传 RustFS
     ▼                      ▼
┌─ pi-host（TS → bun compile 单 exe）──┐   ┌─ RustFS (S3) ─┐
│  嵌入 @earendil-works/pi-coding-agent │   │ 附件（sha256） │
│   多 session / 三通道事件 / 配置面     │   └────────────────┘
│   ctx.ui 转发桌面与 web                │
└────┬──────────────────────────────────┘
     │ WSS（上行消息级事件 / 下行命令）
     ▼
┌─ pidock-server（axum，本地或 docker）─────────────────────┐
│  HTTP：auth(JWT) / machines / sessions / events / commands │
│        /attachments(presign, 归属校验)                     │
│  WS /desktop（机器注册+心跳+命令下发）  WS /web（pub/sub 推送）│
│  Kafka 管道（PIDOCK_PIPELINE=kafka|redis 可切换）           │
│  ingest：保序消费 → Mongo 幂等落库 → Redis pub/sub 推 web    │
│  Redis：在线注册(90s TTL)/presence/refresh token/seq cursor │
│  MongoDB：users/machines/sessions/session_events/commands   │
│           /attachment_meta                                 │
└─────────────────────────────────────────────────────────────┘
```

## 3. 关键决策与理由

### 3.1 pi 集成：SDK 自建 host，而非 RPC 模式

| 能力 | RPC 模式 | SDK 自建 pi-host |
|---|---|---|
| 多 session | 每 session 一个进程 | 单进程多 session（SessionManager）|
| 事件流 | 固定事件集 | `session.subscribe()` 全量 |
| Extensions/Skills 管理 | ❌ | ✅ 发现/启停/热重载 + ctx.ui 转发 |
| Providers/Models/Keys | ❌ | ✅ config.* 全套 |
| 自定义工具注册 | ❌ | ✅ customTools |

RPC 是固定协议面（为语言无关集成设计），配置管理完全不可达。自建 host
（约 2k 行 TS）嵌入 SDK，对上层屏蔽 pi 版本差异。

### 3.2 打包：`bun build --compile` 单文件

pi 官方自己的 `build:binary` 就是 bun compile。pi-host 编译为 ~93MB 单 exe：
- M1 验证了 **jiti（TS 扩展加载）在编译产物内正常工作**
- 作为 Tauri sidecar 打进 NSIS 安装包，用户机器零依赖

### 3.3 统一协议与事件三通道（核心设计）

`crates/pidock-protocol`（serde + ts-rs 导出）+ `packages/protocol`（TS 权威 payload 定义）。

```
Envelope { event_id, session_id, seq?, persist, ts, kind, payload }
```

| 通道 | 事件 | 去向 |
|---|---|---|
| delta（打字机） | message_delta | 仅桌面 UI，**不上云** |
| 快照 | message_snapshot（2s 内联节流） | 推送不落库 |
| 持久 | message_complete / session_meta / compaction_summary | 落库 + 上云 + 推送 |
| 状态 | agent_state_changed / tool_execution_* / approval_request / auto_retry / compaction_lifecycle / session_resynced / command_result | 推送不落库（resynced 除外） |

**seq 锚定**：persist 事件的 `seq` = pi session JSONL 条目序号。
以 `sessionManager.getEntries()` 为唯一事实源做 diff 排水（drain）——
live 与 replay 共用同一"条目序号→seq"映射，永不失配。
幂等键 `(session_id, seq)`（Mongo 唯一索引）支撑断线重放与重同步。

> ⚠️ pi 的 `entry_appended` 事件只对扩展自定义条目触发，常规消息落盘不发
> ——所以必须用 drain 方案而不是订阅该事件。

### 3.4 消息级上云 + 快照节流

- 桌面本地 UI 吃 delta（打字机）；上云的是消息级 + 快照
- 快照：host 在**每个 delta 到达时**判断距上次快照 ≥2s 才发（内联节流）
  ——不用定时器：定时器会被短对话抢先清除（实测踩坑）
- web 端因此有"接近流式"的体验（2s 粒度），且流量可控

### 3.5 状态三层分工

- **Redis = 运行时状态**（易失可自愈，不开持久化）：
  在线机器注册（HASH EX 90s + 30s 心跳续期）、presence、refresh token（设备级、可吊销）、
  事件去重（SETNX）、seq cursor、pub/sub 总线 `user:{uid}:events`
- **MongoDB = 持久事实**：账号、机器档案、sessions（状态机）、session_events（append-only）、
  commands（审计）、attachment_meta
- **Kafka = 纯事件管道**：削峰/解耦，retention 7 天，不存状态
  （`PIDOCK_PIPELINE=redis` 可平替为 Redis Streams——单 key 有序=单分区有序，
  按 machine_id 哈希分片即可，ingest 接口已抽象）

### 3.6 附件：数据面/控制面分离

- 单事件 payload >256KB → host 截断为 16KB 预览 + `{attachment_id=sha256, size}`
  ，**全文落盘** `~/.pi/agent/pidock/attachments/<sha256>`（内容寻址）
- sync-agent 检测事件中的附件引用 → server presign（SigV4）→ **直传 RustFS**
  （不经 server 内存）；sha256 秒传去重
- web「查看全文」→ GET url（归属校验 user_id）→ presigned GET（15min）
- SigV4 手写实现（无 aws-sdk 依赖），**密钥派生必须带 "AWS4" 前缀**
  （见踩坑 §9.1）

### 3.7 DataBus 抽象 = "界面一模一样"的实现关键

```ts
interface DataBus {
  request(method, params): Promise;      // 桌面=Tauri IPC→本地 host
  onEvent(handler): Unsubscribe;         // 桌面=Tauri event；web=WS
  loadHistory(sessionId, afterSeq?);     // 桌面=host 回放；web=REST
  loadAttachment(id): Promise<string>;   // 桌面=host 文件；web=presigned GET
}
```

`packages/ui` 的组件与 store 只认 DataBus。桌面与 web 的差异全部封装在
两个适配器（`apps/desktop/src/bus.ts`、`apps/web/src/bus.ts`）里。

会话列表更新是**纯推送**：server ingest 每次落库后把 session 快照以
`{"ctrl":"session_upserted"}` 推给该用户全部 web 连接，前端 `upsertSessionSummary`
合并（不回拉）。桌面本地列表靠"未知 session_id 触发一次本地刷新"。

### 3.8 命令链路与 token 生命周期

- 命令：web `POST /commands` → gateway 查 Redis 在线 → 桌面 WS 下发 →
  sync-agent 翻译成 host 请求 → `command_result` 事件回环（前端按 command_id
  关联等待）→ 全量落 Mongo 审计；离线机器直接 `refused_offline`
- JWT：access 15min（无状态）+ refresh 30d（Redis 设备级存储，**每次刷新轮换**）
- ⚠️ 轮换后必须立即持久化新 refresh token——桌面端曾因未持久化而永远 401（踩坑 §9.5）
- 威胁声明：**web 完全控制 = 账号即开发机执行权**。v1 个人账号接受；
  TOTP 与设备管理页留接口（refresh token 已按设备存储、可吊销）

### 3.9 compaction 重同步

pi 压缩会话会重写 JSONL → 条目索引偏移。host drain 时检测
`entries.length < lastSeq` → 发 `session_resynced`（临时事件）→ server
`delete_many` 该会话全部事件 → host 从 seq=1 重新回放。**ingest 单 worker
严格保序**（曾经每帧 spawn 导致标记晚于重建事件到达，已修）。

## 4. 协议参考

### 方法（core → host；web 经 REST/命令映射到同一套）

`ping` / `session.create|list|open|close|events` / `agent.prompt|steer|follow_up|abort` /
`config.get|settings.set|providers.list|provider.set_key|provider.remove_key|models.list|models.set_default|extensions.list|extensions.toggle|skills.list|skills.toggle|mcp.get|mcp.set` / `attachment.get`

### 事件 kind

persist：`message_complete` / `session_meta` / `compaction_summary`
ephemeral：`message_delta` / `message_snapshot` / `tool_execution_start|update|end` /
`agent_state_changed` / `queue_changed` / `auto_retry` / `compaction_lifecycle` /
`approval_request` / `command_result` / `error` / `session_resynced`

### 上行过滤（sync-agent → 云）

persist 全量 + 临时事件白名单（snapshot/状态类）。`message_delta`、
`tool_execution_update` 不上云。

## 5. 数据模型

**MongoDB**
```
users            {_id, email(唯一), password_hash(argon2), created_at}
machines         {_id, user_id, hostname, os, version, last_seen, created_at}
sessions         {_id=session_id, user_id, machine_id, status(running|idle|
                 waiting_approval|done|error), title, cwd, model, created_at, updated_at}
session_events   {machine_id, session_id, seq, event_id, kind, payload(JSON 字符串),
                 ts, ingested_at}   唯一索引 (session_id, seq)
commands         {command_id, user_id, machine_id, session_id, type, payload, status, ts}
attachment_meta  {_id=sha256, user_id, size, created_at}
```

**Redis**
```
online:machine:{id}   HASH{user_id,hostname,os,version,connected_at} EX 90s（30s 心跳续期）
online:user:{uid}     SET 在线机器
auth:refresh:{jti}    → user_id, TTL 30d（轮换制，可吊销）
pub/sub               user:{uid}:events（session 事件 + machine_status + session_upserted）
```

## 6. 部署

| 形态 | 说明 |
|---|---|
| 本机开发 | docker compose 起 kafka/mongo/redis/rustfs + 本地 `cargo run -p pidock-server` + `pnpm --filter @pidock/web dev` |
| docker 全栈 | `docker compose up -d`（server 镜像内置 web 静态资源，8080 一个端口） |
| VPS | .env 改凭据 + Caddy WSS + RustFS→云 S3（仅改 endpoint/密钥）+ Kafka SASL |
| 桌面安装包 | `pnpm bundle:app` → NSIS 安装包（含 pi-host sidecar，零依赖） |

## 7. 测试与验收

| 脚本 | 覆盖 |
|---|---|
| `host/scripts/smoke.ts` | host 全链路：spawn→session→真模型流式→持久事件→回放（12 项）|
| `host/scripts/extension-smoke.ts` | jiti × bun-compile |
| `host/scripts/config-smoke.ts` | config.* 全套（隔离 agent 目录，12 项）|
| `server/scripts/fake-desktop.ts` | server 端到端（auth/WS/Kafka/Mongo/push/命令/附件/越权/压缩重同步，20 项）|

## 8. 已知限制与路线图

- 设备管理 UI（auth 层已支持按设备吊销）
- 图片附件的云端预览（文本附件已完整支持）
- 多机器重命名/备注
- 会话列表分页（当前 200 条上限）
- 多实例 server（Redis route 表已预留，命令路由目前单实例内存版）
- 端到端加密（v2，全量明文是 M3 时的明确取舍）

## 9. 踩坑记录（按性价比排序的教训）

1. **SigV4 密钥派生必须带 "AWS4" 前缀**：`HMAC("AWS4"+secret, date)`。
   漏掉 → 所有签名 403 SignatureDoesNotMatch。排查方法：aws-cli 做已知正确
   对照 + 官方测试向量逐步二分（自写测试时警惕 JS 多参调用静默丢参）。
2. **`entry_appended` 只对扩展自定义条目触发**：常规消息落盘不发该事件。
   必须以 `getEntries()` diff 为事实源。
3. **定时器节流输给快模型**：2s 定时器在整轮流式 <2s 结束时被清。
   改内联时间节流（delta 到达时判断）。
4. **Vue 3 裸对象引用**：push 进响应式数组后继续持有裸引用，后续字段赋值
   全部绕过代理不触发渲染。push 后必须取回代理（`arr[arr.length-1]`）。
5. **JWT refresh 轮换必须立即持久化**：服务端轮换后旧 token 作废，
   客户端不保存新 token → 永远 401。
6. **Windows npm 全局垫片**：`bun` 只有 `.cmd`（真实 exe 在 node_modules 里），
   Rust `CreateProcess` 只找 `.exe`。spawn 前需按 PATHEXT 语义手动解析，
   `.cmd` 用 `cmd /C` 包装（退出时 taskkill /T 兜底）。
7. **Tauri 退出挂起**：RunEvent::Exit 清理正常但进程不退 → 事件循环后
   `std::process::exit(0)`。
8. **扩展的 console.log 会污染 stdio JSONL 协议**：host 启动时把 console
   全部重定向到 stderr。
9. **WSL2 docker 无默认路由/代理不可达**：镜像走国内 mirror 拉，
   Dockerfile 内 apt/cargo 也切清华源。
10. **Kafka 消费组再均衡延迟**：实例被杀后默认 45s 才释放分区。
    `session.timeout.ms=10000` + 心跳 3s。
11. **pi `entry_appended` 语义**（见 §3.3）与 **jiti 在 bun compile 内可用**
    （M1 已验证，非问题，记录在案）。
