//! 入站 webhook：外部系统（CI / 告警 / 机器人）凭端点令牌远程驱动节点发消息。
//!
//! 端点 = Mongo `webhook_endpoints` 文档：调用方凭证（`pw_` 令牌的哈希）+
//! 目标配置（节点 / 专家 / 模型 / 权限模式 / 会话策略）一体。令牌格式
//! `pw_<端点id>.<secret>`（端点 id 为 32 位 hex），sha256 落库，明文只在
//! 创建响应里出现一次；每次调用按 _id 直查 Mongo 并实时校验账号状态
//! （调用量低频，不值得上 Redis 读缓存，换来吊销/停用/改配置即时生效）。
//!
//! 调用面只有一个动作：`POST /webhooks/invoke {text}`。会话定位由端点
//! 配置决定——fixed（固定会话）/ latest（节点最近活跃会话）/ new（编排
//! `session.create`（绑专家）→ `set_permission_mode` → `session.rename` →
//! `agent.prompt`，与本地定时任务 execute_run 同一命令序列，节点无感知）。
//! 命令白名单不需要显式表：调用方唯一的自由度就是 text，其余全部封在配置里。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{get, patch, post},
    Json, Router,
};
use futures_util::TryStreamExt;
use hmac::{Hmac, Mac};
use mongodb::bson::{doc, Document as BsonDoc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{api_err, user_active, Bearer};
use crate::gateway;
use crate::state::AppState;

/// 令牌前缀：与 PAT（pd_）区分，校验路径独立——pw_ 不能通过 verify_token，
/// 即泄露 webhook 令牌也调不了 /rpc、/commands 等任意账号命令面
pub const PREFIX: &str = "pw_";
/// 每端点每分钟最多调用次数（固定窗口，Redis 故障时放行，与登录限流同哲学）
const RATE_LIMIT_PER_MIN: i64 = 30;
/// 单步命令等回执上限：session.create 等是快命令，agent.prompt 本就立即回「已受理」
const STEP_TIMEOUT_SECS: u64 = 20;
/// 消息文本长度上限（字符）：给足代码块空间，挡滥用
const MAX_TEXT_CHARS: usize = 100_000;
/// 每用户最多端点数
const MAX_ENDPOINTS_PER_USER: usize = 20;
/// 完成回调等待上限：超过即投递 timeout 兜底通知
pub(crate) const CALLBACK_TTL_SECS: i64 = 30 * 60;
/// 回调 HTTP 客户端单次超时
const CALLBACK_HTTP_TIMEOUT_SECS: u64 = 10;
/// 回调重试节奏：立即 → 10s → 60s，全失败记 failed 留查
const CALLBACK_RETRY_BACKOFF_SECS: [u64; 2] = [10, 60];

type HmacSha256 = Hmac<Sha256>;

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 会话策略合法值
const SESSION_POLICIES: &[&str] = &["new", "latest", "fixed"];

// ------------------------------------------------------------ 校验

/// 校验 pw_ 令牌并取回端点文档：格式 → sha256 对比 Mongo → 停用/账号状态。
/// 返回（归属用户，端点文档）。不做 Redis 缓存：配置本身也要回查，缓存省不掉这次读
async fn verify(state: &AppState, token: &str) -> Result<(String, BsonDoc), String> {
    let rest = token
        .strip_prefix(PREFIX)
        .and_then(|r| r.split_once('.'))
        .ok_or("webhook 令牌格式不正确")?;
    let (ep_id, secret) = rest;
    if ep_id.len() != 32 || !ep_id.bytes().all(|b| b.is_ascii_hexdigit()) || secret.is_empty() {
        return Err("webhook 令牌格式不正确".into());
    }
    let d = state
        .mongo
        .collection::<BsonDoc>("webhook_endpoints")
        .find_one(doc! { "_id": ep_id })
        .await
        .map_err(|e| e.to_string())?
        .ok_or("webhook 令牌已撤销或不存在")?;
    if d.get_str("token_hash").unwrap_or_default() != sha256_hex(token) {
        return Err("webhook 令牌已撤销或不存在".into());
    }
    if !d.get_bool("enabled").unwrap_or(true) {
        return Err("webhook 端点已停用".into());
    }
    let user_id = d.get_str("user_id").unwrap_or_default().to_string();
    if user_id.is_empty() {
        return Err("webhook 令牌已撤销或不存在".into());
    }
    // 与 PAT 同策略：长期令牌每次使用实时校验账号状态
    if !user_active(state, &user_id).await {
        return Err("账号已被禁用".into());
    }
    Ok((user_id, d))
}

/// 端点固定窗口限流；Redis 故障放行并记日志（可用性优先）。IM 入站同用
pub(crate) async fn rate_limit(state: &AppState, key_scope: &str, id: &str, max: i64) -> bool {
    let mut conn = state.redis.clone();
    let key = format!("webhook:rl:{key_scope}:{id}");
    let n: i64 = match redis::cmd("INCR").arg(&key).query_async(&mut conn).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(scope = %key_scope, id = %id, "限流 Redis 故障放行: {e}");
            return true;
        }
    };
    if n == 1 {
        let _: Result<(), _> = conn.expire(&key, 60).await;
    }
    n <= max
}

