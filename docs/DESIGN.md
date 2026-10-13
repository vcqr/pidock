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
│  pidock-core（crates/，与 UI 壳解耦）                      │
│   ├─ host-supervisor：spawn pi-host（dev=bun / prod=sidecar）│
│   ├─ sync-agent：WSS 上行事件 / 下行命令 / 附件直传         │
│   ├─ scheduler：定时任务（本地时区 cron）                   │
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

### 2.1 双壳：Tauri 桌面壳与 webhost 无头壳

核心运行时（supervisor / 调度器 / 云同步）不依赖 Tauri，事件统一走 tokio
broadcast（`CoreCtx.events`），命令统一走 `CoreCtx::handle(cmd, args)`——
两个壳都是薄转换层：

- **Tauri 壳**（`apps/desktop/src-tauri`）：管理 webview 窗口/托盘/原生对话框；
  事件泵把广播转发到 webview（`pidock:event`），命令包装为 `#[tauri::command]`。
- **webhost 壳**（`apps/webhost`）：axum 静态托管同一套桌面前端（release 编译期
  嵌入 dist），`/ws` 把浏览器 `ipc(cmd,args)` 帧桥到 `CoreCtx::handle`、把事件
  广播推给每个客户端——与 `invoke`/`pidock:event` 同语义，前端 `ipc.ts` 按
  `__TAURI_INTERNALS__` 有无自动选传输。适合 Docker/NAS 部署：`docker compose
  --profile webhost up -d --build webhost`。

选型理由：复用整套桌面 UI 与核心（web 控制台那条链路面向「远程查看云端镜像」，
依赖 Kafka/Mongo/Redis；webhost 面向「这台机器的 agent 直接开 Web」，零外部
依赖），增量成本只有一个 axum 壳 + 前端 WS 传输层。鉴权用可选共享令牌
（`PIDOCK_WEB_TOKEN`，WS 握手后以 4401 关闭区分「令牌错」与「不可达」）。

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

### 3.10 入站 webhook（外部系统 → 指定节点）

让 CI / 告警 / 机器人等外部系统凭端点令牌远程驱动节点发消息。端点 =
`webhook_endpoints` 文档：`pw_` 令牌哈希 + 目标配置（节点 / 专家 / 模型 /
权限模式 / 会话策略）一体，明文只在创建响应出现一次。

- **鉴权独立于账号令牌**：`pw_` 前缀不走 `verify_token` 的 access 分支，
  调不了 `/rpc`、`/commands` 等账号命令面——调用方唯一的自由度是
  `POST /webhooks/invoke {text}`，其余全部封在端点配置里（权限最小面）。
  每端点固定窗口限流 30/min（Redis 故障放行），每次调用实时校验账号状态
- **会话策略**：`fixed`（固定会话）/ `latest`（镜像 sessions 按 updated_at
  取最近活跃）/ `new`（服务端编排 `session.create`（绑专家）→
  `set_permission_mode` → `session.rename` → `agent.prompt`，与本地定时
  任务 execute_run 同一命令序列，节点无感知）；编排各步经 gateway
  `route_command` 等回执，节点离线 `refused_offline`
- **审计**：每次 invoke 落 `commands` 集合（`source: "webhook"` +
  `endpoint_id`），与 /commands、/rpc 同表异源
- **完成回调**：invoke 带 `callback_url`（调用级）或端点配置兜底时，登记
  `webhook_pending_callbacks` 挂起记录；agent 转空闲（`agent_state_changed`
  state=idle，上行白名单进 ingest）即认领并异步 POST 结果（最后一轮
  assistant 文本）到回调地址——ingest 单 worker 绝不内联等 HTTP。30 分钟
  未结束由后台清扫（30s 一轮）兜底投递 `status: "timeout"`；投递失败重试
  3 次（10s/60s）。请求签名 `X-PiDock-Signature: sha256=HMAC(body)`，密钥
  为端点 `callback_secret`（创建时生成、列表可见，仅用于调用方验签）。
  共享会话期间被人工插话可能携带后续轮次输出（policy=new 专属会话无此
  问题），已知取舍

### 3.11 IM 接入（平台适配器架构：飞书 / 钉钉）

IM 群聊作为触发源与回复通道：@机器人发消息 → 指定节点执行 → 结果回群。
复用 webhook 的中间层（`deliver` 编排、`on_turn_idle` 空闲触发、审计、
限流框架），替换两端（鉴权 = 平台侧校验；回复 = 平台消息 API）。

**工厂式架构（兼容性的真正保障）**：`im.rs` 定义 `ImPlatform` trait
（`kind` / `validate_channel` / `parse_inbound` / `reply_text` /
`uses_stream` + `stream_connection`），平台差异压缩到这四个方法；静态
注册表（`platforms()`）供路由、校验、回复分发、流监督统一查找。**共享
流水线（`process_message`）上收在本模块**：message_id 去重、审计、
deliver 编排、pending 登记、投递失败回群说明——平台适配器写不出不一致
的流水线。加平台 = 一个文件实现 trait + 注册表加一行。

- **渠道 = `im_channels` 文档**：平台凭证 + 与 webhook_endpoints 同构的
  目标配置（machine/expert/model/permission/session_policy/workspace），
  `webhook::deliver` 直接吃渠道文档。归属 = 配置它的账号；任何 @机器人
  的人都可能触发，权限默认 edit-auto（比 webhook 保守）
- **会话策略（IM 专属语义）**：`auto`（默认，群专属会话）——每个对话按
  `chat_key`（飞书 chat_id / 钉钉 conversationId，稳定标识；钉钉的
  sessionWebhook 是临时的不能做绑定键）绑定一个专属会话：首条消息走
  新建编排并把映射落 `im_chat_sessions`，后续消息直接复用，上下文连续；
  群里发 `/new` 重置映射（该条不投递，回执确认），下一条消息重新开会话。
  `new`（每条新建）/ `latest`（最近活跃，会漂移）/ `fixed`（指定会话，
  保存时强制非空；会话被删则报错回群，发 /new 不适用，需改配置）
