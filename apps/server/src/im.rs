//! IM 接入：外部 IM 平台作为触发源与回复通道。平台差异被压缩到
//! [`ImPlatform`] 的四个方法（凭证校验 / 入站验证解析 / 回文本 / 是否流
//! 模式），其余全部是本模块统一执行的共享流水线——去重、审计、限流、
//! `webhook::deliver` 编排、pending 登记、回复重试。**平台适配器不得自管
//! 这些不变量**，兼容性由构造保证：新增平台 = 一个文件实现 trait + 注册表
//! 加一行，路由与回复分发零改动。
//!
//! 两类接入拓扑：
//! - HTTP 推送型（飞书）：平台回调 `POST /im/{kind}/event/{channel_id}`，
//!   URL 内渠道 id = 弱凭证 + 确定性路由，防伪主力在适配器（token/签名/解密）
//! - 流模式（钉钉 Stream）：无公网回调地址，适配器实现 `stream_connection`
//!   （一次连接的完整生命周期），`run_stream_supervisor` 按注册表对账启停
//!
//! pending 回调记录（webhook_pending_callbacks）`kind` = 平台 kind（历史
//! 记录兼容 `im_` 前缀），回复经 `dispatch_reply` 按注册表分发。

pub(crate) mod dingtalk;
pub(crate) mod feishu;

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Result},
    routing::{get, patch, post},
    Json, Router,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document as BsonDoc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{api_err, Bearer};
use crate::state::AppState;
use crate::webhook;

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 每渠道每分钟入站事件上限（正常流量远低于此，防滥用/打挂）
const RATE_LIMIT_PER_MIN: i64 = 120;
/// 每用户最多渠道数
const MAX_CHANNELS_PER_USER: usize = 10;
const SESSION_POLICIES: &[&str] = &["new", "auto", "latest", "fixed"];
/// 重置群专属会话的命令（仅 auto 策略响应）
const NEW_SESSION_COMMAND: &str = "/new";

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 群专属会话映射（im_chat_sessions）的记录 id
fn chat_session_doc_id(channel_id: &str, chat_key: &str) -> String {
    format!("{channel_id}:{chat_key}")
}

fn is_new_session_command(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case(NEW_SESSION_COMMAND)
}

// ------------------------------------------------------------ 平台适配器

/// 归一化入站消息：message_id 用于去重，chat_key 是会话内稳定的对话标识
/// （飞书 chat_id / 钉钉 conversationId，auto 策略按它绑定专属会话），
/// reply_ctx 是平台自定义的回复定位载荷（随 pending 记录存取）
pub(crate) struct InboundMessage {
    pub message_id: String,
    pub text: String,
    pub chat_key: String,
    pub reply_ctx: Value,
}

pub(crate) enum InboundDecision {
    /// 平台订阅验证请求：把这个 JSON 原样作为响应体返回（飞书 challenge）
    Verification(Value),
    /// 合法但无需处理（非目标事件类型等）
    Ignored(String),
    /// 业务消息 → 共享流水线
    Message(InboundMessage),
}

#[async_trait]
pub(crate) trait ImPlatform: Send + Sync {
    /// 平台标识；同时是路由段与 pending 记录的 kind
    fn kind(&self) -> &'static str;

    /// 平台特定凭证校验（建/改渠道时；通用校验在 validate 里）
    fn validate_channel(&self, body: &ChannelBody) -> Result<(), String>;

    /// 是否提供 HTTP 推送入站（false = 纯流模式，通用路由对其返回 404）
    fn http_inbound(&self) -> bool {
        true
    }

    /// 防伪 + 解密 + 解析。只做「读懂请求」，后续流水线统一执行
    async fn parse_inbound(
        &self,
        state: &AppState,
        channel: &BsonDoc,
        headers: &HeaderMap,
        body: &str,
    ) -> std::result::Result<InboundDecision, String>;

    /// 发送文本回复。失败可重试（共享流水线带 10s/60s 重试）
    async fn reply_text(
        &self,
        state: &AppState,
        channel: &BsonDoc,
        reply_ctx: &Value,
        text: &str,
    ) -> std::result::Result<(), String>;

    /// 是否需要后台流连接；true 时必须实现 stream_connection
    fn uses_stream(&self) -> bool {
        false
    }

    /// 流模式：一次连接的完整生命周期（含订阅与 ACK），断开即返回，
    /// 由 run_stream_supervisor 负责重连
    async fn stream_connection(&self, _state: &AppState, _channel: &BsonDoc) -> std::result::Result<(), String> {
        Err("platform does not support stream mode".into())
    }
}

static FEISHU: feishu::FeishuPlatform = feishu::FeishuPlatform;
static DINGTALK: dingtalk::DingTalkPlatform = dingtalk::DingTalkPlatform;