/// 单步 RPC：把命令经 gateway 中继到节点并在服务端等回执。
/// Err("refused_offline") 表示节点离线，其余为命令失败原因
async fn node_rpc(
    state: &AppState,
    user_id: &str,
    machine_id: &str,
    session_id: &str,
    method: &str,
    payload: Value,
) -> Result<Value, String> {
    let command = json!({ "session_id": session_id, "type": method, "payload": payload });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let command_id =
        gateway::route_command(state, user_id, machine_id, &command, Some(tx)).await?;
    match tokio::time::timeout(std::time::Duration::from_secs(STEP_TIMEOUT_SECS), rx).await {
        Ok(Ok(reply)) => {
            if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                Ok(reply.get("result").cloned().unwrap_or(json!({})))
            } else {
                Err(reply
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("command failed")
                    .to_string())
            }
        }
        Ok(Err(_)) => Err("命令回执通道中断".into()),
        Err(_) => {
            state.pending_commands.lock().await.remove(&command_id);
            Err("timeout".into())
        }
    }
}

/// 按端点配置解析目标会话并投递消息；返回（session_id, 最终命令 command_id）。
/// IM 渠道复用同一编排：传入的文档只需具备同名字段
/// （machine_id/session_policy/fixed_session_id/expert_id/model/thinking_level/permission_mode/workspace/name）。
/// session_override 非空时跳过策略解析直接使用（IM auto 策略的群专属会话）
pub(crate) async fn deliver(
    state: &AppState,
    user_id: &str,
    ep: &BsonDoc,
    text: &str,
    session_override: Option<&str>,
) -> Result<(String, String), String> {
    let machine_id = ep.get_str("machine_id").unwrap_or_default();
    let policy = ep.get_str("session_policy").unwrap_or("latest");
    let ep_name = ep.get_str("name").unwrap_or("");

    let session_id = if let Some(sid) = session_override.filter(|s| !s.is_empty()) {
        sid.to_string()
    } else if policy == "new" || policy == "auto" {
        // 与自动化 execute_run 同序：建会话（绑专家/模型）→ 放宽权限 → 命名 → 发消息
        let created = node_rpc(
            state,
            user_id,
            machine_id,
            "",
            "session.create",
            json!({
                "cwd": ep.get_str("workspace").unwrap_or_default(),
                "model": ep.get_str("model").ok().filter(|s| !s.is_empty()),
                "expert_id": ep.get_str("expert_id").ok().filter(|s| !s.is_empty()),
                "thinking_level": ep.get_str("thinking_level").ok().filter(|s| !s.is_empty()),
                // 程序化创建来源：IM 渠道带 platform 字段，webhook 端点没有
                "source": if ep.get_str("platform").map(|p| !p.is_empty()).unwrap_or(false) { "im" } else { "webhook" },
            }),
        )
        .await?;
        let sid = created
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("session.create 未返回 session_id")?
            .to_string();
        // 专家可能自带建议权限模式；端点显式配置优先（plan 之外都显式下发）
        let pm = ep.get_str("permission_mode").unwrap_or("full");
        if pm != "plan" {
            node_rpc(
                state,
                user_id,
                machine_id,
                &sid,
                "session.set_permission_mode",
                json!({ "session_id": sid, "mode": pm }),
            )
            .await?;
        }
        node_rpc(
            state,
            user_id,
            machine_id,
            &sid,
            "session.rename",
            json!({ "session_id": sid, "name": ep_name }),
        )
        .await?;
        sid
    } else if policy == "fixed" {
        ep.get_str("fixed_session_id").unwrap_or_default().to_string()
    } else {
        // latest：节点最近活跃会话（镜像 sessions 按 updated_at 倒序取一）
        let s = state
            .mongo
            .collection::<BsonDoc>("sessions")
            .find_one(doc! { "user_id": user_id, "machine_id": machine_id })
            .with_options(
                mongodb::options::FindOneOptions::builder()
                    .sort(doc! { "updated_at": -1 })
                    .build(),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or("节点上没有可用会话（策略 latest）")?;
        s.get_str("_id").unwrap_or_default().to_string()
    };
    if session_id.is_empty() {
        return Err("未配置目标会话".into());
    }

    // 权限模式改由 session.create/既有会话决定，消息本体只投 agent.prompt
    let prompt = node_rpc(
        state,
        user_id,
        machine_id,
        &session_id,
        "agent.prompt",
        json!({ "session_id": session_id, "text": text }),
    )
    .await?;
    let command_id = prompt
        .get("command_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((session_id, command_id))
}

// ------------------------------------------------------------ 端点 CRUD

#[derive(Deserialize)]
struct EndpointBody {
    name: String,
    machine_id: String,
    /// new | latest | fixed
    #[serde(default)]
    session_policy: String,
    #[serde(default)]
    fixed_session_id: String,
    #[serde(default)]
    expert_id: String,
    #[serde(default)]
    model: String,
    /// off/minimal/low/medium/high；空 = 专家或默认
    #[serde(default)]
    thinking_level: String,
    /// 空 = full（与自动化无人值守同语义）
    #[serde(default)]
    permission_mode: String,
    /// 工作空间目录；空 = 主目录
    #[serde(default)]
    workspace: String,
    /// 完成回调地址；空 = 不回调（调用方仍可轮询 events）
    #[serde(default)]
    callback_url: String,
}

/// 建端点公共校验：名称/节点归属/策略组合。返回规整后的字段元组
async fn validate(
    state: &AppState,
    user_id: &str,
    body: &EndpointBody,
) -> Result<(), String> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err("请给端点起个名字".into());
    }
    if name.chars().count() > 64 {
        return Err("端点名称过长（最多 64 字符）".into());
    }
    // 节点必须存在且属于本人（防手滑 + 防指到别人节点）
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
    if !body.callback_url.trim().is_empty() && !valid_callback_url(body.callback_url.trim()) {
        return Err("callback_url 必须是 http(s) 地址且不含空白".into());
    }
    Ok(())
}

