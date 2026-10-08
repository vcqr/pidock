//! Auth: register / login / refresh with argon2 + JWT (access 15min,
//! refresh 30d stored in Redis and revocable per-device).

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use futures_util::TryStreamExt;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use mongodb::bson::{doc, Document as BsonDoc};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

const ACCESS_TTL_MIN: i64 = 15;
const REFRESH_TTL_DAYS: i64 = 30;

pub(crate) fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": msg.into()}))).into_response()
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,  // user id
    pub kind: String, // "access" | "refresh"
    pub jti: String,
    pub exp: i64,
    /// 个人访问令牌（pd_ 前缀）等价出的 access claim，非 JWT 签发
    #[serde(default)]
    pub pat: bool,
}

pub fn make_token(
    state: &AppState,
    user_id: &str,
    kind: &str,
) -> Result<(String, String, i64), String> {
    let jti = Uuid::now_v7().to_string();
    let exp = if kind == "access" {
        (Utc::now() + Duration::minutes(ACCESS_TTL_MIN)).timestamp()
    } else {
        (Utc::now() + Duration::days(REFRESH_TTL_DAYS)).timestamp()
    };
    let token = encode(
        &Header::default(),
        &Claims {
            sub: user_id.into(),
            kind: kind.into(),
            jti: jti.clone(),
            exp,
            pat: false,
        },
        &EncodingKey::from_secret(state.cfg.jwt_secret.as_bytes()),
    )
    .map_err(|e| e.to_string())?;
    Ok((token, jti, exp))
}

