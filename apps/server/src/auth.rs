//! Auth: register / login / refresh with argon2 + JWT (access 15min,
//! refresh 30d stored in Redis and revocable per-device).

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{Duration, Utc};
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

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": msg.into()}))).into_response()
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,  // user id
    pub kind: String, // "access" | "refresh"
    pub jti: String,
    pub exp: i64,
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
    let hash =
        hash_password(&body.password).map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let user_id = Uuid::now_v7().to_string();
    let user: BsonDoc = doc! {
        "_id": &user_id,
        "email": body.email.to_lowercase(),
        "password_hash": &hash,
        "created_at": Utc::now().to_rfc3339(),
    };
    state
        .mongo
        .collection::<BsonDoc>("users")
        .insert_one(user)
        .await
        .map_err(|e| {
            if e.to_string().contains("duplicate") {
                api_err(StatusCode::CONFLICT, "该邮箱已注册")
            } else {
                api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    Ok(Json(serde_json::json!({"user_id": user_id})))
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, Response> {
    let users = state.mongo.collection::<BsonDoc>("users");
    let user = users
        .find_one(doc! {"email": body.email.to_lowercase()})
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"))?;
    let hash = user
        .get_str("password_hash")
        .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "corrupt user"))?;
    if !verify_password(&body.password, hash) {
        return Err(api_err(StatusCode::UNAUTHORIZED, "邮箱或密码错误"));
    }
    let user_id = user.get_str("_id").unwrap_or_default().to_string();
    issue_pair(&state, &user_id)
        .await
        .map(Json)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshBody>,
) -> Result<Json<Value>, Response> {
    let claims = verify_token(&state, &body.refresh_token, "refresh")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?;
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

async fn issue_pair(state: &AppState, user_id: &str) -> Result<Value, String> {
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
}
