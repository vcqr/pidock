//! Auth: register / login / refresh with argon2 + JWT (access 15min,
//! refresh 30d stored in Redis and revocable per-device).

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts, Path, Query, State},
    http::{header, HeaderMap, StatusCode, request::Parts},
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
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::state::AppState;

const ACCESS_TTL_MIN: i64 = 15;
const REFRESH_TTL_DAYS: i64 = 30;

pub(crate) fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": msg.into()}))).into_response()
}

/// Bearer token 提取：`Authorization: Bearer <token>` 优先，回落 `?token=`
/// （旧客户端兼容，逐步淘汰——query token 会落反代访问日志）。
/// WS 用 /auth/ws-ticket 签发的一次性 ticket，不传长期 token。
pub struct Bearer(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Bearer {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(v) = parts.headers.get(header::AUTHORIZATION) {
            if let Ok(s) = v.to_str() {
                if let Some(t) = s.strip_prefix("Bearer ").map(str::trim).filter(|t| !t.is_empty()) {
                    return Ok(Bearer(t.to_string()));
                }
            }
        }
        if let Ok(q) =
            Query::<std::collections::HashMap<String, String>>::from_request_parts(parts, state)
                .await
        {
            if let Some(t) = q.get("token").filter(|t| !t.is_empty()) {
                return Ok(Bearer(t.clone()));
            }
        }
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "缺少访问令牌"})),
        ))
    }
}

/// 可缺省版 Bearer：header/query 都没有时给 None 而不是 401——
/// 供 /commands 兼容「token 放 body」的旧客户端自行回落
pub struct BearerOpt(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for BearerOpt {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(BearerOpt(
            Bearer::from_request_parts(parts, state).await.ok().map(|b| b.0),
        ))
    }
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
        ensure_refresh_alive(state, &data.claims.jti, data.claims.exp).await?;
    }
    Ok(data.claims)
}

pub(crate) fn hash_password(password: &str) -> Result<String, String> {
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

/// 未知邮箱/无本地密码的账号也走一次等价 argon2 校验，抹平响应时间差（防枚举）
fn burn_password_check(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let hash = DUMMY.get_or_init(|| {
        hash_password("pidock-dummy-password-for-timing").unwrap_or_default()
    });
    if !hash.is_empty() {
        verify_password(password, hash);
    }
}

#[derive(Deserialize)]
pub struct RegisterBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub invite_code: String,
    /// Turnstile 人机验证 token（启用时必填，见 turnstile::verify）
    #[serde(default)]
    pub turnstile_token: String,
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub turnstile_token: String,
}

#[derive(Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Result<Json<Value>, Response> {
    let ip = crate::guard::client_ip(&state.cfg, &headers, peer);
    crate::turnstile::verify(
        &state.cfg.turnstile_site_key,
        &state.cfg.turnstile_secret_key,
        &body.turnstile_token,
        &ip,
    )
        .await
        .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
    // 邮箱验证码开关：注册拆两段，先验邮箱所有权再建号
    if crate::mailer::register_code_enabled(&state.cfg.email) {
        return register_with_code(&state, &ip, body).await;
    }
    // 同 IP 注册尝试限流（成功失败都计入）
    crate::guard::register_allowed(&state, &ip).await?;
    let (email_lc, hash, code, is_bootstrap) = validate_registration(&state, &body, &ip).await?;
    let (user_id, role) = create_user(&state, &email_lc, &hash, &code, is_bootstrap, &ip).await?;
    Ok(Json(serde_json::json!({"user_id": user_id, "role": role})))
}