pub async fn verify_token(
    state: &AppState,
    token: &str,
    expected_kind: &str,
) -> Result<Claims, String> {
    // 个人访问令牌走独立校验（Redis 白名单），不经 JWT 解码
    if expected_kind == "access" && token.starts_with(crate::pat::PAT_PREFIX) {
        return crate::pat::verify(state, token).await;
    }
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.cfg.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| format!("invalid token: {e}"))?;
    if data.claims.kind != expected_kind {
        return Err(format!("expected {} token", expected_kind));
    }
    if expected_kind == "refresh" {
        let mut conn = state.redis.clone();
        let known: Option<String> = redis::cmd("GET")
            .arg(format!("auth:refresh:{}", data.claims.jti))
            .query_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        if known.is_none() {
            return Err("refresh token revoked or expired".into());
        }
    }
    Ok(data.claims)
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[derive(Deserialize)]
pub struct RegisterBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub invite_code: String,
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> Result<Json<Value>, Response> {
    if !body.email.contains('@') {
        return Err(api_err(StatusCode::BAD_REQUEST, "邮箱格式不正确"));
    }
    if body.password.chars().count() < 8 {
        return Err(api_err(StatusCode::BAD_REQUEST, "密码至少需要 8 个字符"));
    }
    let code = norm_code(&body.invite_code);
    if code.is_empty() {
        return Err(api_err(StatusCode::FORBIDDEN, "注册需要邀请码"));
    }
    let users = state.mongo.collection::<BsonDoc>("users");
    let user_id = Uuid::now_v7().to_string();
    // 引导邀请码（配置下发）仅在没有管理员时有效，用完即废，不落 invites 集合
    let mut role = "user";
    if !state.cfg.bootstrap_invite.is_empty() && code == norm_code(&state.cfg.bootstrap_invite) {
        let admins = users
            .count_documents(doc! { "role": "admin" })
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if admins > 0 {
            return Err(api_err(StatusCode::FORBIDDEN, "引导邀请码已失效"));
        }
        role = "admin";
    } else {
        claim_invite(&state, &code, &user_id)
            .await
            .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
    }
    let hash =
        hash_password(&body.password).map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let user: BsonDoc = doc! {
        "_id": &user_id,
        "email": body.email.to_lowercase(),
        "password_hash": &hash,
        "role": role,
        "created_at": Utc::now().to_rfc3339(),
    };
    if let Err(e) = users.insert_one(user).await {
        if e.to_string().contains("duplicate") {
            // 落库失败（基本只会是邮箱重复）把认领掉的次数退回去，别白扣名额
            let _ = state
                .mongo
                .collection::<BsonDoc>("invites")
                .update_one(
                    doc! { "_id": &code },
                    doc! { "$inc": { "used_count": -1 }, "$pull": { "used_by": &user_id } },
                )
                .await;
            return Err(api_err(StatusCode::CONFLICT, "该邮箱已注册"));
        }
        return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    tracing::info!(user = %user_id, role, "user registered");
    Ok(Json(serde_json::json!({"user_id": user_id, "role": role})))
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, Response> {
    let users = state.mongo.collection::<BsonDoc>("users");
    let email_lc = body.email.trim().to_lowercase();
    let user = users
        .find_one(doc! {"email": &email_lc})
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"))?;
    if user.get_bool("disabled").unwrap_or(false) {
        return Err(api_err(StatusCode::FORBIDDEN, "账号已被禁用"));
    }
    let hash = user.get_str("password_hash").ok().unwrap_or_default();
    if hash.is_empty() {
        // 无本地密码的账号（LDAP/SSO 建）指路对应的登录方式
        let hint = match user.get_str("auth_source").unwrap_or("local") {
            "ldap" => "该账号为 LDAP 账号，请使用 LDAP 登录",
            "oidc" => "该账号为 SSO 账号，请使用 SSO 登录",
            _ => "该账号未设置本地密码",
        };
        return Err(api_err(StatusCode::UNAUTHORIZED, hint));
    }
    if !verify_password(&body.password, hash) {
        return Err(api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"));
    }
    let user_id = user.get_str("_id").unwrap_or_default().to_string();
    let role = user.get_str("role").unwrap_or("user").to_string();
    finish_login(&state, &user_id, &role, None).await
}

#[derive(Deserialize)]
pub struct LoginLdapBody {
    pub username: String,
    pub password: String,
}

/// LDAP 独立登录：与标准登录彻底分开，报错语义各自纯粹
async fn login_ldap(
    State(state): State<AppState>,
    Json(body): Json<LoginLdapBody>,
) -> Result<Json<Value>, Response> {
    let cfg = crate::admin::load_ldap_cfg(&state).await;
    if !cfg.enabled {
        return Err(api_err(StatusCode::NOT_FOUND, "LDAP 登录未启用"));
    }
    match crate::ldap::authenticate(&cfg, body.username.trim(), &body.password).await {
        Ok(lu) => {
            let (user_id, role) =
                external_upsert(&state, "ldap", &lu.email, &lu.name, None, cfg.allow_register)
                    .await
                    .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
            finish_login(&state, &user_id, &role, Some(&lu.email)).await
        }
        Err(crate::ldap::LdapAuthError::Credentials) => {
            Err(api_err(StatusCode::UNAUTHORIZED, "LDAP 用户名或密码错误"))
        }
        Err(crate::ldap::LdapAuthError::Unavailable(e)) => {
            tracing::warn!("ldap login unavailable: {e}");
            Err(api_err(StatusCode::BAD_GATEWAY, "LDAP 服务暂时不可用"))
        }
    }
}

/// 登录成功统一出口：发 token 对 + 记录 last_login_at + 附角色（可选附邮箱）
async fn finish_login(
    state: &AppState,
    user_id: &str,
    role: &str,
    email: Option<&str>,
) -> Result<Json<Value>, Response> {
    let mut payload = issue_pair(state, user_id)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    payload["role"] = serde_json::json!(role);
    if let Some(e) = email {
        payload["email"] = serde_json::json!(e);
    }
    let _ = state
        .mongo
        .collection::<BsonDoc>("users")
        .update_one(
            doc! { "_id": user_id },
            doc! { "$set": { "last_login_at": Utc::now().to_rfc3339() } },
        )
        .await;
    Ok(Json(payload))
}

/// 外部认证（LDAP/OIDC）账号落地：oidc_sub 精确匹配 → email 匹配（命中即绑定
/// 外部身份，自托管场景 IdP 由管理员掌控）→ 未命中且 allow_register 时 JIT 建号。
/// 返回 (user_id, role)。
pub async fn external_upsert(
    state: &AppState,
    source: &str,
    email: &str,
    name: &str,
    oidc_sub: Option<&str>,
    allow_register: bool,
) -> Result<(String, String), String> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return Err("外部账号缺少有效邮箱".into());
    }
    let users = state.mongo.collection::<BsonDoc>("users");

    // OIDC：sub 唯一标识优先，防 IdP 侧改邮箱导致串号
    if let Some(sub) = oidc_sub {
        if let Some(u) = users
            .find_one(doc! { "oidc_sub": sub })
            .await
            .map_err(|e| e.to_string())?
        {
            return touch_external(state, u, name).await;
        }
    }
    if let Some(u) = users
        .find_one(doc! { "email": &email })
        .await
        .map_err(|e| e.to_string())?
    {
        if let Some(sub) = oidc_sub {
            let _ = users
                .update_one(
                    doc! { "_id": u.get_str("_id").unwrap_or_default() },
                    doc! { "$set": { "oidc_sub": sub } },
                )
                .await;
        }
        return touch_external(state, u, name).await;
    }
    if !allow_register {
        return Err("该账号尚未注册，且管理员未开启自动注册".into());
    }
    let user_id = Uuid::now_v7().to_string();
    let mut d = doc! {
        "_id": &user_id,
        "email": &email,
        "role": "user",
        "auth_source": source,
        "created_at": Utc::now().to_rfc3339(),
        "last_login_at": Utc::now().to_rfc3339(),
    };
    if !name.is_empty() {
        d.insert("display_name", name);
    }
    if let Some(sub) = oidc_sub {
        d.insert("oidc_sub", sub);
    }
    match users.insert_one(d).await {
        Ok(_) => {}
        Err(e) if e.to_string().contains("duplicate") => {
            // 并发首登竞态：按 email 重查归并
            let u = users
                .find_one(doc! { "email": &email })
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "账号创建冲突，请重试".to_string())?;
            return touch_external(state, u, name).await;
        }
        Err(e) => return Err(e.to_string()),
    }
    tracing::info!(email = %email, source, "external user auto-registered");
    Ok((user_id, "user".into()))
}