/// 回调地址白名单级校验：仅限 http(s)、无空白、长度可控。
/// SSRF 面 = 账号本人指定接收地址（与 CI webhook 通用做法一致），不做内网探测限制
fn valid_callback_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 2048
        && !u.contains(char::is_whitespace)
}

fn endpoint_json(d: &BsonDoc, hostname: &str) -> Value {
    json!({
        "id": d.get_str("_id").unwrap_or_default(),
        "name": d.get_str("name").unwrap_or_default(),
        "machine_id": d.get_str("machine_id").unwrap_or_default(),
        "machine_hostname": hostname,
        "session_policy": d.get_str("session_policy").unwrap_or("latest"),
        "fixed_session_id": d.get_str("fixed_session_id").unwrap_or_default(),
        "expert_id": d.get_str("expert_id").unwrap_or_default(),
        "model": d.get_str("model").unwrap_or_default(),
        "thinking_level": d.get_str("thinking_level").unwrap_or_default(),
        "permission_mode": d.get_str("permission_mode").unwrap_or("full"),
        "workspace": d.get_str("workspace").unwrap_or_default(),
        "callback_url": d.get_str("callback_url").unwrap_or_default(),
        "callback_secret": d.get_str("callback_secret").unwrap_or_default(),
        "enabled": d.get_bool("enabled").unwrap_or(true),
        "created_at": d.get_str("created_at").unwrap_or_default(),
    })
}

async fn endpoints_list(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let docs = state
        .mongo
        .collection::<BsonDoc>("webhook_endpoints")
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
    let mut endpoints_out = Vec::with_capacity(docs.len());
    for mut d in docs {
        // 早期端点可能没有 callback_secret：列表路径遇到即补发落库
        ensure_callback_secret(&state, &mut d).await;
        let mid = d.get_str("machine_id").unwrap_or_default();
        endpoints_out.push(endpoint_json(&d, machines.get(mid).map(String::as_str).unwrap_or("")));
    }
    Ok(Json(json!({ "endpoints": endpoints_out })))
}