/// 平台注册表：新增平台在此加一行
fn platforms() -> &'static [&'static dyn ImPlatform] {
    static PLATFORMS: [&'static dyn ImPlatform; 2] = [&FEISHU, &DINGTALK];
    &PLATFORMS
}

fn platform_of(kind: &str) -> Option<&'static dyn ImPlatform> {
    platforms().iter().copied().find(|p| p.kind() == kind)
}

// ------------------------------------------------------------ 渠道 CRUD

#[derive(Deserialize)]
pub(crate) struct ChannelBody {
    platform: String,
    name: String,
    app_id: String,
    app_secret: String,
    /// 接入方式：push（HTTP 推送，默认）| stream（长连接，免公网地址）。
    /// 空 = 平台默认（钉钉 stream，飞书 push）
    #[serde(default)]
    mode: String,
    /// 可选；飞书 push 模式配置后事件加密推送
    #[serde(default)]
    encrypt_key: String,
    /// 飞书 push 模式必填（防伪）；长连接模式无需
    #[serde(default)]
    verification_token: String,
    machine_id: String,
    #[serde(default)]
    session_policy: String,
    #[serde(default)]
    fixed_session_id: String,
    #[serde(default)]
    expert_id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    thinking_level: String,
    /// 空 = edit-auto（IM 触发方不可控，比 webhook 的 full 更保守）
    #[serde(default)]
    permission_mode: String,
    #[serde(default)]
    workspace: String,
}

/// 通用校验 + 平台特定校验（经注册表分发）
async fn validate(state: &AppState, user_id: &str, body: &ChannelBody) -> Result<(), String> {
    let Some(platform) = platform_of(&body.platform) else {
        return Err(format!(
            "暂不支持的平台，可选：{}",
            platforms().iter().map(|p| p.kind()).collect::<Vec<_>>().join(" / ")
        ));
    };
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err("渠道名称不能为空且不超过 64 字符".into());
    }
    if body.app_id.trim().is_empty() || body.app_secret.trim().is_empty() {
        return Err("App ID 与 App Secret 不能为空".into());
    }
    if !["", "push", "stream"].contains(&body.mode.as_str()) {
        return Err("无效的接入方式，可选：push / stream".into());
    }
    platform.validate_channel(body)?;
    let owned = state
        .mongo
        .collection::<BsonDoc>("machines")
        .count_documents(doc! { "_id": &body.machine_id, "user_id": user_id })
        .await
        .map_err(|e| e.to_string())?;
    if owned == 0 {
        return Err("目标节点不存在或不属于当前账号".into());
    }
    if !SESSION_POLICIES.contains(&body.session_policy.as_str()) {
        return Err("无效的会话策略，可选：new / latest / fixed".into());
    }
    if body.session_policy == "fixed" && body.fixed_session_id.trim().is_empty() {
        return Err("策略 fixed 需要提供 fixed_session_id".into());
    }
    Ok(())
}

/// 渠道接入方式缺省值：钉钉类流平台 = stream，其余 = push
fn default_mode(platform: &dyn ImPlatform) -> &'static str {
    if platform.uses_stream() { "stream" } else { "push" }
}

fn default_perm(p: &str) -> &str {
    if p.trim().is_empty() {
        "edit-auto"
    } else {
        p.trim()
    }
}

fn channel_json(d: &BsonDoc, hostname: &str) -> Value {
    let mode = {
        let m = d.get_str("mode").unwrap_or_default();
        if m.is_empty() {
            match platform_of(d.get_str("platform").unwrap_or_default()) {
                Some(p) => default_mode(p),
                None => "push",
            }
        } else {
            m
        }
    };
    json!({
        "id": d.get_str("_id").unwrap_or_default(),
        "platform": d.get_str("platform").unwrap_or_default(),
        "mode": mode,
        "name": d.get_str("name").unwrap_or_default(),
        "app_id": d.get_str("app_id").unwrap_or_default(),
        "app_secret": d.get_str("app_secret").unwrap_or_default(),
        "encrypt_key": d.get_str("encrypt_key").unwrap_or_default(),
        "verification_token": d.get_str("verification_token").unwrap_or_default(),
        "machine_id": d.get_str("machine_id").unwrap_or_default(),
        "machine_hostname": hostname,
        "session_policy": d.get_str("session_policy").unwrap_or("latest"),
        "fixed_session_id": d.get_str("fixed_session_id").unwrap_or_default(),
        "expert_id": d.get_str("expert_id").unwrap_or_default(),
        "model": d.get_str("model").unwrap_or_default(),
        "thinking_level": d.get_str("thinking_level").unwrap_or_default(),
        "permission_mode": d.get_str("permission_mode").unwrap_or("edit-auto"),
        "workspace": d.get_str("workspace").unwrap_or_default(),
        "enabled": d.get_bool("enabled").unwrap_or(true),
        "created_at": d.get_str("created_at").unwrap_or_default(),
    })
}