/// 命中已有账号的收尾：禁用即拒；补姓名与 last_login_at
async fn touch_external(
    state: &AppState,
    u: BsonDoc,
    name: &str,
) -> Result<(String, String), String> {
    if u.get_bool("disabled").unwrap_or(false) {
        return Err("账号已被禁用".into());
    }
    let user_id = u.get_str("_id").unwrap_or_default().to_string();
    let role = u.get_str("role").unwrap_or("user").to_string();
    let mut set = doc! { "last_login_at": Utc::now().to_rfc3339() };
    if !name.is_empty() && u.get_str("display_name").unwrap_or_default().is_empty() {
        set.insert("display_name", name);
    }
    let _ = state
        .mongo
        .collection::<BsonDoc>("users")
        .update_one(doc! { "_id": &user_id }, doc! { "$set": set })
        .await;
    Ok((user_id, role))
}

/// WS 网关建连前的状态检查：用户存在且未禁用（查不到/查询失败一律拒绝）
pub async fn user_active(state: &AppState, user_id: &str) -> bool {
    let found = state
        .mongo
        .collection::<BsonDoc>("users")
        .find_one(doc! { "_id": user_id })
        .await;
    match found {
        Ok(Some(u)) => !u.get_bool("disabled").unwrap_or(false),
        _ => false,
    }
}

