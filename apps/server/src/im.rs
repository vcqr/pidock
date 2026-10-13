//! IM 接入：外部 IM 平台（飞书先行，平台适配见 `feishu` 子模块）作为触发源
//! 与回复通道。渠道 = Mongo `im_channels` 文档：平台凭证（app_id/app_secret/
//! verification_token/encrypt_key）+ 目标配置（节点/专家/会话策略——与
//! webhook_endpoints 同构字段，直接复用 webhook::deliver 编排）。
//!
//! 入站：`POST /im/feishu/event/{channel_id}`（URL 里的渠道 id 即弱凭证，
//! 叠加平台侧 verification_token/签名校验）。事件验证通过后 message_id
//! 去重 → 复用 deliver 投递节点 → 立即 200（飞书 3s 超时，绝不同步等
//! agent）；回复在 agent 空闲时经 webhook::post_callback 的 kind 分发回到
//! 平台消息 API（pending 记录 kind=im_feishu + reply_chat_id）。
//!
//! 鉴权语义与 webhook 不同：pw_ 令牌即账号身份；IM 的触发方是平台服务器，
//! 事件里没有 PiDock 身份——渠道归属 = 配置它的账号，任何人在飞书里
//! @机器人都可能触发，因此权限模式建议保守（UI 默认 edit-auto）。

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Response, Result},
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

pub(crate) mod feishu;

/// 平台白名单（v1 只有飞书；钉钉/企微等新增时扩展）
const PLATFORMS: &[&str] = &["feishu"];
/// 每渠道每分钟入站事件上限（飞书正常流量远低于此，防滥用/打挂）
const RATE_LIMIT_PER_MIN: i64 = 120;
/// 每用户最多渠道数
const MAX_CHANNELS_PER_USER: usize = 10;
const SESSION_POLICIES: &[&str] = &["new", "latest", "fixed"];

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ------------------------------------------------------------ 渠道 CRUD

#[derive(Deserialize)]
struct ChannelBody {
    platform: String,
    name: String,
    app_id: String,
    app_secret: String,
    /// 可选；配置后飞书推送加密体，服务端验签 + AES-256-CBC 解密
    #[serde(default)]
    encrypt_key: String,
    /// 防伪必需：飞书推送里携带，必须与本值一致
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

/// 建/改公共校验。返回规整后的 permission_mode 默认值
async fn validate(state: &AppState, user_id: &str, body: &ChannelBody) -> Result<(), String> {
    if !PLATFORMS.contains(&body.platform.as_str()) {
        return Err(format!("暂不支持的平台，可选：{}", PLATFORMS.join(" / ")));
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err("渠道名称不能为空且不超过 64 字符".into());
    }
    if body.app_id.trim().is_empty() || body.app_secret.trim().is_empty() {
        return Err("App ID 与 App Secret 不能为空".into());
    }
    if body.verification_token.trim().is_empty() {
        return Err("Verification Token 不能为空（飞书开发者后台「事件与回调 → 加密策略」获取）".into());
    }
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

fn default_perm(p: &str) -> &str {
    if p.trim().is_empty() {
        "edit-auto"
    } else {
        p.trim()
    }
}

fn channel_json(d: &BsonDoc, hostname: &str) -> Value {
    json!({
        "id": d.get_str("_id").unwrap_or_default(),
        "platform": d.get_str("platform").unwrap_or_default(),
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
    Ok(Json(json!({ "channels": out })))
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
    // 策略组合用合并后的值校验
    let merged = ChannelBody {
        platform: existing.get_str("platform").unwrap_or("feishu").to_string(),
        name: body.name.clone().unwrap_or_else(|| existing.get_str("name").unwrap_or_default().to_string()),
        app_id: body.app_id.clone().unwrap_or_else(|| existing.get_str("app_id").unwrap_or_default().to_string()),
        app_secret: body
            .app_secret
            .clone()
            .unwrap_or_else(|| existing.get_str("app_secret").unwrap_or_default().to_string()),
        encrypt_key: body
            .encrypt_key
            .clone()
            .unwrap_or_else(|| existing.get_str("encrypt_key").unwrap_or_default().to_string()),
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

// ------------------------------------------------------------ 入站事件

/// 飞书事件订阅入口（公网回调地址）。channel_id 在路径里 = 弱凭证 +
/// 确定性路由；防伪主力是 verification_token / Encrypt Key 签名。
async fn feishu_event(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let channel = match state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .find_one(doc! { "_id": &channel_id, "platform": "feishu" })
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
    feishu::handle_event(&state, &channel, &headers, &body).await
}

use axum::response::IntoResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/channels", get(channels_list).post(channels_create))
        .route("/im/channels/{id}", patch(channels_update).delete(channels_delete))
        .route("/im/feishu/event/{channel_id}", post(feishu_event))
}
