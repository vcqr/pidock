//! 登录邮箱验证码（两步验证的第二步）：密码校验通过后发 6 位数字码到账号邮箱，
//! 前端凭 challenge id + 验证码换正式 token 对。
//!
//! - 挑战存 Redis，TTL 10 分钟；只存验证码的 HMAC（jwt_secret 做钥），不落明文
//! - 单挑战最多试 5 次，超限销毁；重发冷却 60s；同邮箱发码 5 封/小时（防轰炸）
//! - Redis 故障时从严：挑战写不进去直接报错（可用性优先不适用于安全门槛）

use hmac::{Hmac, Mac};
use rand::Rng;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::state::AppState;

const CHALLENGE_TTL: u64 = 600; // 验证码 10 分钟有效
const MAX_ATTEMPTS: i64 = 5;
const RESEND_COOLDOWN: u64 = 60;
const MAIL_PER_HOUR: i64 = 5;

type HmacSha256 = Hmac<Sha256>;

/// 校验失败统一返回 (HTTP 状态, 用户可读消息)，由调用方 api_err 映射
pub type OtpError = (axum::http::StatusCode, String);

/// 6 位数字码（十万分位补零）
pub fn generate_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}

/// 验证码 HMAC-SHA256 摘要（hex）。存储与比对都走摘要，Redis 泄露不暴露码值
fn code_digest(secret: &str, code: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac 接受任意长度密钥");
    mac.update(code.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex64(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 常数时间比对（hmac::verify_slice），避免逐字节比较的计时侧信道
fn code_matches(secret: &str, expected_hex: &str, code: &str) -> bool {
    let Some(expected) = unhex64(expected_hex) else {
        return false;
    };
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(code.trim().as_bytes());
    mac.verify_slice(&expected).is_ok()
}

/// 展示用脱敏：user@example.com → u***@example.com；单字符本地段只打码不加星
pub fn mask_email(email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let masked = match local.chars().next() {
        Some(c) if local.chars().count() > 1 => format!("{c}***"),
        Some(c) => c.to_string(),
        None => "***".into(),
    };
    if domain.is_empty() {
        masked
    } else {
        format!("{masked}@{domain}")
    }
}

#[derive(Serialize, Deserialize)]
struct Challenge {
    /// 收件邮箱（小写），重发时直接复用，不信任前端回传
    email: String,
    /// 验证码 HMAC 摘要
    code: String,
    attempts: i64,
    /// 验证通过后要继续的事（登录：已验密的用户；注册：待建号的资料）
    pending: Pending,
}

/// 验证通过后的后续动作，随挑战一起存 Redis
#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pending {
    /// 登录：密码已校验通过，直接给该用户发 token
    Login { user_id: String },
    /// 注册：密码已哈希，邀请码意图已预检；建号在验证通过后原子完成
    Register {
        password_hash: String,
        invite_code: String,
        /// 引导邀请码意图（建号时复核管理员数量）
        bootstrap: bool,
    },
}

/// 发码成功后给登录接口的返回信息
pub struct Started {
    pub challenge: String,
    pub masked_email: String,
}

fn challenge_key(challenge: &str) -> String {
    format!("login:otp:ch:{}", challenge.trim())
}

/// 同邮箱发码频率（含首次与重发）：超过 5 封/小时拒绝
async fn mail_quota_ok(state: &AppState, email_lc: &str) -> Result<(), OtpError> {
    let key = format!("login:otp:mail:{email_lc}");
    let mut conn = state.redis.clone();
    let n: i64 = redis::cmd("INCR")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            tracing::warn!("otp: redis incr failed: {e}");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "验证码服务暂不可用".to_string())
        })?;
    if n == 1 {
        let _: Result<(), _> = conn.expire(&key, 3600).await;
    }
    if n > MAIL_PER_HOUR {
        let ttl: i64 = conn.ttl::<_, i64>(&key).await.unwrap_or(3600).max(1);
        let mins = (ttl + 59) / 60;
        return Err((
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            format!("该邮箱验证码发送过于频繁，请约 {mins} 分钟后再试"),
        ));
    }
    Ok(())
}

async fn read_challenge(state: &AppState, challenge: &str) -> Result<Challenge, OtpError> {
    let mut conn = state.redis.clone();
    let raw: Option<String> = conn
        .get(challenge_key(challenge))
        .await
        .map_err(|e| {
            tracing::warn!("otp: redis get failed: {e}");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "验证码服务暂不可用".to_string())
        })?;
    let raw = raw.ok_or_else(|| {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            "验证码已过期，请重新登录".to_string(),
        )
    })?;
    serde_json::from_str(&raw).map_err(|e| {
        tracing::warn!("otp: challenge 解析失败: {e}");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "验证码状态损坏，请重新登录".to_string(),
        )
    })
}

/// 密码校验/注册预检通过后发码并建挑战。Err = 发送被限流或邮件投递失败
pub async fn start(state: &AppState, email_lc: &str, pending: Pending) -> Result<Started, OtpError> {
    mail_quota_ok(state, email_lc).await?;
    let code = generate_code();
    let challenge = Uuid::now_v7().simple().to_string();
    let ch = Challenge {
        email: email_lc.into(),
        code: code_digest(&state.cfg.jwt_secret, &code),
        attempts: 0,
        pending,
    };
    let mut conn = state.redis.clone();
    let payload = serde_json::to_string(&ch).map_err(|e| {
        tracing::warn!("otp: challenge 序列化失败: {e}");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "验证码服务暂不可用".to_string(),
        )
    })?;
    let _: Result<(), _> = conn
        .set_ex(challenge_key(&challenge), payload, CHALLENGE_TTL)
        .await;
    send_mail(state, email_lc, &code).await?;
    tracing::info!(email = %email_lc, "login code sent");
    Ok(Started {
        challenge,
        masked_email: mask_email(email_lc),
    })
}