async fn channels_list(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let docs = state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .find(doc! { "user_id": &user_id })
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! { "created_at": -1 })
                .build(),
        )
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<BsonDoc>>()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let machines: std::collections::HashMap<String, String> = state
        .mongo
        .collection::<BsonDoc>("machines")
        .find(doc! { "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<BsonDoc>>()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .into_iter()
        .filter_map(|m| {
            let id = m.get_str("_id").unwrap_or_default().to_string();
            let host = m.get_str("hostname").unwrap_or_default().to_string();
            (!id.is_empty()).then(|| (id, host))
        })
        .collect();
    let mut out = Vec::with_capacity(docs.len());
    for d in &docs {
        let mid = d.get_str("machine_id").unwrap_or_default();
        out.push(channel_json(d, machines.get(mid).map(String::as_str).unwrap_or("")));
    }
    Ok(Json(json!({ "channels": out, "platforms": platforms().iter().map(|p| p.kind()).collect::<Vec<_>>() })))
}

async fn channels_create(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<ChannelBody>,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    validate(&state, &user_id, &body)
        .await
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    let count = state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .count_documents(doc! { "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if count >= MAX_CHANNELS_PER_USER as u64 {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            format!("最多创建 {MAX_CHANNELS_PER_USER} 个 IM 渠道，请先删除不用的"),
        ));
    }
    let id = uuid::Uuid::now_v7().to_string();
    let created_at = now_rfc3339();
    let d = doc! {
        "_id": &id,
        "user_id": &user_id,
        "platform": &body.platform,
        "mode": if body.mode.trim().is_empty() {
            default_mode(platform_of(&body.platform).expect("validated"))
        } else {
            body.mode.trim()
        },
        "name": body.name.trim(),
        "app_id": body.app_id.trim(),
        "app_secret": body.app_secret.trim(),
        "encrypt_key": body.encrypt_key.trim(),
        "verification_token": body.verification_token.trim(),
        "machine_id": &body.machine_id,
        "session_policy": &body.session_policy,
        "fixed_session_id": body.fixed_session_id.trim(),
        "expert_id": body.expert_id.trim(),
        "model": body.model.trim(),
        "thinking_level": body.thinking_level.trim(),
        "permission_mode": default_perm(&body.permission_mode),
        "workspace": body.workspace.trim(),
        "enabled": true,
        "created_at": &created_at,
        "updated_at": &created_at,
    };
    state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .insert_one(d.clone())
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(user = %user_id, channel = %id, platform = %body.platform, "im channel created");
    Ok(Json(channel_json(&d, "")))
}