async fn endpoints_create(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<EndpointBody>,
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
        .collection::<BsonDoc>("webhook_endpoints")
        .count_documents(doc! { "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if count >= MAX_ENDPOINTS_PER_USER as u64 {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            format!("最多创建 {MAX_ENDPOINTS_PER_USER} 个 webhook 端点，请先删除不用的"),
        ));
    }

    let ep_id = Uuid::now_v7().simple().to_string();
    // rand::random 单次取样即丢弃：ThreadRng 带 Drop 不能活过下面的 await
    let secret_bytes: [u8; 24] = rand::random();
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token = format!("{PREFIX}{ep_id}.{secret}");
    // 回调签名密钥：server 用它 HMAC 签回调请求，调用方验签；非准入凭证，明文落库
    let cb_secret_bytes: [u8; 16] = rand::random();
    let cb_secret: String = cb_secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let created_at = now_rfc3339();
    let d = doc! {
        "_id": &ep_id,
        "user_id": &user_id,
        "name": body.name.trim(),
        "token_hash": sha256_hex(&token),
        "machine_id": &body.machine_id,
        "session_policy": &body.session_policy,
        "fixed_session_id": body.fixed_session_id.trim(),
        "expert_id": body.expert_id.trim(),
        "model": body.model.trim(),
        "thinking_level": body.thinking_level.trim(),
        "permission_mode": if body.permission_mode.trim().is_empty() { "full" } else { body.permission_mode.trim() },
        "workspace": body.workspace.trim(),
        "callback_url": body.callback_url.trim(),
        "callback_secret": &cb_secret,
        "enabled": true,
        "created_at": &created_at,
        "updated_at": &created_at,
    };
    state
        .mongo
        .collection::<BsonDoc>("webhook_endpoints")
        .insert_one(d.clone())
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(user = %user_id, endpoint = %ep_id, "webhook endpoint created");
    let mut out = endpoint_json(&d, "");
    out["token"] = json!(token); // 明文只在创建响应出现一次
    Ok(Json(out))
}

/// PATCH：改配置 / 启停。字段缺省 = 保持不变；显式空串 = 清空该字段
#[derive(Deserialize)]
struct EndpointPatch {
    #[serde(default)]
    name: Option<String>,
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
    callback_url: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn endpoints_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
    Json(body): Json<EndpointPatch>,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let endpoints = state.mongo.collection::<BsonDoc>("webhook_endpoints");
    let existing = endpoints
        .find_one(doc! { "_id": &id, "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "webhook 端点不存在"))?;

    // 策略组合校验用合并后的值
    let policy = body
        .session_policy
        .as_deref()
        .unwrap_or(existing.get_str("session_policy").unwrap_or("latest"));
    let fixed_sid = body
        .fixed_session_id
        .as_deref()
        .map(str::trim)
        .unwrap_or(existing.get_str("fixed_session_id").unwrap_or_default());
    if !SESSION_POLICIES.contains(&policy) {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "无效的会话策略，可选：new / latest / fixed",
        ));
    }
    if policy == "fixed" && fixed_sid.is_empty() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "策略 fixed 需要提供 fixed_session_id",
        ));
    }

    let mut set = doc! { "updated_at": now_rfc3339() };
    if let Some(v) = &body.machine_id {
        // 换目标节点：同样要过归属校验
        let owned = state
            .mongo
            .collection::<BsonDoc>("machines")
            .count_documents(doc! { "_id": v, "user_id": &user_id })
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if owned == 0 {
            return Err(api_err(StatusCode::BAD_REQUEST, "目标节点不存在或不属于当前账号"));
        }
        set.insert("machine_id", v.as_str());
    }
    if let Some(v) = &body.name {
        let v = v.trim();
        if v.is_empty() || v.chars().count() > 64 {
            return Err(api_err(StatusCode::BAD_REQUEST, "端点名称不能为空且不超过 64 字符"));
        }
        set.insert("name", v);
    }
    if let Some(v) = &body.session_policy {
        set.insert("session_policy", v);
    }
    if let Some(v) = &body.fixed_session_id {
        set.insert("fixed_session_id", v.trim());
    }
    if let Some(v) = &body.expert_id {
        set.insert("expert_id", v.trim());
    }
    if let Some(v) = &body.model {
        set.insert("model", v.trim());
    }
    if let Some(v) = &body.thinking_level {
        set.insert("thinking_level", v.trim());
    }
    if let Some(v) = &body.permission_mode {
        set.insert("permission_mode", if v.trim().is_empty() { "full" } else { v.trim() });
    }
    if let Some(v) = &body.workspace {
        set.insert("workspace", v.trim());
    }
    if let Some(v) = &body.callback_url {
        let v = v.trim();
        if !v.is_empty() && !valid_callback_url(v) {
            return Err(api_err(StatusCode::BAD_REQUEST, "callback_url 必须是 http(s) 地址且不含空白"));
        }
        set.insert("callback_url", v);
    }
    if let Some(v) = body.enabled {
        set.insert("enabled", v);
    }
    endpoints
        .update_one(doc! { "_id": &id, "user_id": &user_id }, doc! { "$set": set })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

