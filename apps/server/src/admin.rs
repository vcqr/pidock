//! Admin：用户管理（列表/角色/禁用/删除）+ 登录方式（LDAP/OIDC）配置端点。
//! 鉴权走 Authorization: Bearer（Bearer 提取器，query ?token= 兼容回落）；
//! 每个 handler 手动调 verify_admin（每请求查库，改角色立即生效）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{get, patch, post},
    Json, Router,
};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document as BsonDoc},
    options::UpdateOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{api_err, hash_password, valid_email, valid_password, verify_admin, Bearer};
use crate::ldap::LdapCfg;
use crate::sso::OidcCfg;
use crate::state::AppState;

fn default_true() -> bool {
    true
}

/// 注册策略（auth_config 集合 _id="policy"）：关闭后 /auth/register 拒绝，
/// 账号改由管理员在后台创建；bootstrap 引导码不受影响（救急通道）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegPolicy {
    #[serde(default = "default_true")]
    pub allow_register: bool,
}

impl Default for RegPolicy {
    fn default() -> Self {
        Self { allow_register: true }
    }
}

pub async fn load_policy(state: &AppState) -> RegPolicy {
    load_cfg::<RegPolicy>(state, "policy").await
}

// ---------------------------------------------------- auth_config 读写

/// 从 auth_config 读配置；缺文档 = 默认值（未启用），读失败仅告警并按默认处理
pub async fn load_cfg<T: serde::de::DeserializeOwned + Default>(state: &AppState, id: &str) -> T {
    match state
        .mongo
        .collection::<BsonDoc>("auth_config")
        .find_one(doc! { "_id": id })
        .await
    {
        Ok(Some(d)) => mongodb::bson::de::from_document::<T>(d).unwrap_or_else(|e| {
            tracing::warn!("auth_config[{id}] 解析失败，按默认配置处理: {e}");
            T::default()
        }),
        Ok(None) => T::default(),
        Err(e) => {
            tracing::warn!("auth_config[{id}] 读取失败，按默认配置处理: {e}");
            T::default()
        }
    }
}

pub async fn load_ldap_cfg(state: &AppState) -> LdapCfg {
    load_cfg::<LdapCfg>(state, "ldap").await
}

pub async fn load_oidc_cfg(state: &AppState) -> OidcCfg {
    load_cfg::<OidcCfg>(state, "oidc").await
}

async fn save_cfg<T: serde::Serialize>(
    state: &AppState,
    id: &str,
    cfg: &T,
) -> Result<(), Response> {
    let mut d = mongodb::bson::to_document(cfg)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    d.insert("_id", id);
    d.insert("updated_at", chrono::Utc::now().to_rfc3339());
    state
        .mongo
        .collection::<BsonDoc>("auth_config")
        .update_one(
            doc! { "_id": id },
            doc! { "$set": d },
        )
        .with_options(UpdateOptions::builder().upsert(true).build())
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------- 用户管理

async fn users_list(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let docs = state
        .mongo
        .collection::<BsonDoc>("users")
        .find(doc! {})
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! { "created_at": -1 })
                .build(),
        )
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let out: Vec<Value> = docs
        .iter()
        .map(|d| {
            json!({
                "user_id": d.get_str("_id").unwrap_or_default(),
                "email": d.get_str("email").unwrap_or_default(),
                "role": d.get_str("role").unwrap_or("user"),
                "auth_source": d.get_str("auth_source").unwrap_or("local"),
                "disabled": d.get_bool("disabled").unwrap_or(false),
                "display_name": d.get_str("display_name").unwrap_or_default(),
                "created_at": d.get_str("created_at").unwrap_or_default(),
                "last_login_at": d.get_str("last_login_at").unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(json!({ "users": out })))
}

#[derive(Deserialize)]
struct UserPatchBody {
    /// "admin" | "user"
    role: Option<String>,
    disabled: Option<bool>,
}

#[derive(Deserialize)]
struct UserCreateBody {
    email: String,
    /// 8~128 位，由管理员交付给用户
    password: String,
    /// 缺省 "user"
    #[serde(default)]
    role: String,
}

/// 管理员后台建号：绕过邀请码（注册关闭时的账户开通通道）
async fn users_create(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<UserCreateBody>,
) -> Result<Json<Value>, Response> {
    let admin_id = verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let email = body.email.trim().to_lowercase();
    if !valid_email(&body.email) {
        return Err(api_err(StatusCode::BAD_REQUEST, "邮箱格式不正确"));
    }
    if !valid_password(&body.password) {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "密码需 8~128 位，且同时包含字母和数字",
        ));
    }
    let role = if body.role.is_empty() { "user" } else { body.role.as_str() };
    if role != "user" && role != "admin" {
        return Err(api_err(StatusCode::BAD_REQUEST, "role 只能是 admin 或 user"));
    }
    let hash = hash_password(&body.password)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let user_id = uuid::Uuid::now_v7().to_string();
    let user: mongodb::bson::Document = mongodb::bson::doc! {
        "_id": &user_id,
        "email": &email,
        "password_hash": &hash,
        "role": role,
        "auth_source": "local",
        "created_at": chrono::Utc::now().to_rfc3339(),
    };
    let users = state.mongo.collection::<BsonDoc>("users");
    if let Err(e) = users.insert_one(user).await {
        if e.to_string().contains("duplicate") {
            return Err(api_err(StatusCode::CONFLICT, "该邮箱已注册"));
        }
        return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    tracing::info!(email = %email, role, admin = %admin_id, "user created by admin");
    Ok(Json(json!({ "user_id": user_id, "email": email, "role": role })))
}

// ---------------------------------------------------- 注册策略

#[derive(Deserialize)]
struct PolicyPutBody {
    #[serde(flatten)]
    cfg: RegPolicy,
}

async fn policy_get(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let policy = load_policy(&state).await;
    Ok(Json(json!({ "config": policy })))
}

async fn policy_put(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<PolicyPutBody>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    save_cfg(&state, "policy", &body.cfg).await?;
    tracing::info!("register policy updated (allow_register={})", body.cfg.allow_register);
    Ok(Json(json!({ "ok": true })))
}

async fn users_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
    Json(body): Json<UserPatchBody>,
) -> Result<Json<Value>, Response> {
    let admin_id = verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    if body.role.is_none() && body.disabled.is_none() {
        return Err(api_err(StatusCode::BAD_REQUEST, "没有要修改的字段"));
    }
    if let Some(role) = &body.role {
        if role != "admin" && role != "user" {
            return Err(api_err(StatusCode::BAD_REQUEST, "role 只能是 admin 或 user"));
        }
    }
    if id == admin_id {
        return Err(api_err(StatusCode::BAD_REQUEST, "不能修改自己的账号"));
    }
    let users = state.mongo.collection::<BsonDoc>("users");
    let target = users
        .find_one(doc! { "_id": &id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "用户不存在"))?;
    let target_is_admin = target.get_str("role").unwrap_or("user") == "admin";
    // 降级/禁用最后一个管理员会让管理端失去控制权，拒绝
    let losing_admin =
        target_is_admin && (body.role.as_deref() == Some("user") || body.disabled == Some(true));
    if losing_admin {
        let others = users
            .count_documents(doc! { "role": "admin", "_id": doc! { "$ne": &id } })
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if others == 0 {
            return Err(api_err(StatusCode::BAD_REQUEST, "至少需要保留一个管理员"));
        }
    }
    let mut set = doc! {};
    if let Some(role) = &body.role {
        set.insert("role", role);
    }
    if let Some(d) = body.disabled {
        set.insert("disabled", d);
    }
    users
        .update_one(doc! { "_id": &id }, doc! { "$set": set })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(target = %id, admin = %admin_id, "user updated by admin");
    Ok(Json(json!({ "ok": true })))
}

