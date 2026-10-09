//! Cloudflare Turnstile 服务端校验：登录 / LDAP 登录 / 注册前的人机验证。
//! 配置 server.turnstile_site_key（明面下发前端渲染 widget）+
//! server.turnstile_secret_key（服务端调 siteverify），两者都非空才启用；
//! secret 为空 = 功能关闭，直接放行。
//! 协议：https://developers.cloudflare.com/turnstile/get-started/

use std::time::Duration;

use serde::Deserialize;

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

#[derive(Deserialize)]
struct SiteverifyResp {
    success: bool,
    #[serde(rename = "error-codes", default)]
    error_codes: Vec<String>,
}

/// 开关：两个 key 都非空才启用。只配一个（如只配了 secret）必须视为关闭——
/// 否则服务端强制要 token 而前端没有 widget，会把所有用户锁在登录页外。
pub fn enabled(site_key: &str, secret: &str) -> bool {
    !site_key.trim().is_empty() && !secret.trim().is_empty()
}

/// 校验前端提交的 turnstile token（300 秒有效、一次性，验过即作废）。
/// 未启用（见 enabled）→ 恒放行；token 缺失或校验失败 → Err(用户可读消息)，
/// 由调用方映射成 403（不计入登录失败限流，防爆破仍由 guard 兜底）。
pub async fn verify(site_key: &str, secret: &str, token: &str, remote_ip: &str) -> Result<(), String> {
    let secret = secret.trim();
    if !enabled(site_key, secret) {
        return Ok(());
    }
    let token = token.trim();
    if token.is_empty() {
        return Err("请先完成人机验证".into());
    }
    // 块作用域：Serializer 含 &dyn（!Send），必须在 .await 前析构，否则 handler future 非 Send
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("secret", secret);
        form.append_pair("response", token);
        if !remote_ip.is_empty() {
            form.append_pair("remoteip", remote_ip);
        }
        form.finish()
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Turnstile 客户端构建失败: {e}"))?;
    let resp = client
        .post(SITEVERIFY_URL)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Turnstile 校验请求失败: {e}"))?;
    let r: SiteverifyResp = resp
        .json()
        .await
        .map_err(|e| format!("Turnstile 响应解析失败: {e}"))?;
    if r.success {
        Ok(())
    } else {
        tracing::warn!(codes = ?r.error_codes, "turnstile verify failed");
        Err("人机验证未通过，请重试".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 开关：两个 key 都非空才启用，只配一个 = 关
    #[test]
    fn enabled_requires_both_keys() {
        assert!(enabled("1x00000000000000000000AA", "1x0000000000000000000000000000000AA"));
        assert!(!enabled("", ""));
        assert!(!enabled("site-only", ""));
        assert!(!enabled("", "secret-only"));
        assert!(!enabled("  ", "secret-only")); // 空白串同空串
    }

    /// 未启用（含只配一半）缺 token 也放行（存量部署行为不变）
    #[tokio::test]
    async fn disabled_passes_through() {
        assert!(verify("", "", "", "").await.is_ok());
        assert!(verify("", "", "any-token", "1.2.3.4").await.is_ok());
        assert!(verify("site-only", "", "any-token", "").await.is_ok());
    }

    /// 启用后 token 缺失/空白直接拒，不出网
    #[tokio::test]
    async fn enabled_requires_token() {
        assert_eq!(
            verify(
                "1x00000000000000000000AA",
                "1x0000000000000000000000000000000AA",
                "  ",
                ""
            )
            .await,
            Err("请先完成人机验证".into())
        );
    }
}