async fn endpoints_delete(
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
        .collection::<BsonDoc>("webhook_endpoints")
        .delete_one(doc! { "_id": &id, "user_id": user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if r.deleted_count == 0 {
        return Err(api_err(StatusCode::NOT_FOUND, "webhook 端点不存在"));
    }
    let mut conn = state.redis.clone();
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(format!("webhook:rl:{id}"))
        .query_async(&mut conn)
        .await;
    tracing::info!(endpoint = %id, "webhook endpoint deleted");
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------ 调用入口

#[derive(Deserialize)]
struct InvokeBody {
    text: String,
    /// 旧调用方把令牌放 body；标准走 Authorization 头。header 优先，body 兜底
    #[serde(default)]
    token: String,
    /// 本次调用的完成回调地址；空 = 用端点配置的 callback_url，两者皆空则不回调
    #[serde(default)]
    callback_url: String,
}

async fn invoke(
    State(state): State<AppState>,
    tok: crate::auth::BearerOpt,
    Json(body): Json<InvokeBody>,
) -> Result<Json<Value>, Response> {
    let token = tok
        .0
        .or_else(|| (!body.token.is_empty()).then(|| body.token.clone()))
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "missing token"))?;
    let (user_id, ep) = verify(&state, &token)
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?;
    let ep_id = ep.get_str("_id").unwrap_or_default().to_string();
    let machine_id = ep.get_str("machine_id").unwrap_or_default().to_string();

    if !rate_limit(&state, "webhook", &ep_id, RATE_LIMIT_PER_MIN).await {
        return Err(api_err(StatusCode::TOO_MANY_REQUESTS, "调用过于频繁，请稍后再试"));
    }
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "text 不能为空"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            format!("text 过长（最多 {MAX_TEXT_CHARS} 字符）"),
        ));
    }

    let started = std::time::Instant::now();
    match deliver(&state, &user_id, &ep, &text, None).await {
        Ok((session_id, command_id)) => {
            // 完成回调：调用级 URL 优先于端点配置；登记失败不影响投递结果
            let cb_url = if !body.callback_url.trim().is_empty() {
                body.callback_url.trim().to_string()
            } else {
                ep.get_str("callback_url").unwrap_or_default().to_string()
            };
            let mut callback_id = String::new();
            if !cb_url.is_empty() {
                if valid_callback_url(&cb_url) {
                    let mut ep_mutable = ep.clone();
                    let cb_secret = ensure_callback_secret(&state, &mut ep_mutable).await;
                    match register_pending(&state, &ep, &cb_secret, &cb_url, &session_id, &text).await {
                        Ok(id) => callback_id = id,
                        Err(e) => tracing::warn!(endpoint = %ep_id, "webhook 回调登记失败: {e}"),
                    }
                } else {
                    tracing::warn!(endpoint = %ep_id, "webhook callback_url 非法，跳过回调");
                }
            }
            audit(&state, &user_id, &ep, &machine_id, &session_id, "ok", started).await;
            Ok(Json(json!({
                "status": "ok",
                "session_id": session_id,
                "command_id": command_id,
                "callback": !callback_id.is_empty(),
                "callback_id": callback_id,
            })))
        }
        Err(e) => {
            let status = if e == "refused_offline" { "refused_offline" } else { "error" };
            audit(&state, &user_id, &ep, &machine_id, "", status, started).await;
            tracing::info!(endpoint = %ep_id, machine = %machine_id, "webhook invoke {status}: {e}");
            Ok(Json(json!({ "status": status, "error": e })))
        }
    }
}

/// 审计落 commands 集合（与 /commands、/rpc 同表，source 区分来源）
async fn audit(
    state: &AppState,
    user_id: &str,
    ep: &BsonDoc,
    machine_id: &str,
    session_id: &str,
    status: &str,
    started: std::time::Instant,
) {
    let _ = state
        .mongo
        .collection::<BsonDoc>("commands")
        .insert_one(doc! {
            "command_id": Uuid::now_v7().to_string(),
            "user_id": user_id,
            "machine_id": machine_id,
            "session_id": session_id,
            "type": "webhook.invoke",
            "payload": ep.get_str("name").unwrap_or_default(),
            "source": "webhook",
            "endpoint_id": ep.get_str("_id").unwrap_or_default(),
            "status": status,
            "duration_ms": started.elapsed().as_millis() as i64,
            "ts": now_rfc3339(),
        })
        .await;
}