async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshBody>,
) -> Result<Json<Value>, Response> {
    let claims = verify_token(&state, &body.refresh_token, "refresh")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?;
    // 禁用/删除的用户即使 refresh token 还在 Redis 白名单里也拒绝续期
    let user = state
        .mongo
        .collection::<BsonDoc>("users")
        .find_one(doc! { "_id": &claims.sub })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "user not found"))?;
    if user.get_bool("disabled").unwrap_or(false) {
        return Err(api_err(StatusCode::FORBIDDEN, "账号已被禁用"));
    }
    // rotate: revoke old refresh, issue new pair
    let mut conn = state.redis.clone();
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(format!("auth:refresh:{}", claims.jti))
        .query_async(&mut conn)
        .await;
    issue_pair(&state, &claims.sub)
        .await
        .map(Json)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub(crate) async fn issue_pair(state: &AppState, user_id: &str) -> Result<Value, String> {
    let (access, _, _) = make_token(state, user_id, "access")?;
    let (refresh, refresh_jti, refresh_exp) = make_token(state, user_id, "refresh")?;
    let ttl = (refresh_exp - Utc::now().timestamp()).max(0) as u64;
    let mut conn = state.redis.clone();
    redis::cmd("SETEX")
        .arg(format!("auth:refresh:{refresh_jti}"))
        .arg(ttl)
        .arg(user_id)
        .query_async::<()>(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"access_token": access, "refresh_token": refresh, "user_id": user_id}))
}

/// random device label for audits
pub fn random_suffix() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect()
}

// ------------------------------------------------------------ invites

/// 邀请码字母表：去掉易混淆的 0/O/1/I/L
const INVITE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// 生成 XXXXX-XXXXX 形式的邀请码（31^10 组合空间，可读可抄写）
pub fn gen_invite_code() -> String {
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let s: String = (0..10)
        .map(|_| *INVITE_ALPHABET.choose(&mut rng).unwrap() as char)
        .collect();
    format!("{}-{}", &s[..5], &s[5..])
}

/// 邀请码输入归一化：去首尾空白 + 大写（用户抄写时可能小写）
pub fn norm_code(s: &str) -> String {
    s.trim().to_uppercase()
}

/// 原子认领邀请码：过滤条件（未撤销 / 未过期 / used_count < max_uses）与
/// 自增在同一次 update 里，Mongo 单文档原子性保证并发注册不会超发
async fn claim_invite(state: &AppState, code: &str, user_id: &str) -> Result<(), String> {
    use mongodb::bson::Bson;
    let now = Utc::now().to_rfc3339();
    let filter = doc! {
        "_id": code,
        "revoked": doc! { "$ne": true },
        "$expr": doc! { "$lt": ["$used_count", "$max_uses"] },
        "$or": [
            { "expires_at": { "$in": [Bson::Null, ""] } },
            { "expires_at": { "$gt": now } },
        ],
    };
    let r = state
        .mongo
        .collection::<BsonDoc>("invites")
        .update_one(
            filter,
            doc! { "$inc": { "used_count": 1 }, "$push": { "used_by": user_id } },
        )
        .await
        .map_err(|e| e.to_string())?;
    if r.matched_count == 0 {
        return Err("邀请码无效、已撤销或已用尽".into());
    }
    Ok(())
}

/// 校验调用者是管理员（access token 有效且 users.role == "admin"），
/// 返回 user_id；权限每请求查库而不是写进 JWT，改角色立即生效
pub async fn verify_admin(state: &AppState, token: &str) -> Result<String, (StatusCode, String)> {
    let claims = verify_token(state, token, "access")
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;
    let user = state
        .mongo
        .collection::<BsonDoc>("users")
        .find_one(doc! { "_id": &claims.sub })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "user not found".into()))?;
    if user.get_bool("disabled").unwrap_or(false) {
        return Err((StatusCode::FORBIDDEN, "账号已被禁用".into()));
    }
    if user.get_str("role").unwrap_or("user") != "admin" {
        return Err((StatusCode::FORBIDDEN, "需要管理员权限".into()));
    }
    Ok(claims.sub)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/login/ldap", post(login_ldap))
        .route("/auth/refresh", post(refresh))
}

// ------------------------------------------------------------ admin: 邀请码管理

#[derive(Deserialize)]
struct AdminTokenQuery {
    token: String,
}