async fn users_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let admin_id = verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    if id == admin_id {
        return Err(api_err(StatusCode::BAD_REQUEST, "不能删除自己的账号"));
    }
    let users = state.mongo.collection::<BsonDoc>("users");
    let target = users
        .find_one(doc! { "_id": &id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "用户不存在"))?;
    if target.get_str("role").unwrap_or("user") == "admin" {
        let others = users
            .count_documents(doc! { "role": "admin", "_id": doc! { "$ne": &id } })
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if others == 0 {
            return Err(api_err(StatusCode::BAD_REQUEST, "至少需要保留一个管理员"));
        }
    }
    // 只删账号记录；machines/sessions 数据保留归档
    users
        .delete_one(doc! { "_id": &id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(target = %id, admin = %admin_id, "user deleted by admin");
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------- 登录方式配置

#[derive(Deserialize)]
struct LdapPutBody {
    #[serde(flatten)]
    cfg: LdapCfg,
}

async fn ldap_cfg_get(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let cfg = load_ldap_cfg(&state).await;
    Ok(Json(json!({ "config": cfg })))
}

async fn ldap_cfg_put(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<LdapPutBody>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    body.cfg
        .validate()
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    save_cfg(&state, "ldap", &body.cfg).await?;
    tracing::info!("ldap auth config updated (enabled={})", body.cfg.enabled);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct OidcPutBody {
    #[serde(flatten)]
    cfg: OidcCfg,
}

async fn oidc_cfg_get(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let cfg = load_oidc_cfg(&state).await;
    Ok(Json(json!({ "config": cfg })))
}

async fn oidc_cfg_put(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<OidcPutBody>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    body.cfg
        .validate()
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    save_cfg(&state, "oidc", &body.cfg).await?;
    tracing::info!("oidc auth config updated (enabled={})", body.cfg.enabled);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct LdapTestBody {
    /// 可选：用来验证 user_filter 与用户绑定
    #[serde(default)]
    username: String,
    /// 可选：配合 username 做用户密码验证
    #[serde(default)]
    password: String,
}

async fn ldap_test(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<LdapTestBody>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let cfg = load_ldap_cfg(&state).await;
    if cfg.server_url.trim().is_empty() || cfg.base_dn.trim().is_empty() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "请先保存完整的 LDAP 配置（服务器地址与 Base DN）",
        ));
    }
    match crate::ldap::test(&cfg, &body.username, &body.password).await {
        Ok(msg) => Ok(Json(json!({ "ok": true, "message": msg }))),
        Err(e) => Err(api_err(StatusCode::BAD_GATEWAY, e)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/users",
            post(users_create).get(users_list),
        )
        .route(
            "/admin/users/{id}",
            patch(users_patch).delete(users_delete),
        )
        .route("/admin/auth/policy", get(policy_get).put(policy_put))
        .route("/admin/auth/ldap", get(ldap_cfg_get).put(ldap_cfg_put))
        .route("/admin/auth/oidc", get(oidc_cfg_get).put(oidc_cfg_put))
        .route("/admin/auth/ldap/test", post(ldap_test))
}