// ------------------------------------------------------------ 完成回调投递

/// 完成回调机制：invoke 登记挂起记录（`webhook_pending_callbacks`），agent
/// 空闲（`agent_state_changed` state=idle，经上行白名单进 ingest）即视为该轮
/// 结束——认领记录后异步 POST 回调地址，body 携带最后一轮 assistant 输出；
/// 超时未结束由后台清扫兜底投递 timeout 通知。请求以端点 callback_secret
/// HMAC-SHA256 签名（`X-PiDock-Signature: sha256=<hex>`），重试 3 次。
/// 共享会话（latest/fixed）若期间被人工插话，回调可能携带后续轮次的输出——
/// 专属会话（policy=new）无此问题；已知取舍，精确关联留给按需演进。

/// 早期创建的端点没有 callback_secret：列表/调用路径遇到即补发落库
async fn ensure_callback_secret(state: &AppState, d: &mut BsonDoc) -> String {
    let existing = d.get_str("callback_secret").unwrap_or_default().to_string();
    if !existing.is_empty() {
        return existing;
    }
    let secret_bytes: [u8; 16] = rand::random();
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = state
        .mongo
        .collection::<BsonDoc>("webhook_endpoints")
        .update_one(
            doc! { "_id": d.get_str("_id").unwrap_or_default() },
            doc! { "$set": { "callback_secret": &secret } },
        )
        .await;
    d.insert("callback_secret", secret.clone());
    secret
}

async fn register_pending(
    state: &AppState,
    ep: &BsonDoc,
    cb_secret: &str,
    callback_url: &str,
    session_id: &str,
    text: &str,
) -> Result<String, String> {
    let cb_id = Uuid::now_v7().to_string();
    let now = now_rfc3339();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(CALLBACK_TTL_SECS)).to_rfc3339();
    state
        .mongo
        .collection::<BsonDoc>("webhook_pending_callbacks")
        .insert_one(doc! {
            "_id": &cb_id,
            "kind": "webhook",
            "endpoint_id": ep.get_str("_id").unwrap_or_default(),
            "user_id": ep.get_str("user_id").unwrap_or_default(),
            "machine_id": ep.get_str("machine_id").unwrap_or_default(),
            "session_id": session_id,
            "callback_url": callback_url,
            "callback_secret": cb_secret,
            "text": text,
            "status": "pending",
            "created_at": &now,
            "expires_at": &expires_at,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(cb_id)
}

/// ingest 挂钩：agent 转入空闲 = 一轮结束。原子认领挂起回调后交给独立任务
/// 投递——ingest 是单 worker 保序消费，绝不能在这里等 HTTP
pub async fn on_turn_idle(state: AppState, session_id: &str) {
    let claimed = state
        .mongo
        .collection::<BsonDoc>("webhook_pending_callbacks")
        .find_one_and_update(
            doc! {
                "session_id": session_id,
                "status": "pending",
                "expires_at": doc! { "$gt": now_rfc3339() },
            },
            doc! { "$set": { "status": "delivering", "claimed_at": now_rfc3339() } },
        )
        .with_options(
            mongodb::options::FindOneAndUpdateOptions::builder()
                .return_document(mongodb::options::ReturnDocument::After)
                .build(),
        )
        .await;
    match claimed {
        Ok(Some(cb)) => {
            tokio::spawn(async move { post_callback(&state, &cb, false).await });
        }
        // 认领不到 pending = 这轮不是 IM/webhook 触发：若是群绑定的会话
        // （auto 策略），把 agent 最终回复转发到群（双向同步）
        Ok(None) => {
            tokio::spawn(crate::im::forward_session_activity(state, session_id.to_string()));
        }
        Err(e) => tracing::warn!("webhook 回调认领失败: {e}"),
    }
}

/// 后台清扫：建索引 + 兜底投递超时回调（30s 一轮）
pub async fn run_maintenance(state: AppState) {
    let pending = state.mongo.collection::<BsonDoc>("webhook_pending_callbacks");
    let index = mongodb::IndexModel::builder()
        .keys(doc! { "session_id": 1, "status": 1 })
        .build();
    if let Err(e) = pending.create_index(index).await {
        tracing::warn!("webhook 回调索引创建失败: {e}");
    }
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let expired = pending
            .find(doc! {
                "status": "pending",
                "expires_at": doc! { "$lte": now_rfc3339() },
            })
            .await;
        let Ok(cursor) = expired else {
            continue;
        };
        let Ok(docs) = cursor.try_collect::<Vec<BsonDoc>>().await else {
            continue;
        };
        for cb in docs {
            let claimed = pending
                .find_one_and_update(
                    doc! { "_id": cb.get_str("_id").unwrap_or_default(), "status": "pending" },
                    doc! { "$set": { "status": "delivering", "claimed_at": now_rfc3339() } },
                )
                .with_options(
                    mongodb::options::FindOneAndUpdateOptions::builder()
                        .return_document(mongodb::options::ReturnDocument::After)
                        .build(),
                )
                .await;
            if let Ok(Some(cb)) = claimed {
                let st = state.clone();
                tokio::spawn(async move { post_callback(&st, &cb, true).await });
            }
        }
    }
}