- **飞书（双模式：push / stream）**：渠道 `mode` 字段选择。push 模式
  `POST /im/feishu/event/{channel_id}`：URL 内
  渠道 id = 弱凭证 + 确定性路由。url_verification 原样回 challenge（1s
  内）；Encrypt Key 验签（`X-Lark-Signature` = sha256(ts+nonce+key+body)）
  + 解密（AES-256-CBC，key = SHA256(Encrypt Key)，base64(iv+data)，PKCS7）；
  事件 token 必须等于 verification_token；只接 text（清洗 `@_user_N`）；
  回复走 `im/v1/messages`，tenant_access_token 换取后缓存 Redis。
  stream 模式（长连接，免公网地址，验证方式 = 连接握手鉴权，无需
  verification_token）：`POST /callback/ws/endpoint` 换 wss 地址（带
  device_id/service_id），帧为 pbbp2 protobuf（Frame{SeqID=1,LogID=2,
  service=3,method=4,headers=5,payload=8}），事件 payload 与推送同构；
  事件处理完回写 ACK 帧（payload `{"code":200}`），未 ACK 飞书重推；
  保活每 PingInterval（服务端可下发，默认 120s）发 CONTROL/ping 帧；
  sum>1 分包按 message_id 聚合。事件解析与推送共用 parse_event_payload
- **钉钉（Stream 流模式）**：服务端作 WS 客户端主动连钉钉，**无需公网
  回调地址**。`POST /v1.0/gateway/connections/open`（clientId/clientSecret
  + 订阅 SYSTEM/ping、SYSTEM/disconnect、CALLBACK 机器人 topic）换
  endpoint+ticket → WSS 接入 → 下行帧 `{type, headers{topic,messageId},
  data(JSON 串)}` 逐帧 ACK（`{code:200, headers{messageId}}`）；机器人
  载荷 `msgId/text.content/sessionWebhook`，回复直接 POST sessionWebhook
  （临时地址免 access_token）；保活 = 空闲 60s 发 WS Ping、5s 无入站判死；
  业务处理 spawn 出去不阻塞读循环，处理完才 ACK（未 ACK 钉钉重推 → 去重
  兜底）。`run_stream_supervisor`（30s 对账）为 enabled 渠道保证连接任务
  在跑，停用/删除即回收；连接内部 3s→60s 退避重连，重连前重读渠道配置
- **回复分发**：pending 记录 `kind` = 平台 kind（历史 `im_` 前缀兼容）+
  `reply_ctx`（平台自定义不透明 JSON：飞书 chat_id / 钉钉 conversationId
  + sessionWebhook + robotCode）；`post_callback` → `im::dispatch_reply`
  按注册表分发，共享「取最后一轮 assistant 文本 → 10s/60s 重试 → 落终态」，
  超时/无输出有兜底文案
- **双向同步**：auto 绑定的会话（im_chat_sessions）是双向通道。桌面/web
  在同会话的活动推到群——人工输入经 ingest 的 `message_complete(role=user)`
  挂钩实时镜像（前缀「👤 桌面端」；IM 自身 prompt 的会话回显由 Redis
  echo 标记按文本哈希抑制 30s）；agent 最终回复在空闲信号认领不到 pending
  时转发（前缀「🤖」= 桌面侧发起），IM 触发的轮次走原有回执通道不重复。
  钉钉主动推送优先群消息 API（`robot/groupMessages/send`，conversationId
  + robotCode(缺省 AppKey) + access_token 缓存 Redis），sessionWebhook
  仅作兜底——它是临时地址，撑不住主动推送
- **管理面**：web 控制台「IM 渠道」页，表单/详情按平台条件渲染（飞书展示
  事件订阅 URL，钉钉展示 Stream 接入说明）；平台清单由 server 注册表下发
- 管理面在 web 控制台「Webhook」页（`/webhooks` CRUD + 启停），专家下拉
  经 `/rpc` `experts.list` 实时拉取目标节点；自动化任务表单同款专家字段
  （`ScheduledJob.expert_id` 随 `session.create` 绑定）

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
access_tokens    {_id=jti, user_id, name, token_hash(sha256), days, expires_at, created_at}
webhook_endpoints {_id=端点id, user_id, name, token_hash, machine_id, session_policy
                 (new|latest|fixed), fixed_session_id, expert_id, model, thinking_level,
                 permission_mode, workspace, callback_url, callback_secret, enabled,
                 created_at, updated_at}
webhook_pending_callbacks {_id=投递id, kind(webhook|feishu|dingtalk…), endpoint_id, im_channel_id,
                 user_id, machine_id, session_id, callback_url, callback_secret, reply_ctx(JSON 串),
                 text, status(pending|delivering|delivered|failed), attempts, last_status_code,
                 last_error, created_at, expires_at, claimed_at, finished_at}
im_channels      {_id=渠道id, user_id, platform(feishu|dingtalk…), name, app_id, app_secret,
                 verification_token, encrypt_key, machine_id, session_policy(auto|new|latest|fixed),
                 fixed_session_id, expert_id, model, thinking_level, permission_mode,
                 workspace, enabled, created_at, updated_at}
im_chat_sessions {_id="{渠道id}:{chat_key}", channel_id, chat_key, session_id,
                 reply_ctx(JSON 串，最近一次 IM 消息的回复定位), created_at, updated_at}
                 auto 策略的群专属会话映射 + 双向同步的反查索引（session_id）
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