/// 邮箱格式：无空白、恰一个 @、域名至少含一个点且不以点开头/结尾、无连续点。
/// 注册与管理员建号共用；登录不校验（老账号邮箱可能不满足新规则）。
pub(crate) fn valid_email(email: &str) -> bool {
    let e = email.trim();
    if e.is_empty() || e.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = e.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

/// 密码策略：8~128 位，同时包含字母和数字（不强制大小写/特殊符号）
pub(crate) fn valid_password(password: &str) -> bool {
    (8..=128).contains(&password.chars().count())
        && password.chars().any(|c| c.is_alphabetic())
        && password.chars().any(|c| c.is_ascii_digit())
}

/// 注册前置校验（一段式与发码两段式共用）：邮箱/密码格式、邀请码非空、注册开关。
/// 返回 (小写邮箱, 密码哈希, 规范化邀请码, 是否引导码)
async fn validate_registration(
    state: &AppState,
    body: &RegisterBody,
    ip: &str,
) -> Result<(String, String, String, bool), Response> {
    if !valid_email(&body.email) {
        return Err(api_err(StatusCode::BAD_REQUEST, "邮箱格式不正确"));
    }
    if !valid_password(&body.password) {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "密码需 8~128 位，且同时包含字母和数字",
        ));
    }
    let code = norm_code(&body.invite_code);
    if code.is_empty() {
        crate::guard::register_failure(ip, "missing invite");
        return Err(api_err(StatusCode::FORBIDDEN, "注册需要邀请码"));
    }
    // 全局注册开关：关闭后仅 bootstrap 引导码（救急通道）与管理员后台建号可用
    let is_bootstrap =
        !state.cfg.bootstrap_invite.is_empty() && code == norm_code(&state.cfg.bootstrap_invite);
    if !crate::admin::load_policy(state).await.allow_register && !is_bootstrap {
        crate::guard::register_failure(ip, "registration closed");
        return Err(api_err(StatusCode::FORBIDDEN, "管理员已关闭注册，请联系管理员开通账号"));
    }
    let email_lc = body.email.trim().to_lowercase();
    let hash =
        hash_password(&body.password).map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok((email_lc, hash, code, is_bootstrap))
}

/// 注册两段式的第一段：校验 + 发码建挑战；账号与邀请码认领留到验证通过后
async fn register_with_code(
    state: &AppState,
    ip: &str,
    body: RegisterBody,
) -> Result<Json<Value>, Response> {
    // 发码在 IP 限流之后，防止绕过限流刷邮件
    crate::guard::register_allowed(state, ip).await?;
    let (email_lc, hash, code, is_bootstrap) = validate_registration(state, &body, ip).await?;
    if !is_bootstrap {
        precheck_invite(state, &code).await?;
    }
    let started = crate::otp::start(
        state,
        &email_lc,
        crate::otp::Pending::Register {
            password_hash: hash,
            invite_code: code,
            bootstrap: is_bootstrap,
        },
    )
    .await
    .map_err(|(s, m)| api_err(s, m))?;
    tracing::info!(email = %email_lc, "register code sent");
    Ok(Json(serde_json::json!({
        "pending": "register_code",
        "challenge": started.challenge,
        "email": started.masked_email,
    })))
}

/// 注册发码前的只读预检（正式认领在验证通过后原子进行）：挡掉无效码，不给注定失败的注册发邮件
async fn precheck_invite(state: &AppState, code: &str) -> Result<(), Response> {
    let now = Utc::now().to_rfc3339();
    let inv = state
        .mongo
        .collection::<BsonDoc>("invites")
        .find_one(doc! { "_id": code })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let invalid = || api_err(StatusCode::FORBIDDEN, "邀请码无效、已撤销或已用尽");
    let Some(inv) = inv else { return Err(invalid()) };
    if inv.get_bool("revoked").unwrap_or(false) {
        return Err(invalid());
    }
    let int_of = |key: &str| -> i64 {
        match inv.get(key) {
            Some(mongodb::bson::Bson::Int32(v)) => *v as i64,
            Some(mongodb::bson::Bson::Int64(v)) => *v,
            _ => 0,
        }
    };
    if int_of("used_count") >= int_of("max_uses") {
        return Err(invalid());
    }
    let expires_at = inv.get_str("expires_at").unwrap_or_default();
    if !expires_at.is_empty() && expires_at <= now.as_str() {
        return Err(invalid());
    }
    Ok(())
}

