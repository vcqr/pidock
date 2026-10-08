//! Personal access tokens：给桌面端同步等场景的长期令牌。
//!
//! 格式 `pd_<jti>.<secret>`（不透明串）。校验路径独立于 JWT：
//! verify_token 对 `pd_` 前缀走本模块——Redis `auth:pat:{jti}` 白名单决定
//! 有效性（吊销即时生效，与 refresh token 同一信任模型），Mongo
//! `access_tokens` 只存元数据（名称/哈希/创建时间）供列表展示。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{delete, get},
    Json, Router,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document as BsonDoc};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{api_err, verify_token, Claims};
use crate::state::AppState;

/// 令牌前缀：verify_token 据此分流到 PAT 校验路径
pub const PAT_PREFIX: &str = "pd_";
/// 令牌有效期（10 年）；Redis 白名单 TTL 与之对齐
const PAT_TTL_DAYS: i64 = 365 * 10;
/// 每个用户最多持有的令牌数，防滥用
const MAX_TOKENS_PER_USER: usize = 20;

fn hex_char(rng: &mut impl Rng) -> char {
    b"0123456789abcdef"[rng.gen_range(0..16)] as char
}fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn pat_key(jti: &str) -> String {
    format!("auth:pat:{jti}")
}

/// 生成新令牌：明文只在此处出现一次，库里只落 sha256
pub async fn create(state: &AppState, user_id: &str, name: &str) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请给令牌起个名字（如「办公室台式机」）".into());
    }
    if name.chars().count() > 64 {
        return Err("令牌名称过长（最多 64 字符）".into());
    }
    let tokens = state.mongo.collection::<BsonDoc>("access_tokens");
    let existing = tokens
        .count_documents(doc! { "user_id": user_id })
        .await
        .map_err(|e| e.to_string())?;
    if existing >= MAX_TOKENS_PER_USER as u64 {
        return Err(format!("最多创建 {MAX_TOKENS_PER_USER} 个访问令牌，请先删除不用的"));
    }

    let jti = Uuid::now_v7().simple().to_string();
    // rand::random 单次取样即丢弃：ThreadRng 带 Drop 不能活过下面的 await
    let secret_bytes: [u8; 24] = rand::random();
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token = format!("{PAT_PREFIX}{jti}.{secret}");
    let created_at = chrono::Utc::now().to_rfc3339();
    tokens
        .insert_one(doc! {
            "_id": &jti,
            "user_id": user_id,
            "name": name,
            "token_hash": sha256_hex(&token),
            "created_at": &created_at,
        })
        .await
        .map_err(|e| e.to_string())?;
    let ttl = (PAT_TTL_DAYS * 24 * 3600) as u64;
    let mut conn = state.redis.clone();
    redis::cmd("SETEX")
        .arg(pat_key(&jti))
        .arg(ttl)
        .arg(user_id)
        .query_async::<()>(&mut conn)
        .await
        .map_err(|e| format!("令牌写入失败: {e}"))?;
    tracing::info!(user = %user_id, "access token created");
    Ok(json!({"id": jti, "token": token, "name": name, "created_at": created_at}))
}

/// PAT 校验（verify_token 的 `pd_` 分支）：Redis 白名单即真理，
/// 返回等价 access Claims，后续鉴权逻辑与普通登录完全一致
pub async fn verify(state: &AppState, token: &str) -> Result<Claims, String> {
    let rest = token
        .strip_prefix(PAT_PREFIX)
        .and_then(|r| r.split_once('.'))
        .ok_or("访问令牌格式不正确")?;
    let (jti, secret) = rest;
    // jti 必须是 32 位 hex（UUID simple），防怪键进 Redis
    if jti.len() != 32 || !jti.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("访问令牌格式不正确".into());
    }
    if secret.is_empty() {
        return Err("访问令牌格式不正确".into());
    }
    let mut conn = state.redis.clone();
    let user_id: Option<String> = redis::cmd("GET")
        .arg(pat_key(jti))
        .query_async(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    let user_id = user_id.ok_or("访问令牌已撤销或不存在")?;
    // PAT 长期有效，不能只靠 access 15min 自然过期兜底：每次使用实时校验账号状态
    if !crate::auth::user_active(state, &user_id).await {
        return Err("账号已被禁用".into());
    }
    Ok(Claims {
        sub: user_id,
        kind: "access".into(),
        jti: jti.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::days(PAT_TTL_DAYS)).timestamp(),
        pat: true,
    })
}

/// 吊销：删除本人名下的令牌（Redis 白名单 + Mongo 元数据）
async fn revoke(state: &AppState, user_id: &str, id: &str) -> Result<(), String> {
    let r = state
        .mongo
        .collection::<BsonDoc>("access_tokens")
        .delete_one(doc! { "_id": id, "user_id": user_id })
        .await
        .map_err(|e| e.to_string())?;
    if r.deleted_count == 0 {
        return Err("令牌不存在".into());
    }
    let mut conn = state.redis.clone();
    let _: Result<(), _> = redis::cmd("DEL").arg(pat_key(id)).query_async(&mut conn).await;
    tracing::info!(user = %user_id, token = %id, "access token revoked");
    Ok(())
}

// ------------------------------------------------------------ endpoints

#[derive(Deserialize)]
struct TokenQuery {
    token: String,
}

#[derive(Deserialize)]
struct TokenCreateBody {
    token: String,
    #[serde(default)]
    name: String,
}

async fn tokens_list(
    State(state): State<AppState>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &q.token, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    let docs = state
        .mongo
        .collection::<BsonDoc>("access_tokens")
        .find(doc! { "user_id": &user_id })
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! { "created_at": -1 })
                .build(),
        )
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    let docs = docs.try_collect::<Vec<BsonDoc>>().await;
    for d in docs.map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(json!({
            "id": d.get_str("_id").unwrap_or_default(),
            "name": d.get_str("name").unwrap_or_default(),
            "created_at": d.get_str("created_at").unwrap_or_default(),
        }));
    }
    Ok(Json(json!({ "tokens": out })))
}

async fn tokens_create(
    State(state): State<AppState>,
    Json(body): Json<TokenCreateBody>,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &body.token, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    create(&state, &user_id, &body.name)
        .await
        .map(Json)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))
}


async fn tokens_revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &q.token, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    revoke(&state, &user_id, &id)
        .await
        .map(|_| Json(json!({ "ok": true })))
        .map_err(|e| api_err(StatusCode::NOT_FOUND, e))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/tokens", get(tokens_list).post(tokens_create))
        .route("/me/tokens/{id}", delete(tokens_revoke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape() {
        let mut rng = rand::thread_rng();
        let jti = Uuid::now_v7().simple().to_string();
        let secret: String = (0..48).map(|_| hex_char(&mut rng)).collect();
        let token = format!("{PAT_PREFIX}{jti}.{secret}");
        assert!(token.starts_with("pd_"));
        assert_eq!(token.len(), 3 + 32 + 1 + 48);
        let (jti2, secret2) = token["pd_".len()..].split_once('.').unwrap();
        assert_eq!(jti2.len(), 32);
        assert!(jti2.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(secret2.len(), 48);
    }

    #[test]
    fn hash_is_deterministic_hex() {
        let h1 = sha256_hex("pd_abc.def");
        let h2 = sha256_hex("pd_abc.def");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert_ne!(h1, sha256_hex("pd_abc.deG"));
    }
}