/// 会话最近一条 assistant 消息（取最近 10 条 message_complete 解析挑选）
/// 会话最近一条 assistant 消息（取最近 10 条 message_complete 解析挑选）。
/// IM 回复通道复用同一提取
pub(crate) async fn fetch_last_assistant(state: &AppState, session_id: &str) -> Option<Value> {
    let docs = state
        .mongo
        .collection::<BsonDoc>("session_events")
        .find(doc! { "session_id": session_id, "kind": "message_complete" })
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! { "seq": -1 })
                .limit(10)
                .build(),
        )
        .await
        .ok()?
        .try_collect::<Vec<BsonDoc>>()
        .await
        .ok()?;
    for d in docs {
        let payload: Value = match serde_json::from_str(d.get_str("payload").unwrap_or_default()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if payload.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let text = payload
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
            .unwrap_or_default();
        return Some(json!({
            "role": "assistant",
            "text": text,
            "stop_reason": payload.get("stopReason").cloned().unwrap_or(Value::Null),
            "model": payload.get("model").cloned().unwrap_or(Value::Null),
            "duration_ms": payload.get("duration_ms").cloned().unwrap_or(Value::Null),
        }));
    }
    None
}

fn sign_callback(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key 非空");
    mac.update(body);
    format!("sha256={:x}", mac.finalize().into_bytes())
}

/// 投递分发：按 pending 记录的 kind 路由到对应回复通道
/// （webhook = POST callback_url；其余 = IM，经 im::dispatch_reply 按注册
/// 表找平台适配器）。timed_out = 清扫超时兜底，投递内容为超时提示而非任务结果
pub(crate) async fn post_callback(state: &AppState, cb: &BsonDoc, timed_out: bool) {
    match cb.get_str("kind").unwrap_or("webhook") {
        "webhook" => post_webhook_callback(state, cb, timed_out).await,
        other => {
            let kind = other.strip_prefix("im_").unwrap_or(other); // 兼容历史 im_feishu 记录
            crate::im::dispatch_reply(state, kind, cb, timed_out).await;
        }
    }
}

/// 投递一次 webhook 回调（带重试）
async fn post_webhook_callback(state: &AppState, cb: &BsonDoc, timed_out: bool) {
    let cb_id = cb.get_str("_id").unwrap_or_default().to_string();
    let url = cb.get_str("callback_url").unwrap_or_default().to_string();
    let secret = cb.get_str("callback_secret").unwrap_or_default().to_string();
    let session_id = cb.get_str("session_id").unwrap_or_default().to_string();
    let (status, message) = if timed_out {
        ("timeout", Value::Null)
    } else {
        match fetch_last_assistant(state, &session_id).await {
            Some(m) => ("ok", m),
            None => ("no_output", Value::Null),
        }
    };
    let payload = json!({
        "event": "webhook.completed",
        "status": status,
        "delivery_id": cb_id,
        "endpoint_id": cb.get_str("endpoint_id").unwrap_or_default(),
        "session_id": session_id,
        "invoke_text": cb.get_str("text").unwrap_or_default(),
        "message": message,
        "ts": now_rfc3339(),
    });
    let body = serde_json::to_vec(&payload).unwrap_or_default();
    let signature = sign_callback(&secret, &body);

    let client = match crate::http::base_client(&state.cfg)
        .and_then(|b| {
            b.timeout(std::time::Duration::from_secs(CALLBACK_HTTP_TIMEOUT_SECS))
                .build()
                .map_err(|e| e.to_string())
        }) {
        Ok(c) => c,
        Err(e) => {
            finish_pending(state, &cb_id, "failed", 0, None, &format!("http client: {e}")).await;
            return;
        }
    };
    let mut attempts = 0u32;
    let mut last_code: Option<u16> = None;
    let mut last_error = String::new();
    for i in 0..=CALLBACK_RETRY_BACKOFF_SECS.len() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(CALLBACK_RETRY_BACKOFF_SECS[i - 1])).await;
        }
        attempts += 1;
        match client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-pidock-event", "webhook.completed")
            .header("x-pidock-delivery", &cb_id)
            .header("x-pidock-signature", &signature)
            .body(body.clone())
            .send()
            .await
        {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() {
                    finish_pending(state, &cb_id, "delivered", attempts, Some(code), "").await;
                    tracing::info!(delivery = %cb_id, code, "webhook 回调投递成功");
                    return;
                }
                last_code = Some(code);
                last_error = format!("HTTP {code}");
            }
            Err(e) => {
                last_error = e.to_string();
            }
        }
    }
    finish_pending(state, &cb_id, "failed", attempts, last_code, &last_error).await;
    tracing::warn!(delivery = %cb_id, url = %url, "webhook 回调投递失败: {last_error}");
}