#[derive(Deserialize)]
struct ChannelPatch {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    app_secret: Option<String>,
    #[serde(default)]
    encrypt_key: Option<String>,
    #[serde(default)]
    verification_token: Option<String>,
    #[serde(default)]
    machine_id: Option<String>,
    #[serde(default)]
    session_policy: Option<String>,
    #[serde(default)]
    fixed_session_id: Option<String>,
    #[serde(default)]
    expert_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    thinking_level: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn channels_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
    Json(body): Json<ChannelPatch>,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let channels = state.mongo.collection::<BsonDoc>("im_channels");
    let existing = channels
        .find_one(doc! { "_id": &id, "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "IM 渠道不存在"))?;
    // 策略组合用合并后的值校验（凭证字段不影响组合校验，仅占位）
    let merged = ChannelBody {
        platform: existing.get_str("platform").unwrap_or("feishu").to_string(),
        name: body.name.clone().unwrap_or_else(|| existing.get_str("name").unwrap_or_default().to_string()),
        app_id: body.app_id.clone().unwrap_or_else(|| existing.get_str("app_id").unwrap_or_default().to_string()),
        app_secret: body
            .app_secret
            .clone()
            .unwrap_or_else(|| existing.get_str("app_secret").unwrap_or_default().to_string()),
        mode: body
            .mode
            .clone()
            .unwrap_or_else(|| existing.get_str("mode").unwrap_or_default().to_string()),
        encrypt_key: String::new(),
        verification_token: body
            .verification_token
            .clone()
            .unwrap_or_else(|| existing.get_str("verification_token").unwrap_or_default().to_string()),
        machine_id: body
            .machine_id
            .clone()
            .unwrap_or_else(|| existing.get_str("machine_id").unwrap_or_default().to_string()),
        session_policy: body
            .session_policy
            .clone()
            .unwrap_or_else(|| existing.get_str("session_policy").unwrap_or("latest").to_string()),
        fixed_session_id: body
            .fixed_session_id
            .clone()
            .unwrap_or_else(|| existing.get_str("fixed_session_id").unwrap_or_default().to_string()),
        expert_id: String::new(),
        model: String::new(),
        thinking_level: String::new(),
        permission_mode: String::new(),
        workspace: String::new(),
    };
    validate(&state, &user_id, &merged)
        .await
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;

    let mut set = doc! { "updated_at": now_rfc3339() };
    macro_rules! put {
        ($field:ident) => {
            if let Some(v) = &body.$field {
                set.insert(stringify!($field), v.trim());
            }
        };
    }
    put!(name);
    if let Some(v) = &body.mode {
        if !["", "push", "stream"].contains(&v.trim()) {
            return Err(api_err(StatusCode::BAD_REQUEST, "无效的接入方式，可选：push / stream"));
        }
        set.insert(
            "mode",
            if v.trim().is_empty() {
                default_mode(platform_of(existing.get_str("platform").unwrap_or("feishu")).expect("existing"))
            } else {
                v.trim()
            },
        );
    }
    put!(app_id);
    put!(app_secret);
    put!(encrypt_key);
    put!(verification_token);
    put!(machine_id);
    put!(session_policy);
    put!(fixed_session_id);
    put!(expert_id);
    put!(model);
    put!(thinking_level);
    put!(workspace);
    if let Some(v) = &body.permission_mode {
        set.insert("permission_mode", default_perm(v));
    }
    if let Some(v) = body.enabled {
        set.insert("enabled", v);
    }
    channels
        .update_one(doc! { "_id": &id, "user_id": &user_id }, doc! { "$set": set })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

async fn channels_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let r = state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .delete_one(doc! { "_id": &id, "user_id": user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if r.deleted_count == 0 {
        return Err(api_err(StatusCode::NOT_FOUND, "IM 渠道不存在"));
    }
    let mut conn = state.redis.clone();
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(format!("webhook:rl:im:{id}"))
        .query_async(&mut conn)
        .await;
    tracing::info!(channel = %id, "im channel deleted");
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------ 入站流水线

/// 共享入站流水线（HTTP 推送与流模式共用）：去重 → 审计 → deliver 编排 →
/// pending 登记 / 失败回群说明。平台适配器只负责把请求读成 InboundMessage
pub(crate) enum PipelineOutcome {
    Accepted { session_id: String },
    Duplicate,
    /// 已按控制命令处理（如 /new），不再投递
    Handled { note: String },
    Failed { error: String },
}

pub(crate) async fn process_message(
    state: &AppState,
    platform: &dyn ImPlatform,
    channel: &BsonDoc,
    msg: InboundMessage,
) -> PipelineOutcome {
    let channel_id = channel.get_str("_id").unwrap_or_default().to_string();
    let machine_id = channel.get_str("machine_id").unwrap_or_default().to_string();
    let user_id = channel.get_str("user_id").unwrap_or_default().to_string();

    // 去重：平台失败重推（飞书 HTTP 重试 / 钉钉未 ACK 重发）同一事件只处理一次。
    // SET NX 成功回 "OK"（字符串）、重复回 Nil——不能用 i64 接
    let dedup_key = format!("im:dedup:{channel_id}:{}", msg.message_id);
    let mut conn = state.redis.clone();
    let acquired: bool = match redis::cmd("SET")
        .arg(&dedup_key)
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(3600)
        .query_async::<redis::Value>(&mut conn)
        .await
    {
        Ok(redis::Value::Nil) => false,
        Ok(_) => true,
        // Redis 故障放行：宁可重复处理也不丢消息
        Err(e) => {
            tracing::warn!(channel = %channel_id, "im 去重 Redis 故障放行: {e}");
            true
        }
    };
    if !acquired {
        return PipelineOutcome::Duplicate;
    }

    // IM 来源标记：桌面输入镜像据此抑制本条 prompt 的会话回显（echo 事件
    // 与 prompt 文本一致，按文本哈希标记——新会话场景 deliver 前还拿不到
    // session_id，只能按文本锚定）
    let _: Result<i64, _> = redis::cmd("SET")
        .arg(format!("im:echo:{}", sha256_hex(&msg.text)))
        .arg(1)
        .arg("EX")
        .arg(30)
        .query_async(&mut conn)
        .await;

    // auto 策略：每个对话一个专属会话。首条消息走新建并把 chat_key →
    // session_id 映射落库（im_chat_sessions），后续消息直接复用；
    // /new 重置映射（该条不投递），下一条消息重新开会话
    let policy = channel.get_str("session_policy").unwrap_or("latest").to_string();
    let mut session_override: Option<String> = None;
    if policy == "auto" && !msg.chat_key.is_empty() {
        let map_key = chat_session_doc_id(&channel_id, &msg.chat_key);
        let chats = state.mongo.collection::<BsonDoc>("im_chat_sessions");
        if is_new_session_command(&msg.text) {
            let r = chats.delete_one(doc! { "_id": &map_key }).await;
            let cleared = r.as_ref().map(|d| d.deleted_count > 0).unwrap_or(false);
            tracing::info!(channel = %channel_id, chat = %msg.chat_key, "im: /new 重置群专属会话（原有映射存在: {cleared}）");
            let _ = platform
                .reply_text(state, channel, &msg.reply_ctx, "✅ 新会话已就绪，下一条消息将开始全新的上下文。")
                .await;
            return PipelineOutcome::Handled { note: NEW_SESSION_COMMAND.to_string() };
        }
        if let Ok(Some(d)) = chats.find_one(doc! { "_id": &map_key }).await {
            let sid = d.get_str("session_id").unwrap_or_default().to_string();
            if !sid.is_empty() {
                session_override = Some(sid);
            }
        }
    }

    let started = std::time::Instant::now();
    match webhook::deliver(state, &user_id, channel, &msg.text, session_override.as_deref()).await {
        Ok((session_id, _command_id)) => {
            audit(state, channel, &machine_id, &session_id, "ok", started).await;
            // auto 策略：绑定落库/刷新（reply_ctx 随每条消息更新——钉钉的
            // robotCode 等定位信息以最新载荷为准，旧映射自愈）
            if policy == "auto" && !msg.chat_key.is_empty() {
                let map_key = chat_session_doc_id(&channel_id, &msg.chat_key);
                let _ = state
                    .mongo
                    .collection::<BsonDoc>("im_chat_sessions")
                    .update_one(
                        doc! { "_id": &map_key },
                        doc! {
                            "$set": {
                                "channel_id": &channel_id,
                                "chat_key": &msg.chat_key,
                                "session_id": &session_id,
                                "reply_ctx": serde_json::to_string(&msg.reply_ctx).unwrap_or_default(),
                                "updated_at": now_rfc3339(),
                            },
                            "$setOnInsert": { "created_at": now_rfc3339() },
                        },
                    )
                    .with_options(mongodb::options::UpdateOptions::builder().upsert(true).build())
                    .await;
                tracing::info!(channel = %channel_id, chat = %msg.chat_key, session = %session_id, "im: 绑定群专属会话");
            }
            match register_pending(state, platform, channel, &session_id, &msg).await {
                Ok(_) => PipelineOutcome::Accepted { session_id },
                Err(e) => {
                    tracing::warn!(channel = %channel_id, "im 回调登记失败: {e}");
                    PipelineOutcome::Accepted { session_id }
                }
            }
        }
        Err(e) => {
            let status = if e == "refused_offline" { "refused_offline" } else { "error" };
            audit(state, channel, &machine_id, "", status, started).await;
            // 投递失败直接回群里说明（best effort），避免用户干等
            let hint = if e == "refused_offline" { "节点离线，消息未送达" } else { &e };
            let _ = platform
                .reply_text(state, channel, &msg.reply_ctx, &format!("⚠️ PiDock 投递失败：{hint}"))
                .await;
            PipelineOutcome::Failed { error: e }
        }
    }
}

async fn audit(
    state: &AppState,
    channel: &BsonDoc,
    machine_id: &str,
    session_id: &str,
    status: &str,
    started: std::time::Instant,
) {
    let _ = state
        .mongo
        .collection::<BsonDoc>("commands")
        .insert_one(doc! {
            "command_id": uuid::Uuid::now_v7().to_string(),
            "user_id": channel.get_str("user_id").unwrap_or_default(),
            "machine_id": machine_id,
            "session_id": session_id,
            "type": "im.invoke",
            "payload": channel.get_str("name").unwrap_or_default(),
            "source": "im",
            "endpoint_id": channel.get_str("_id").unwrap_or_default(),
            "status": status,
            "duration_ms": started.elapsed().as_millis() as i64,
            "ts": now_rfc3339(),
        })
        .await;
}

/// 登记 IM 挂起回调：agent 空闲时 post_callback 按 kind 分发回 dispatch_reply
async fn register_pending(
    state: &AppState,
    platform: &dyn ImPlatform,
    channel: &BsonDoc,
    session_id: &str,
    msg: &InboundMessage,
) -> Result<String, String> {
    let cb_id = uuid::Uuid::now_v7().to_string();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(webhook::CALLBACK_TTL_SECS)).to_rfc3339();
    state
        .mongo
        .collection::<BsonDoc>("webhook_pending_callbacks")
        .insert_one(doc! {
            "_id": &cb_id,
            "kind": platform.kind(),
            "im_channel_id": channel.get_str("_id").unwrap_or_default(),
            "user_id": channel.get_str("user_id").unwrap_or_default(),
            "machine_id": channel.get_str("machine_id").unwrap_or_default(),
            "session_id": session_id,
            "callback_url": "",
            "callback_secret": "",
            "reply_ctx": serde_json::to_string(&msg.reply_ctx).unwrap_or_default(),
            "text": &msg.text,
            "status": "pending",
            "created_at": now_rfc3339(),
            "expires_at": &expires_at,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(cb_id)
}

/// 通用 HTTP 入站（推送型平台）：`/im/{kind}/event/{channel_id}`。
/// URL 内渠道 id = 弱凭证 + 确定性路由；防伪主力在适配器
async fn inbound_event(
    State(state): State<AppState>,
    Path((kind, channel_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let Some(platform) = platform_of(&kind) else {
        return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "unknown platform"}))).into_response();
    };
    if !platform.http_inbound() {
        return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "platform has no http inbound"})))
            .into_response();
    }
    let channel = match state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .find_one(doc! { "_id": &channel_id, "platform": &kind })
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "channel not found"}))).into_response();
        }
        Err(e) => {
            tracing::error!(channel = %channel_id, "im 渠道读取失败: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false}))).into_response();
        }
    };
    if !channel.get_bool("enabled").unwrap_or(true) {
        return (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "channel disabled"}))).into_response();
    }
    if !webhook::rate_limit(&state, "im", &channel_id, RATE_LIMIT_PER_MIN).await {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"ok": false}))).into_response();
    }

    match platform.parse_inbound(&state, &channel, &headers, &body).await {
        Ok(InboundDecision::Verification(resp)) => Json(resp).into_response(),
        Ok(InboundDecision::Ignored(reason)) => Json(json!({"ok": true, "ignored": reason})).into_response(),
        Ok(InboundDecision::Message(msg)) => match process_message(&state, platform, &channel, msg).await {
            PipelineOutcome::Accepted { session_id } => Json(json!({"ok": true, "session_id": session_id})).into_response(),
            PipelineOutcome::Duplicate => Json(json!({"ok": true, "dedup": true})).into_response(),
            PipelineOutcome::Handled { note } => Json(json!({"ok": true, "handled": note})).into_response(),
            PipelineOutcome::Failed { error } => Json(json!({"ok": false, "error": error})).into_response(),
        },
        Err(e) => {
            tracing::warn!(channel = %channel_id, platform = %kind, "im 入站校验失败: {e}");
            (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": e}))).into_response()
        }
    }
}