#[derive(Deserialize)]
struct InviteCreateBody {
    token: String,
    /// 可用次数，缺省 1（一次性）
    #[serde(default)]
    max_uses: i64,
    /// 有效天数，0/缺省 = 永不过期
    #[serde(default)]
    expires_days: i64,
}

async fn invites_create(
    State(state): State<AppState>,
    Json(body): Json<InviteCreateBody>,
) -> Result<Json<Value>, Response> {
    let admin_id = verify_admin(&state, &body.token)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let max_uses = body.max_uses.clamp(1, 1000);
    let expires_at = (body.expires_days > 0)
        .then(|| (Utc::now() + Duration::days(body.expires_days.min(3650))).to_rfc3339());
    let invites = state.mongo.collection::<BsonDoc>("invites");
    // 31^10 随机撞码几乎不可能，重试几次兜底
    for _ in 0..3 {
        let code = gen_invite_code();
        let mut doc = doc! {
            "_id": &code,
            "created_by": &admin_id,
            "created_at": Utc::now().to_rfc3339(),
            "max_uses": max_uses,
            "used_count": 0,
            "revoked": false,
        };
        if let Some(exp) = &expires_at {
            doc.insert("expires_at", exp);
        }
        match invites.insert_one(doc).await {
            Ok(_) => {
                tracing::info!(code, max_uses, "invite created by admin {admin_id}");
                return Ok(Json(serde_json::json!({
                    "code": code,
                    "max_uses": max_uses,
                    "expires_at": expires_at.unwrap_or_default(),
                })));
            }
            Err(e) if e.to_string().contains("duplicate") => continue,
            Err(e) => return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        }
    }
    Err(api_err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "邀请码生成冲突，请重试",
    ))
}

async fn invites_list(
    State(state): State<AppState>,
    Query(q): Query<AdminTokenQuery>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &q.token)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let docs = state
        .mongo
        .collection::<BsonDoc>("invites")
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
            serde_json::json!({
                "code": d.get_str("_id").unwrap_or_default(),
                "created_by": d.get_str("created_by").unwrap_or_default(),
                "created_at": d.get_str("created_at").unwrap_or_default(),
                "max_uses": d.get_i64("max_uses").unwrap_or(1),
                "used_count": d.get_i64("used_count").unwrap_or(0),
                "expires_at": d.get_str("expires_at").unwrap_or_default(),
                "revoked": d.get_bool("revoked").unwrap_or(false),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "invites": out })))
}

async fn invites_revoke(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<AdminTokenQuery>,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &q.token)
        .await
        .map_err(|(s, e)| api_err(s, e))?;
    let code = norm_code(&code);
    let r = state
        .mongo
        .collection::<BsonDoc>("invites")
        .update_one(doc! { "_id": &code }, doc! { "$set": { "revoked": true } })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if r.matched_count == 0 {
        return Err(api_err(StatusCode::NOT_FOUND, "邀请码不存在"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 管理端路由：建/列/撤邀请码，全部要求 role=admin
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/admin/invites", post(invites_create).get(invites_list))
        .route("/admin/invites/{code}", delete(invites_revoke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_code_shape() {
        for _ in 0..100 {
            let c = gen_invite_code();
            let (a, b) = c.split_once('-').expect("应含连字符");
            assert_eq!(a.len(), 5);
            assert_eq!(b.len(), 5);
            assert!(
                c.chars()
                    .all(|ch| ch == '-' || INVITE_ALPHABET.contains(&(ch as u8))),
                "只允许无歧义字母表: {c}"
            );
        }
    }

    #[test]
    fn invite_codes_dont_collide() {
        let set: std::collections::HashSet<String> = (0..500).map(|_| gen_invite_code()).collect();
        assert_eq!(set.len(), 500);
    }

    #[test]
    fn norm_code_trims_and_uppercases() {
        assert_eq!(norm_code(" ab-cd "), "AB-CD");
        assert_eq!(norm_code("ab-cd"), "AB-CD");
    }
}
