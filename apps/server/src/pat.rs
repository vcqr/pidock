//! Personal access tokens：给桌面端同步等场景的长期令牌。
//!
//! 格式 `pd_<jti>.<secret>`（不透明串）。事实源是 Mongo `access_tokens`
//! （token_hash + expires_at），Redis `auth:pat:{jti}` 只作读缓存（存
//! {user_id, hash}，TTL 为剩余寿命）——Redis 重启/丢数据不影响令牌有效性。
//! 吊销 = 删 Mongo 文档 + 失效缓存，即时生效；每次使用实时校验账号状态。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::{delete, get},
    Json, Router,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document as BsonDoc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{api_err, verify_token, Bearer, Claims};
use crate::state::AppState;

/// 令牌前缀：verify_token 据此分流到 PAT 校验路径
pub const PAT_PREFIX: &str = "pd_";
/// 有效期选项（天）；0 = 不过期。管理端与前端共用这份白名单
pub const EXPIRY_CHOICES: &[i64] = &[0, 3, 15, 30, 90, 180, 360];
/// 「不过期」的实际 TTL：100 年，等同不过期且不占 Redis 永久键
const NEVER_TTL_DAYS: i64 = 365 * 100;
/// 每个用户最多持有的令牌数，防滥用
const MAX_TOKENS_PER_USER: usize = 20;

fn ttl_seconds(days: i64) -> u64 {
    let d = if days == 0 { NEVER_TTL_DAYS } else { days };
    (d * 24 * 3600) as u64
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn pat_key(jti: &str) -> String {
    format!("auth:pat:{jti}")
}

/// 回填读缓存：{user_id, hash}，TTL 为剩余寿命（不过期键取上限）
async fn fill_cache(state: &AppState, jti: &str, user_id: &str, hash: &str, expires_at: &str) {
    let ttl = if expires_at.is_empty() {
        ttl_seconds(0)
    } else {
        chrono::DateTime::parse_from_rfc3339(expires_at)
            .map(|t| (t.timestamp() - chrono::Utc::now().timestamp()).clamp(60, (NEVER_TTL_DAYS * 86400) as i64) as u64)
            .unwrap_or_else(|_| ttl_seconds(0))
    };
    let payload = json!({ "u": user_id, "h": hash });
    let mut conn = state.redis.clone();
    let _: Result<(), _> = redis::cmd("SETEX")
        .arg(pat_key(jti))
        .arg(ttl)
        .arg(payload.to_string())
        .query_async(&mut conn)
        .await;
}

/// 生成新令牌：明文只在此处出现一次，库里只落 sha256。
/// days 取 EXPIRY_CHOICES 之一（0 = 不过期）
pub async fn create(state: &AppState, user_id: &str, name: &str, days: i64) -> Result<Value, String> {
    if !EXPIRY_CHOICES.contains(&days) {
        return Err(format!(
            "无效的有效期，可选：{}",
            EXPIRY_CHOICES
                .iter()
                .map(|d| if *d == 0 { "不过期".to_string() } else { format!("{d}天") })
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
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
    let hash = sha256_hex(&token);
    let created_at = chrono::Utc::now().to_rfc3339();
    // 展示用到期时间（0 = 永不）；实际强制过期靠 Redis TTL
    let expires_at = if days > 0 {
        (chrono::Utc::now() + chrono::Duration::days(days)).to_rfc3339()
    } else {
        String::new()
    };
    tokens
        .insert_one(doc! {
            "_id": &jti,
            "user_id": user_id,
            "name": name,
            "token_hash": &hash,
            "days": days,
            "expires_at": &expires_at,
            "created_at": &created_at,
        })
        .await
        .map_err(|e| e.to_string())?;
    // 预热读缓存（失败无碍：校验时会回源 Mongo）
    fill_cache(state, &jti, user_id, &hash, &expires_at).await;
    tracing::info!(user = %user_id, days, "access token created");
    Ok(json!({
        "id": jti, "token": token, "name": name,
        "days": days, "expires_at": expires_at, "created_at": created_at,
    }))
}

/// PAT 校验（verify_token 的 `pd_` 分支）：缓存命中验哈希，未命中回源
/// Mongo（token_hash + expires_at 为事实源）并回填；返回等价 access Claims
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
    let hash = sha256_hex(token);
    let key = pat_key(jti);
    let mut conn = state.redis.clone();

    // 1) 缓存命中：值存 {u, h}；哈希不匹配视为脏数据回源
    let cached: Option<String> = conn.get(&key).await.unwrap_or(None);
    let mut user_id: Option<String> = None;
    if let Some(raw) = cached {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if v.get("h").and_then(|x| x.as_str()) == Some(hash.as_str()) {
                user_id = v.get("u").and_then(|x| x.as_str()).map(str::to_string);
            }
        }
    }
    // 2) 回源 Mongo：token_hash 与 expires_at 为事实源
    let user_id = match user_id {
        Some(u) => u,
        None => {
            let d = state
                .mongo
                .collection::<BsonDoc>("access_tokens")
                .find_one(doc! { "_id": jti })
                .await
                .map_err(|e| e.to_string())?;
            let Some(d) = d else {
                return Err("访问令牌已撤销或已过期".into());
            };
            if d.get_str("token_hash").unwrap_or_default() != hash {
                return Err("访问令牌已撤销或已过期".into());
            }
            let expires_at = d.get_str("expires_at").unwrap_or_default();
            let expired = !expires_at.is_empty()
                && chrono::DateTime::parse_from_rfc3339(expires_at)
                    .map(|t| t < chrono::Utc::now())
                    .unwrap_or(false);
            if expired {
                let _: Result<(), _> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
                return Err("访问令牌已撤销或已过期".into());
            }
            let u = d.get_str("user_id").unwrap_or_default().to_string();
            if u.is_empty() {
                return Err("访问令牌已撤销或已过期".into());
            }
            fill_cache(state, jti, &u, &hash, expires_at).await;
            u
        }
    };
    // PAT 长期有效，不能只靠 access 15min 自然过期兜底：每次使用实时校验账号状态
    if !crate::auth::user_active(state, &user_id).await {
        return Err("账号已被禁用".into());
    }
    Ok(Claims {
        sub: user_id,
        kind: "access".into(),
        jti: jti.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::days(NEVER_TTL_DAYS)).timestamp(),
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
struct TokenCreateBody {
    #[serde(default)]
    name: String,
    /// 有效期天数（EXPIRY_CHOICES 之一，0 = 不过期）
    #[serde(default)]
    days: i64,
}

async fn tokens_list(
    State(state): State<AppState>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &tok.0, "access")
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
            "days": d.get_i64("days").unwrap_or(0),
            "expires_at": d.get_str("expires_at").unwrap_or_default(),
            "created_at": d.get_str("created_at").unwrap_or_default(),
        }));
    }
    Ok(Json(json!({ "tokens": out })))
}

async fn tokens_create(
    State(state): State<AppState>,
    tok: Bearer,
    Json(body): Json<TokenCreateBody>,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &tok.0, "access")
        .await
        .map_err(|e| api_err(StatusCode::UNAUTHORIZED, e))?
        .sub;
    create(&state, &user_id, &body.name, body.days)
        .await
        .map(Json)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))
}


async fn tokens_revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    tok: Bearer,
) -> Result<Json<Value>, Response> {
    let user_id = verify_token(&state, &tok.0, "access")
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
    use rand::Rng;

    fn hex_char(rng: &mut impl Rng) -> char {
        b"0123456789abcdef"[rng.gen_range(0..16)] as char
    }

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