// ------------------------------------------------------------ 回复分发

/// post_callback 的 IM 分支：按 pending 记录的 kind 从注册表取适配器，
/// 共享「取最后一轮 assistant 文本 → 重试 → 落终态」；平台只管发文本
pub(crate) async fn dispatch_reply(state: &AppState, kind_raw: &str, cb: &BsonDoc, timed_out: bool) {
    let kind = kind_raw.strip_prefix("im_").unwrap_or(kind_raw); // 兼容历史 im_feishu 记录
    let Some(platform) = platform_of(kind) else {
        webhook::finish_pending(state, cb.get_str("_id").unwrap_or_default(), "failed", 0, None, "unknown im platform")
            .await;
        return;
    };
    let cb_id = cb.get_str("_id").unwrap_or_default().to_string();
    let channel_id = cb.get_str("im_channel_id").unwrap_or_default().to_string();
    let session_id = cb.get_str("session_id").unwrap_or_default().to_string();

    let channel = match state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .find_one(doc! { "_id": &channel_id })
        .await
    {
        Ok(Some(c)) => c,
        _ => {
            webhook::finish_pending(state, &cb_id, "failed", 0, None, "im channel missing").await;
            return;
        }
    };
    // reply_ctx：新记录是 JSON 串；历史飞书记录是扁平 reply_chat_id
    let reply_ctx = match serde_json::from_str::<Value>(cb.get_str("reply_ctx").unwrap_or_default()) {
        Ok(v) => v,
        Err(_) => json!({ "chat_id": cb.get_str("reply_chat_id").unwrap_or_default() }),
    };

    let (status, text) = if timed_out {
        ("timeout", "⏱️ PiDock 任务超过 30 分钟未结束，已放弃等待。".to_string())
    } else {
        match webhook::fetch_last_assistant(state, &session_id).await {
            Some(m) => {
                let t = m.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if t.is_empty() {
                    ("no_output", "PiDock 任务已结束，本轮没有文本输出。".to_string())
                } else {
                    ("ok", t)
                }
            }
            None => ("no_output", "PiDock 任务已结束，本轮没有文本输出。".to_string()),
        }
    };

    const RETRY_BACKOFF_SECS: [u64; 2] = [10, 60];
    let mut attempts = 0u32;
    for i in 0..=RETRY_BACKOFF_SECS.len() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(RETRY_BACKOFF_SECS[i - 1])).await;
        }
        attempts += 1;
        match platform.reply_text(state, &channel, &reply_ctx, &text).await {
            Ok(()) => {
                webhook::finish_pending(state, &cb_id, "delivered", attempts, None, "").await;
                tracing::info!(delivery = %cb_id, platform = %kind, "im 回复投递成功");
                return;
            }
            Err(e) => {
                if i == RETRY_BACKOFF_SECS.len() {
                    webhook::finish_pending(state, &cb_id, "failed", attempts, None, &e).await;
                    tracing::warn!(delivery = %cb_id, platform = %kind, status = %status, "im 回复投递失败: {e}");
                }
            }
        }
    }
}