pub(crate) async fn finish_pending(
    state: &AppState,
    cb_id: &str,
    status: &str,
    attempts: u32,
    last_code: Option<u16>,
    last_error: &str,
) {
    let mut set = doc! {
        "status": status,
        "attempts": attempts as i64,
        "finished_at": now_rfc3339(),
    };
    if let Some(code) = last_code {
        set.insert("last_status_code", code as i64);
    }
    if !last_error.is_empty() {
        set.insert("last_error", last_error);
    }
    let _ = state
        .mongo
        .collection::<BsonDoc>("webhook_pending_callbacks")
        .update_one(doc! { "_id": cb_id }, doc! { "$set": set })
        .await;
}

/// 重新生成端点令牌：端点 id（jti）不变，只换 secret 并更新哈希——
/// 令牌明文只存哈希无法找回，丢失时从这里换新，配置与历史引用不受影响
async fn endpoints_regenerate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = crate::auth::verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let endpoints = state.mongo.collection::<BsonDoc>("webhook_endpoints");
    let exists = endpoints
        .count_documents(doc! { "_id": &id, "user_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if exists == 0 {
        return Err(api_err(StatusCode::NOT_FOUND, "webhook 端点不存在"));
    }
    let secret_bytes: [u8; 24] = rand::random();
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token = format!("{PREFIX}{id}.{secret}");
    endpoints
        .update_one(
            doc! { "_id": &id, "user_id": &user_id },
            doc! { "$set": { "token_hash": sha256_hex(&token), "updated_at": now_rfc3339() } },
        )
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(user = %user_id, endpoint = %id, "webhook endpoint token regenerated");
    Ok(Json(json!({ "id": id, "token": token })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(endpoints_list).post(endpoints_create))
        .route("/webhooks/{id}", patch(endpoints_update).delete(endpoints_delete))
        .route("/webhooks/{id}/regenerate", post(endpoints_regenerate))
        .route("/webhooks/invoke", post(invoke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format_roundtrip() {
        let ep_id = Uuid::now_v7().simple().to_string();
        assert_eq!(ep_id.len(), 32);
        let token = format!("{PREFIX}{ep_id}.0123abcd");
        let (jti, secret) = token
            .strip_prefix(PREFIX)
            .and_then(|r| r.split_once('.'))
            .unwrap();
        assert_eq!(jti, ep_id);
        assert_eq!(secret, "0123abcd");
        // 非 hex / 长度不对的 jti 应被 verify 前置格式检查思路拒绝
        assert!("xyz".len() != 32);
    }

    #[test]
    fn callback_url_validation() {
        assert!(valid_callback_url("https://ci.example.com/hook"));
        assert!(valid_callback_url("http://127.0.0.1:9000/cb"));
        assert!(!valid_callback_url("ftp://x/y"));
        assert!(!valid_callback_url("https://x/y with space"));
        assert!(!valid_callback_url("javascript:alert(1)"));
        assert!(!valid_callback_url(&format!("https://a/{}", "x".repeat(3000))));
    }

    #[test]
    fn callback_signature_is_deterministic_hmac() {
        let s1 = sign_callback("secret", b"hello");
        let s2 = sign_callback("secret", b"hello");
        let s3 = sign_callback("secret", b"world");
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
        assert!(s1.starts_with("sha256="));
        assert_eq!(s1["sha256=".len()..].len(), 64); // hex sha256
    }
}