/// 建号收尾：引导码复核管理员数量 / 原子认领邀请码 / 落库（邮箱冲突退回认领的名额）
async fn create_user(
    state: &AppState,
    email_lc: &str,
    hash: &str,
    code: &str,
    bootstrap: bool,
    ip: &str,
) -> Result<(String, String), Response> {
    let users = state.mongo.collection::<BsonDoc>("users");
    let user_id = Uuid::now_v7().to_string();
    // 引导邀请码（配置下发）仅在没有管理员时有效，用完即废，不落 invites 集合
    let mut role = "user";
    if bootstrap {
        let admins = users
            .count_documents(doc! { "role": "admin" })
            .await
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if admins > 0 {
            return Err(api_err(StatusCode::FORBIDDEN, "引导邀请码已失效"));
        }
        role = "admin";
    } else {
        claim_invite(state, code, &user_id)
            .await
            .map_err(|e| {
                crate::guard::register_failure(ip, &format!("invite: {e}"));
                api_err(StatusCode::FORBIDDEN, e)
            })?;
    }
    let user: BsonDoc = doc! {
        "_id": &user_id,
        "email": email_lc,
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
                    doc! { "_id": code },
                    doc! { "$inc": { "used_count": -1 }, "$pull": { "used_by": &user_id } },
                )
                .await;
            crate::guard::register_failure(ip, "duplicate email");
            return Err(api_err(StatusCode::CONFLICT, "该邮箱已注册"));
        }
        return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    tracing::info!(user = %user_id, role, "user registered");
    Ok((user_id, role.to_string()))
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, Response> {
    let ip = crate::guard::client_ip(&state.cfg, &headers, peer);
    let email_lc = body.email.trim().to_lowercase();
    // 人机验证：启用时验不过直接拒（token 一次性，前端失败后需重新过验证）
    crate::turnstile::verify(
        &state.cfg.turnstile_site_key,
        &state.cfg.turnstile_secret_key,
        &body.turnstile_token,
        &ip,
    )
        .await
        .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
    // 防爆破：同账号失败 5 次 / 同 IP 失败 20 次，锁 15 分钟
    crate::guard::login_allowed(&state, &email_lc, &ip).await?;
    let users = state.mongo.collection::<BsonDoc>("users");
    let user = users
        .find_one(doc! {"email": &email_lc})
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(user) = user else {
        burn_password_check(&body.password);
        crate::guard::login_failure(&state, &email_lc, &ip).await;
        return Err(api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"));
    };
    if user.get_bool("disabled").unwrap_or(false) {
        burn_password_check(&body.password);
        return Err(api_err(StatusCode::FORBIDDEN, "账号已被禁用"));
    }
    let hash = user.get_str("password_hash").ok().unwrap_or_default();
    if hash.is_empty() {
        // 无本地密码的账号（LDAP/SSO 建）指路对应的登录方式
        burn_password_check(&body.password);
        let hint = match user.get_str("auth_source").unwrap_or("local") {
            "ldap" => "该账号为 LDAP 账号，请使用 LDAP 登录",
            "oidc" => "该账号为 SSO 账号，请使用 SSO 登录",
            _ => "该账号未设置本地密码",
        };
        return Err(api_err(StatusCode::UNAUTHORIZED, hint));
    }
    if !verify_password(&body.password, hash) {
        crate::guard::login_failure(&state, &email_lc, &ip).await;
        return Err(api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"));
    }
    crate::guard::login_clear(&state, &email_lc, &ip).await;
    let user_id = user.get_str("_id").unwrap_or_default().to_string();
    let role = user.get_str("role").unwrap_or("user").to_string();
    // 两步验证：密码过了先发码，凭验证码换 token（LDAP/SSO 不走此流程）
    if crate::mailer::login_code_enabled(&state.cfg.email) {
        let started = crate::otp::start(
            &state,
            &email_lc,
            crate::otp::Pending::Login {
                user_id: user_id.clone(),
            },
        )
        .await
        .map_err(|(s, m)| api_err(s, m))?;
        return Ok(Json(serde_json::json!({
            "pending": "email_code",
            "challenge": started.challenge,
            "email": started.masked_email,
        })));
    }
    finish_login(&state, &user_id, &role, None).await
}

#[derive(Deserialize)]
pub struct LoginCodeVerifyBody {
    pub challenge: String,
    pub code: String,
}

#[derive(Deserialize)]
pub struct CodeResendBody {
    pub challenge: String,
}

/// 登录两步验证第二步：challenge + 验证码换正式 token 对
async fn login_code_verify(
    State(state): State<AppState>,
    Json(body): Json<LoginCodeVerifyBody>,
) -> Result<Json<Value>, Response> {
    let v = crate::otp::verify(&state, &body.challenge, &body.code)
        .await
        .map_err(|(s, m)| api_err(s, m))?;
    let crate::otp::Pending::Login { user_id } = v.pending else {
        return Err(api_err(StatusCode::BAD_REQUEST, "该验证码不用于登录"));
    };
    let users = state.mongo.collection::<BsonDoc>("users");
    let Some(user) = users
        .find_one(doc! { "_id": &user_id })
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Err(api_err(StatusCode::UNAUTHORIZED, "账号不存在"));
    };
    if user.get_bool("disabled").unwrap_or(false) {
        return Err(api_err(StatusCode::FORBIDDEN, "账号已被禁用"));
    }
    let role = user.get_str("role").unwrap_or("user").to_string();
    let email = user.get_str("email").ok().map(|s| s.to_string());
    finish_login(&state, &user_id, &role, email.as_deref()).await
}

/// 注册两步验证第二步：验证码通过 → 原子认领邀请码并建号 → 直接登录发 token
async fn register_code_verify(
    State(state): State<AppState>,
    Json(body): Json<LoginCodeVerifyBody>,
) -> Result<Json<Value>, Response> {
    let v = crate::otp::verify(&state, &body.challenge, &body.code)
        .await
        .map_err(|(s, m)| api_err(s, m))?;
    let crate::otp::Pending::Register {
        password_hash,
        invite_code,
        bootstrap,
    } = v.pending
    else {
        return Err(api_err(StatusCode::BAD_REQUEST, "该验证码不用于注册"));
    };
    let (user_id, role) = create_user(&state, &v.email, &password_hash, &invite_code, bootstrap, "")
        .await?;
    finish_login(&state, &user_id, &role, Some(&v.email)).await
}

/// 重发验证码（登录/注册共用）：60s 冷却 + 每邮箱小时配额，由 otp 模块把关
async fn code_resend(
    State(state): State<AppState>,
    Json(body): Json<CodeResendBody>,
) -> Result<Json<Value>, Response> {
    let started = crate::otp::resend(&state, &body.challenge)
        .await
        .map_err(|(s, m)| api_err(s, m))?;
    Ok(Json(serde_json::json!({
        "resent": true,
        "challenge": started.challenge,
        "email": started.masked_email,
    })))
}

#[derive(Deserialize)]
pub struct LoginLdapBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub turnstile_token: String,
}