// ------------------------------------------------------------ 流模式监督

/// 流模式平台对账循环：enabled 且 uses_stream 的渠道保证有连接任务在跑，
/// 删除/停用即回收。连接内部自带重连退避，这里只管生命周期对账（30s 一轮）
pub(crate) async fn run_stream_supervisor(state: AppState) {
    let mut live: std::collections::HashMap<String, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        // 先收割已结束的任务：JoinHandle 留在表里会让监督任务以为连接还活着，
        // 死掉的连接永远等不到重启
        live.retain(|id, handle| {
            if handle.is_finished() {
                tracing::warn!(channel = %id, "im 流连接任务已退出，本轮重启");
                false
            } else {
                true
            }
        });
        let channels: Vec<BsonDoc> = match state
            .mongo
            .collection::<BsonDoc>("im_channels")
            .find(doc! { "enabled": true })
            .await
        {
            Ok(cursor) => match cursor.try_collect::<Vec<BsonDoc>>().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("im 流监督查询失败: {e}");
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("im 流监督查询失败: {e}");
                continue;
            }
        };
        let mut wanted: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ch in channels {
            let Some(platform) = platform_of(ch.get_str("platform").unwrap_or_default()) else {
                continue;
            };
            // 接入方式：渠道显式 mode 优先，缺省按平台（钉钉 stream / 飞书 push）
            let mode = ch
                .get_str("mode")
                .ok()
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| default_mode(platform).to_string());
            if mode != "stream" {
                continue;
            }
            let id = ch.get_str("_id").unwrap_or_default().to_string();
            if id.is_empty() || !wanted.insert(id.clone()) {
                continue;
            }
            if !live.contains_key(&id) {
                tracing::info!(channel = %id, platform = platform.kind(), "im 流连接启动");
                let st = state.clone();
                let handle = tokio::spawn(async move {
                    stream_conn_loop(st, platform, ch).await;
                });
                live.insert(id, handle);
            }
        }
        live.retain(|id, handle| {
            if wanted.contains(id) {
                true
            } else {
                handle.abort();
                tracing::info!(channel = %id, "im 流连接停止");
                false
            }
        });
    }
}

