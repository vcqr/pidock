//! 防爆破限流：基于 Redis 的失败计数。
//!
//! - 登录（标准/LDAP）按「目标账号」与「来源 IP」双维度计数：账号 5 次、
//!   IP 20 次（防撞库横扫），窗口 15 分钟，超限 429；成功登录清零
//! - 注册按「来源 IP」计所有尝试（成功失败都算），1 小时 10 次
//! - Redis 故障时：登录限流放行并记日志（可用性优先，自托管场景不能因 Redis
//!   抖动锁死所有人）；注册路径 fail-closed 拒绝（公网防「打挂 Redis 解锁爆破」）

use axum::http::{HeaderMap, StatusCode};
use mongodb::bson::doc;
use redis::AsyncCommands;

use crate::auth::api_err;
use crate::state::AppState;

const LOGIN_ACCT_MAX: i64 = 5;
const LOGIN_IP_MAX: i64 = 20;
const LOGIN_WINDOW: u64 = 900; // 15 分钟
const REG_IP_MAX: i64 = 10;
const REG_WINDOW: u64 = 3600; // 1 小时

/// 来源 IP：反代后配 trust_proxy 才信任 X-Forwarded-For，否则取直连对端
pub fn client_ip(cfg: &crate::config::Config, headers: &HeaderMap, peer: std::net::SocketAddr) -> String {
    if cfg.trust_proxy {
        if let Some(xff) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(first) = xff.split(',').next() {
                let ip = first.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
    }
    peer.ip().to_string()
}

/// Redis INCR + 首次 EXPIRE；返回 (次数, 剩余锁定秒数)
async fn incr_window(state: &AppState, key: &str, window: u64) -> (i64, u64) {
    incr_window_raw(state, key, window)
        .await
        .unwrap_or((0, 0))
}

/// 同上，但 Redis 故障时返回 Err——由调用方决定放行（登录，可用性优先）
/// 还是拒绝（注册/发码，安全门槛 fail-closed）
async fn incr_window_raw(
    state: &AppState,
    key: &str,
    window: u64,
) -> Result<(i64, u64), ()> {
    let mut conn = state.redis.clone();
    let count: i64 = redis::cmd("INCR")
        .arg(key)
        .query_async(&mut conn)
        .await
        .map_err(|_| ())?;
    if count == 1 {
        let _: Result<(), _> = conn
            .expire(key, window as i64)
            .await;
    }
    let ttl: u64 = conn.ttl(key).await.unwrap_or(0).max(0) as u64;
    Ok((count, ttl))
}

async fn get_count(state: &AppState, key: &str) -> i64 {
    let mut conn = state.redis.clone();
    conn.get(key).await.unwrap_or(0)
}

async fn del_keys(state: &AppState, keys: &[String]) {
    let mut conn = state.redis.clone();
    let _: Result<(), _> = conn.del(keys).await;
}

fn too_many(what: &str, ttl: u64) -> axum::response::Response {
    let mins = ttl.div_ceil(60).max(1);
    api_err(
        StatusCode::TOO_MANY_REQUESTS,
        format!("{what}尝试次数过多，请约 {mins} 分钟后再试"),
    )
}

/// 登录前置检查：账号或 IP 任一超限即拒绝。Err = 已被限流
pub async fn login_allowed(
    state: &AppState,
    account: &str,
    ip: &str,
) -> Result<(), axum::response::Response> {
    let acct_key = format!("guard:login:acct:{}", account.to_lowercase());
    let ip_key = format!("guard:login:ip:{ip}");
    let (acct_n, ip_n) = (get_count(state, &acct_key).await, get_count(state, &ip_key).await);
    if acct_n >= LOGIN_ACCT_MAX {
        let ttl: u64 = {
            let mut conn = state.redis.clone();
            conn.ttl::<_, i64>(&acct_key).await.unwrap_or(0).max(1) as u64
        };
        return Err(too_many("该账号密码", ttl));
    }
    if ip_n >= LOGIN_IP_MAX {
        let ttl: u64 = {
            let mut conn = state.redis.clone();
            conn.ttl::<_, i64>(&ip_key).await.unwrap_or(0).max(1) as u64
        };
        return Err(too_many("此来源 IP 的登录", ttl));
    }
    Ok(())
}

/// 登录失败记账：账号与 IP 两个计数都 +1
pub async fn login_failure(state: &AppState, account: &str, ip: &str) {
    let (acct_n, _) = incr_window(
        state,
        &format!("guard:login:acct:{}", account.to_lowercase()),
        LOGIN_WINDOW,
    )
    .await;
    let (ip_n, _) = incr_window(state, &format!("guard:login:ip:{ip}"), LOGIN_WINDOW).await;
    tracing::warn!(
        account = %account,
        ip = %ip,
        acct_fails = acct_n,
        ip_fails = ip_n,
        "login failed (audit)"
    );
}

/// 登录成功清零
pub async fn login_clear(state: &AppState, account: &str, ip: &str) {
    del_keys(
        state,
        &[
            format!("guard:login:acct:{}", account.to_lowercase()),
            format!("guard:login:ip:{ip}"),
        ],
    )
    .await;
}

/// 注册前置检查：同 IP 1 小时内全部尝试（成功+失败）计入。
/// fail-closed：Redis 故障时拒绝——公网不能留「打挂 Redis 解锁爆破/刷邮件」的门
pub async fn register_allowed(state: &AppState, ip: &str) -> Result<(), axum::response::Response> {
    let key = format!("guard:reg:ip:{ip}");
    let (n, ttl) = incr_window_raw(state, &key, REG_WINDOW).await.map_err(|_| {
        api_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "注册服务暂不可用，请稍后再试",
        )
    })?;
    if n > REG_IP_MAX {
        return Err(too_many("此来源 IP 的注册", ttl));
    }
    Ok(())
}

/// 注册失败审计（邀请码错误/邮箱冲突等）
pub fn register_failure(ip: &str, reason: &str) {
    tracing::warn!(ip = %ip, reason, "register rejected (audit)");
}

/// Mongo 记录一次失败明细（可选的轻量审计表，便于管理端后续回查）
#[allow(dead_code)]
pub async fn audit_log(state: &AppState, kind: &str, account: &str, ip: &str, detail: &str) {
    let _ = state
        .mongo
        .collection::<mongodb::bson::Document>("audit_log")
        .insert_one(doc! {
            "kind": kind,
            "account": account.to_lowercase(),
            "ip": ip,
            "detail": detail,
            "at": chrono::Utc::now().to_rfc3339(),
        })
        .await;
}