/// 重发：新码覆盖旧码并重置尝试次数，60s 冷却 + 共享每邮箱小时配额
pub async fn resend(state: &AppState, challenge: &str) -> Result<Started, OtpError> {
    let mut ch = read_challenge(state, challenge).await?;
    let mut conn = state.redis.clone();
    let nx: Option<String> = redis::cmd("SET")
        .arg(format!("login:otp:rl:{}", challenge.trim()))
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(RESEND_COOLDOWN)
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            tracing::warn!("otp: redis setnx failed: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "验证码服务暂不可用".to_string(),
            )
        })?;
    if nx.is_none() {
        return Err((
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "发送过于频繁，请约 1 分钟后再试".to_string(),
        ));
    }
    mail_quota_ok(state, &ch.email).await?;
    let code = generate_code();
    ch.code = code_digest(&state.cfg.jwt_secret, &code);
    ch.attempts = 0;
    let payload = serde_json::to_string(&ch).map_err(|_| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "验证码服务暂不可用".to_string(),
        )
    })?;
    let _: Result<(), _> = conn
        .set_ex(challenge_key(challenge), payload, CHALLENGE_TTL)
        .await;
    send_mail(state, &ch.email, &code).await?;
    tracing::info!(email = %ch.email, "login code resent");
    Ok(Started {
        challenge: challenge.trim().to_string(),
        masked_email: mask_email(&ch.email),
    })
}

/// 验证成功后的产物：收件邮箱 + 存进挑战的后续动作
pub struct Verified {
    pub email: String,
    pub pending: Pending,
}

/// 校验验证码：正确返回后续动作并销毁挑战；错误扣尝试次数（5 次销毁）
pub async fn verify(state: &AppState, challenge: &str, code: &str) -> Result<Verified, OtpError> {
    let mut ch = read_challenge(state, challenge).await?;
    let key = challenge_key(challenge);
    let mut conn = state.redis.clone();
    if ch.attempts >= MAX_ATTEMPTS {
        let _: Result<(), _> = conn.del(&key).await;
        return Err((
            axum::http::StatusCode::UNAUTHORIZED,
            "尝试次数过多，请重新登录".to_string(),
        ));
    }
    if !code_matches(&state.cfg.jwt_secret, &ch.code, code) {
        ch.attempts += 1;
        let msg = if ch.attempts >= MAX_ATTEMPTS {
            let _: Result<(), _> = conn.del(&key).await;
            "尝试次数过多，请重新登录".to_string()
        } else {
            let ttl: i64 = conn.ttl::<_, i64>(&key).await.unwrap_or(0).max(1) as i64;
            let payload = serde_json::to_string(&ch)
                .map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "验证码服务暂不可用".to_string()))?;
            let _: Result<(), _> = conn.set_ex(&key, payload, ttl as u64).await;
            format!("验证码错误（还可尝试 {} 次）", MAX_ATTEMPTS - ch.attempts)
        };
        tracing::warn!(email = %ch.email, attempts = ch.attempts, "login code verify failed (audit)");
        return Err((axum::http::StatusCode::UNAUTHORIZED, msg));
    }
    let _: Result<(), _> = conn.del(&key).await;
    Ok(Verified {
        email: ch.email,
        pending: ch.pending,
    })
}

async fn send_mail(state: &AppState, to: &str, code: &str) -> Result<(), OtpError> {
    crate::mailer::send_login_code(&state.cfg.email, to, code, (CHALLENGE_TTL / 60) as i64)
        .await
        .map_err(|e| {
            tracing::warn!("otp: 邮件发送失败: {e}");
            (
                axum::http::StatusCode::BAD_GATEWAY,
                "验证码邮件发送失败，请稍后重试".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_six_digits() {
        for _ in 0..200 {
            let c = generate_code();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|ch| ch.is_ascii_digit()));
        }
    }

    #[test]
    fn digest_roundtrip_and_secret_sensitivity() {
        let d = code_digest("secret-a", "123456");
        assert_eq!(d, code_digest("secret-a", "123456"));
        assert_ne!(d, code_digest("secret-a", "123457"));
        assert_ne!(d, code_digest("secret-b", "123456"));
        assert_eq!(d.len(), 64);
    }

    /// 常数时间比对：正/误码判定与坏摘要防御
    #[test]
    fn code_matches_constant_time() {
        let d = code_digest("secret-a", "123456");
        assert!(code_matches("secret-a", &d, "123456"));
        assert!(code_matches("secret-a", &d, " 123456 ")); // 前后空白容忍
        assert!(!code_matches("secret-a", &d, "123457"));
        assert!(!code_matches("secret-b", &d, "123456"));
        assert!(!code_matches("secret-a", "not-hex", "123456"));
        assert!(!code_matches("secret-a", "", "123456"));
    }

    #[test]
    fn mask_email_cases() {
        assert_eq!(mask_email("user@example.com"), "u***@example.com");
        assert_eq!(mask_email("a@b.co"), "a@b.co"); // 单字符本地段
        assert_eq!(mask_email("no-at-sign"), "n***");
        assert_eq!(mask_email(""), "***");
    }

    /// Pending 随挑战落 Redis：两种用途都要无损往返
    #[test]
    fn pending_serde_roundtrip() {
        for p in [
            Pending::Login { user_id: "u1".into() },
            Pending::Register {
                password_hash: "$argon2id$v=19$...".into(),
                invite_code: "AB-CD".into(),
                bootstrap: true,
            },
        ] {
            let s = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_json::from_str::<Pending>(&s).unwrap(), p);
        }
    }
}