async fn stream_conn_loop(state: AppState, platform: &'static dyn ImPlatform, mut channel: BsonDoc) {
    let channel_id = channel.get_str("_id").unwrap_or_default().to_string();
    let mut backoff: u64 = 3;
    loop {
        // 每次连接前重读渠道：配置/启停即时生效（停用由监督任务 abort）
        match state
            .mongo
            .collection::<BsonDoc>("im_channels")
            .find_one(doc! { "_id": &channel_id })
            .await
        {
            Ok(Some(c)) => channel = c,
            Ok(None) => {
                tracing::info!(channel = %channel_id, "im 渠道已删除，流连接退出");
                break;
            }
            // Mongo 瞬时故障绝不能让连接任务静默退出（监督任务认死了
            // JoinHandle 就不会再拉起）——重试而不是退出
            Err(e) => {
                tracing::warn!(channel = %channel_id, "im 渠道读取失败，{backoff}s 后重试: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
        }
        if !channel.get_bool("enabled").unwrap_or(true) {
            tracing::info!(channel = %channel_id, "im 渠道已停用，流连接退出");
            break;
        }
        let started = std::time::Instant::now();
        match platform.stream_connection(&state, &channel).await {
            Ok(()) => break,
            Err(e) => {
                // 存活超过一分钟的连接视为曾经健康，重连退避归零
                if started.elapsed() >= std::time::Duration::from_secs(60) {
                    backoff = 3;
                }
                tracing::warn!(channel = %channel_id, "im 流连接断开，{backoff}s 后重连: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

// ------------------------------------------------------------ 双向同步

/// 从 message_complete 载荷提取文本块
fn blocks_text(payload: &Value) -> String {
    payload
        .get("blocks")
        .and_then(|b| b.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 反查会话绑定的群：auto 策略的 im_chat_sessions（reply_ctx 缺失时带
/// chat_key 兜底键，由各适配器自行解释）
async fn bound_chats(state: &AppState, session_id: &str) -> Vec<(&'static dyn ImPlatform, BsonDoc, Value)> {
    let bindings = match state
        .mongo
        .collection::<BsonDoc>("im_chat_sessions")
        .find(doc! { "session_id": session_id })
        .await
    {
        Ok(c) => c.try_collect::<Vec<BsonDoc>>().await.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(session = %session_id, "im 绑定反查失败: {e}");
            return vec![];
        }
    };
    let mut out = Vec::new();
    for b in bindings {
        let channel_id = b.get_str("channel_id").unwrap_or_default();
        let Ok(Some(channel)) = state
            .mongo
            .collection::<BsonDoc>("im_channels")
            .find_one(doc! { "_id": channel_id, "enabled": true })
            .await
        else {
            continue;
        };
        let Some(platform) = platform_of(channel.get_str("platform").unwrap_or_default()) else {
            continue;
        };
        let mut reply_ctx: Value =
            serde_json::from_str(b.get_str("reply_ctx").unwrap_or_default()).unwrap_or(json!({}));
        if reply_ctx.get("chat_key").is_none() {
            if let Some(chat_key) = b.get_str("chat_key").ok().filter(|s| !s.is_empty()) {
                reply_ctx["chat_key"] = json!(chat_key);
            }
        }
        out.push((platform, channel, reply_ctx));
    }
    out
}

/// 桌面端人工输入镜像：会话被群绑定时，把桌面/web 里的人输消息转述到群。
/// IM 自身来源的 prompt 会话回显由 echo 标记抑制（process_message 设置）
pub(crate) async fn mirror_user_message(state: AppState, session_id: String, payload: Value) {
    let text = blocks_text(&payload);
    if text.trim().is_empty() {
        return;
    }
    let mut conn = state.redis.clone();
    let echo: bool = redis::cmd("EXISTS")
        .arg(format!("im:echo:{}", sha256_hex(text.trim())))
        .query_async(&mut conn)
        .await
        .unwrap_or(0)
        == 1;
    if echo {
        return;
    }
    for (platform, channel, reply_ctx) in bound_chats(&state, &session_id).await {
        let body = format!("👤 桌面端\n{text}");
        if let Err(e) = platform.reply_text(&state, &channel, &reply_ctx, &body).await {
            tracing::warn!(
                channel = %channel.get_str("_id").unwrap_or_default(),
                "im 镜像桌面输入失败: {e}"
            );
        }
    }
}

/// 桌面端 agent 回复转发：非 IM 触发的轮次结束时（on_turn_idle 认领不到
/// pending），把最后一轮 assistant 回复发到绑定的群
pub(crate) async fn forward_session_activity(state: AppState, session_id: String) {
    let Some(m) = webhook::fetch_last_assistant(&state, &session_id).await else {
        return;
    };
    let text = m.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if text.is_empty() {
        return;
    }
    for (platform, channel, reply_ctx) in bound_chats(&state, &session_id).await {
        let body = format!("🤖 {text}");
        if let Err(e) = platform.reply_text(&state, &channel, &reply_ctx, &body).await {
            tracing::warn!(
                channel = %channel.get_str("_id").unwrap_or_default(),
                "im 转发桌面回复失败: {e}"
            );
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/channels", get(channels_list).post(channels_create))
        .route("/im/channels/{id}", patch(channels_update).delete(channels_delete))
        .route("/im/{kind}/event/{channel_id}", post(inbound_event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_command_detection() {
        assert!(is_new_session_command("/new"));
        assert!(is_new_session_command("  /new  "));
        assert!(is_new_session_command("/NEW"));
        assert!(!is_new_session_command("/new 帮我看看日志"));
        assert!(!is_new_session_command("你好 /new"));
        assert!(!is_new_session_command(""));
    }

    #[test]
    fn chat_session_key_scopes_by_channel() {
        // 映射键必须含渠道维度：同群接两个渠道互不串
        assert_eq!(
            chat_session_doc_id("ch1", "oc_abc"),
            "ch1:oc_abc"
        );
        assert_ne!(
            chat_session_doc_id("ch1", "oc_abc"),
            chat_session_doc_id("ch2", "oc_abc")
        );
    }
}