/// LDAP 独立登录：与标准登录彻底分开，报错语义各自纯粹
async fn login_ldap(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginLdapBody>,
) -> Result<Json<Value>, Response> {
    let ip = crate::guard::client_ip(&state.cfg, &headers, peer);
    let username = body.username.trim().to_string();
    crate::turnstile::verify(
        &state.cfg.turnstile_site_key,
        &state.cfg.turnstile_secret_key,
        &body.turnstile_token,
        &ip,
    )
        .await
        .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
    crate::guard::login_allowed(&state, &username, &ip).await?;
    let cfg = crate::admin::load_ldap_cfg(&state).await;
    if !cfg.enabled {
        return Err(api_err(StatusCode::NOT_FOUND, "LDAP 登录未启用"));
    }
    match crate::ldap::authenticate(&cfg, &username, &body.password).await {
        Ok(lu) => {
            let (user_id, role) =
                external_upsert(&state, "ldap", &lu.email, &lu.name, None, cfg.allow_register)
                    .await
                    .map_err(|e| api_err(StatusCode::FORBIDDEN, e))?;
            crate::guard::login_clear(&state, &username, &ip).await;
            finish_login(&state, &user_id, &role, Some(&lu.email)).await
        }
        Err(crate::ldap::LdapAuthError::Credentials) => {
            crate::guard::login_failure(&state, &username, &ip).await;
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
    // rotate: revoke old refresh（Mongo 为准 + 缓存失效），签发新对
    revoke_refresh(&state, &claims.jti).await;
    issue_pair(&state, &claims.sub)
        .await
        .map(Json)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

/// 吊销 refresh 会话：删 Mongo 白名单文档 + 失效 Redis 缓存（rotate/logout 共用）
pub(crate) async fn revoke_refresh(state: &AppState, jti: &str) {
    let mut conn = state.redis.clone();
    let _: Result<(), _> = conn.del(format!("auth:refresh:{jti}")).await;
    let _ = state
        .mongo
        .collection::<BsonDoc>("refresh_tokens")
        .delete_one(doc! { "_id": jti })
        .await;
}

#[derive(Deserialize)]
pub struct LogoutBody {
    pub refresh_token: String,
}

/// 登出：吊销当前会话的 refresh token（幂等；access token 15 分钟自亡）。
/// 无需 access 鉴权——持有有效 refresh 本身即会话凭证
async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutBody>,
) -> Result<Json<Value>, Response> {
    let claims = verify_token(&state, &body.refresh_token, "refresh")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?;
    revoke_refresh(&state, &claims.jti).await;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// WS 一次性票据：短命 ticket 换掉长期 token 出现在 URL / 反代访问日志里。
/// ticket 60 秒有效、单次使用（GETDEL），连接时仍复核账号状态
async fn ws_ticket(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let claims = verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?;
    if !user_active(&state, &claims.sub).await {
        return Err(api_err(StatusCode::FORBIDDEN, "账号已被禁用"));
    }
    let ticket = Uuid::now_v7().simple().to_string();
    let mut conn = state.redis.clone();
    let _: Result<(), _> = redis::cmd("SETEX")
        .arg(format!("ws:ticket:{ticket}"))
        .arg(60u64)
        .arg(&claims.sub)
        .query_async::<()>(&mut conn)
        .await;
    Ok(Json(serde_json::json!({"ticket": ticket})))
}

pub(crate) async fn issue_pair(state: &AppState, user_id: &str) -> Result<Value, String> {
    let (access, _, _) = make_token(state, user_id, "access")?;
    let (refresh, refresh_jti, refresh_exp) = make_token(state, user_id, "refresh")?;
    // refresh 白名单以 Mongo 为准（Redis 重启不丢会话），Redis 只作读缓存
    let expires_at = (Utc::now() + Duration::days(REFRESH_TTL_DAYS)).to_rfc3339();
    state
        .mongo
        .collection::<BsonDoc>("refresh_tokens")
        .insert_one(doc! {
            "_id": &refresh_jti,
            "user_id": user_id,
            "expires_at": &expires_at,
            "created_at": Utc::now().to_rfc3339(),
        })
        .await
        .map_err(|e| e.to_string())?;
    let ttl = (refresh_exp - Utc::now().timestamp()).max(0) as u64;
    let mut conn = state.redis.clone();
    // 缓存写失败不影响发牌（Mongo 已是事实源）
    let _: Result<(), _> = redis::cmd("SETEX")
        .arg(format!("auth:refresh:{refresh_jti}"))
        .arg(ttl)
        .arg(1u8)
        .query_async::<()>(&mut conn)
        .await;
    Ok(serde_json::json!({"access_token": access, "refresh_token": refresh, "user_id": user_id}))
}

/// refresh 白名单校验：缓存命中即过，未命中回源 Mongo（在册且未过期）并回填。
/// Redis 数据丢失只是缓存失效，不影响会话有效性
async fn ensure_refresh_alive(state: &AppState, jti: &str, exp: i64) -> Result<(), String> {
    let key = format!("auth:refresh:{jti}");
    let mut conn = state.redis.clone();
    let cached: Option<String> = conn.get(&key).await.unwrap_or(None);
    if cached.is_some() {
        return Ok(());
    }
    let tokens = state.mongo.collection::<BsonDoc>("refresh_tokens");
    let doc = tokens
        .find_one(doc! { "_id": jti })
        .await
        .map_err(|e| e.to_string())?;
    let Some(d) = doc else {
        return Err("refresh token revoked or expired".into());
    };
    let expires_at = d.get_str("expires_at").unwrap_or_default();
    let expired = !expires_at.is_empty()
        && chrono::DateTime::parse_from_rfc3339(expires_at)
            .map(|t| t < Utc::now())
            .unwrap_or(false);
    if expired {
        let _ = tokens.delete_one(doc! { "_id": jti }).await;
        return Err("refresh token revoked or expired".into());
    }
    // 回填缓存：TTL 取 JWT 剩余寿命（不短于 1 分钟）。缓存写失败不影响请求
    let ttl = (exp - Utc::now().timestamp()).clamp(60, REFRESH_TTL_DAYS * 86400) as u64;
    let _: Result<(), _> = redis::cmd("SETEX")
        .arg(&key)
        .arg(ttl)
        .arg(1u8)
        .query_async::<()>(&mut conn)
        .await;
    Ok(())
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
        .route("/auth/register/code/verify", post(register_code_verify))
        .route("/auth/login", post(login))
        .route("/auth/login/code/verify", post(login_code_verify))
        .route("/auth/code/resend", post(code_resend))
        .route("/auth/login/ldap", post(login_ldap))
        .route("/auth/logout", post(logout))
        .route("/auth/ws-ticket", post(ws_ticket))
        .route("/auth/refresh", post(refresh))
}

// ------------------------------------------------------------ admin: 邀请码管理

#[derive(Deserialize)]
struct InviteCreateBody {
    /// 可用次数，缺省 1（一次性）
    #[serde(default)]
    max_uses: i64,
    /// 有效天数，0/缺省 = 永不过期
    #[serde(default)]
    expires_days: i64,
}

async fn invites_create(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<InviteCreateBody>,
) -> Result<Json<Value>, Response> {
    let admin_id = verify_admin(&state, &tok.0)
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
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
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
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    verify_admin(&state, &tok.0)
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
