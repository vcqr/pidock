//! 飞书开放平台适配器（HTTP 推送型）。
//!
//! 只实现 [`crate::im::ImPlatform`] 的四件事：凭证校验、入站验证/解密/
//! 解析（url_verification 回 challenge、事件 token 比对、AES-256-CBC 解密）、
//! 回文本（tenant_access_token + im/v1/messages）。去重/审计/deliver/
//! pending 登记由共享流水线（crate::im::process_message）统一执行。

use async_trait::async_trait;
use axum::http::HeaderMap;
use mongodb::bson::Document as BsonDoc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::im::{ChannelBody, InboundDecision, InboundMessage};
use crate::state::AppState;

const FEISHU_BASE: &str = "https://open.feishu.cn";
/// 单次调用飞书 API 的超时
const HTTP_TIMEOUT_SECS: u64 = 10;

pub(crate) struct FeishuPlatform;

// ------------------------------------------------------------ 验签与解密

/// 配置 Encrypt Key 后飞书携带签名头：sha256(ts + nonce + encrypt_key + body)；
/// 缺失时放行（token 校验兜底），不匹配即拒
fn verify_signature(encrypt_key: &str, headers: &HeaderMap, raw_body: &str) -> Result<(), String> {
    let Some(sig) = headers.get("x-lark-signature").and_then(|v| v.to_str().ok()) else {
        tracing::warn!("feishu: 请求未带签名头，回落 token 校验");
        return Ok(());
    };
    let ts = headers
        .get("x-lark-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let nonce = headers
        .get("x-lark-request-nonce")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut hasher = Sha256::new();
    hasher.update(ts.as_bytes());
    hasher.update(nonce.as_bytes());
    hasher.update(encrypt_key.as_bytes());
    hasher.update(raw_body.as_bytes());
    let expect = format!("{:x}", hasher.finalize());
    if expect != sig {
        return Err("签名不匹配".into());
    }
    Ok(())
}

/// 解密事件体：base64(iv + AES-256-CBC(plaintext))，key = SHA256(encrypt_key)，PKCS7
fn decrypt_event(encrypt_key: &str, encrypted: &str) -> Result<Value, String> {
    use aes::Aes256;
    use base64::Engine;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    type Aes256CbcDec = cbc::Decryptor<Aes256>;

    let data = base64::engine::general_purpose::STANDARD
        .decode(encrypted.trim())
        .map_err(|e| format!("密文 base64 解码失败: {e}"))?;
    if data.len() < 32 {
        return Err("密文过短".into());
    }
    let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
    let iv: [u8; 16] = data[..16].try_into().map_err(|_| "iv 长度异常")?;
    let plaintext = Aes256CbcDec::new((&key).into(), (&iv).into())
        .decrypt_padded_vec_mut::<Pkcs7>(&data[16..])
        .map_err(|e| format!("AES 解密失败: {e}"))?;
    serde_json::from_slice(&plaintext).map_err(|e| format!("解密后不是合法 JSON: {e}"))
}

fn check_token(channel: &BsonDoc, got: Option<&str>) -> Result<(), String> {
    let expect = channel.get_str("verification_token").unwrap_or_default();
    if expect.is_empty() {
        return Err("渠道未配置 Verification Token".into());
    }
    if got != Some(expect) {
        return Err("Verification Token 不匹配".into());
    }
    Ok(())
}

/// @ 机器人的文本带 @_user_1 占位符，按空白段剔除
fn clean_mention(text: &str) -> String {
    text.split_whitespace()
        .filter(|seg| !seg.starts_with("@_user_"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ------------------------------------------------------------ 适配器实现

#[async_trait]
impl crate::im::ImPlatform for FeishuPlatform {
    fn kind(&self) -> &'static str {
        "feishu"
    }

    fn validate_channel(&self, body: &ChannelBody) -> Result<(), String> {
        if body.verification_token.trim().is_empty() {
            return Err("Verification Token 不能为空（飞书开发者后台「事件与回调 → 加密策略」获取）".into());
        }
        Ok(())
    }

    async fn parse_inbound(
        &self,
        _state: &AppState,
        channel: &BsonDoc,
        headers: &HeaderMap,
        raw_body: &str,
    ) -> Result<InboundDecision, String> {
        // 1) 外层 JSON；加密体先验签再解密
        let mut payload: Value = serde_json::from_str(raw_body).map_err(|e| format!("非法 JSON: {e}"))?;
        if let Some(encrypted) = payload.get("encrypt").and_then(|v| v.as_str()) {
            let encrypt_key = channel.get_str("encrypt_key").unwrap_or_default();
            if encrypt_key.is_empty() {
                return Err("收到加密事件但渠道未配置 Encrypt Key".into());
            }
            verify_signature(encrypt_key, headers, raw_body)?;
            payload = decrypt_event(encrypt_key, encrypted)?;
        }

        // 2) 订阅地址验证：原样回 challenge（飞书要求 1s 内，token 先校验）
        if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
            check_token(channel, payload.get("token").and_then(|v| v.as_str()))?;
            tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: url_verification 通过");
            let challenge = payload.get("challenge").cloned().unwrap_or(Value::Null);
            return Ok(InboundDecision::Verification(json!({ "challenge": challenge })));
        }

        // 3) 事件信封：token 校验 + 类型分发
        check_token(channel, payload.pointer("/header/token").and_then(|v| v.as_str()))?;
        let event_type = payload
            .pointer("/header/event_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if event_type != "im.message.receive_v1" {
            return Ok(InboundDecision::Ignored(event_type.to_string()));
        }
        let message = payload.pointer("/event/message").cloned().unwrap_or(json!({}));
        let message_id = message.get("message_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let chat_id = message.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let message_type = message.get("message_type").and_then(|v| v.as_str()).unwrap_or("");
        if message_id.is_empty() || chat_id.is_empty() {
            return Ok(InboundDecision::Ignored("missing message_id/chat_id".into()));
        }
        if message_type != "text" {
            tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: 忽略非文本消息 {message_type}");
            return Ok(InboundDecision::Ignored(message_type.to_string()));
        }
        // content 是 JSON 字符串需二次解析；@ 占位清洗
        let content: Value =
            serde_json::from_str(message.get("content").and_then(|v| v.as_str()).unwrap_or("{}"))
                .unwrap_or(json!({}));
        let text = clean_mention(content.get("text").and_then(|v| v.as_str()).unwrap_or(""));
        if text.is_empty() {
            return Ok(InboundDecision::Ignored("empty text".into()));
        }
        Ok(InboundDecision::Message(InboundMessage {
            message_id,
            text,
            chat_key: chat_id.clone(),
            reply_ctx: json!({ "chat_id": chat_id }),
        }))
    }

    async fn reply_text(
        &self,
        state: &AppState,
        channel: &BsonDoc,
        reply_ctx: &Value,
        text: &str,
    ) -> Result<(), String> {
        // 桌面侧活动转发只有 chat_key 兜底（对飞书而言 chat_key 即 chat_id）
        let chat_id = reply_ctx
            .get("chat_id")
            .and_then(|v| v.as_str())
            .or_else(|| reply_ctx.get("chat_key").and_then(|v| v.as_str()))
            .ok_or("reply_ctx 缺少 chat_id")?;
        let token = tenant_token(state, channel).await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| e.to_string())?;
        // content 与事件方向一样是 JSON 字符串
        let content = json!({ "text": text }).to_string();
        let resp = client
            .post(format!("{FEISHU_BASE}/open-apis/im/v1/messages?receive_id_type=chat_id"))
            .bearer_auth(token)
            .json(&json!({ "receive_id": chat_id, "msg_type": "text", "content": content }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let code = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(json!({}));
        if code / 100 == 2 && body.get("code").and_then(|v| v.as_i64()) == Some(0) {
            Ok(())
        } else {
            Err(format!(
                "飞书回消息失败 HTTP {code}: {}",
                body.get("msg").and_then(|v| v.as_str()).unwrap_or("")
            ))
        }
    }
}

/// tenant_access_token：app 凭证换取，Redis 缓存到过期前 5 分钟
async fn tenant_token(state: &AppState, channel: &BsonDoc) -> Result<String, String> {
    let channel_id = channel.get_str("_id").unwrap_or_default().to_string();
    let cache_key = format!("im:feishu:token:{channel_id}");
    let mut conn = state.redis.clone();
    use redis::AsyncCommands;
    if let Ok(Some(cached)) = conn.get::<_, Option<String>>(&cache_key).await {
        if !cached.is_empty() {
            return Ok(cached);
        }
    }
    let app_id = channel.get_str("app_id").unwrap_or_default();
    let app_secret = channel.get_str("app_secret").unwrap_or_default();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{FEISHU_BASE}/open-apis/auth/v3/tenant_access_token/internal"))
        .json(&json!({ "app_id": app_id, "app_secret": app_secret }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    if body.get("code").and_then(|v| v.as_i64()) != Some(0) {
        return Err(format!(
            "获取 tenant_access_token 失败: {}",
            body.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown")
        ));
    }
    let token = body
        .get("tenant_access_token")
        .and_then(|v| v.as_str())
        .ok_or("响应缺少 tenant_access_token")?
        .to_string();
    let expire = body.get("expire").and_then(|v| v.as_i64()).unwrap_or(7200);
    let ttl = expire.saturating_sub(300).clamp(60, 7200) as u64;
    let _: Result<(), _> = conn.set_ex(&cache_key, &token, ttl).await;
    Ok(token)
}

// ------------------------------------------------------------ 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypt_roundtrip_matches_feishu_spec() {
        use aes::Aes256;
        use base64::Engine;
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Aes256CbcEnc = cbc::Encryptor<Aes256>;

        let encrypt_key = "test-encrypt-key";
        let inner = r#"{"challenge":"abc123","token":"t","type":"url_verification"}"#;
        // 按飞书规范加密：key=SHA256(encrypt_key)，base64(iv+ct)
        let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
        let iv: [u8; 16] = rand::random();
        let ct = Aes256CbcEnc::new((&key).into(), (&iv).into()).encrypt_padded_vec_mut::<Pkcs7>(inner.as_bytes());
        let mut blob = iv.to_vec();
        blob.extend(ct);
        let encrypted = base64::engine::general_purpose::STANDARD.encode(&blob);

        let out = decrypt_event(encrypt_key, &encrypted).unwrap();
        assert_eq!(out.get("challenge").and_then(|v| v.as_str()), Some("abc123"));
        // 错误的 key 解不出原文
        assert!(decrypt_event("wrong-key", &encrypted).is_err());
    }

    #[test]
    fn mention_cleaning() {
        assert_eq!(clean_mention("@_user_1 帮我看看构建日志"), "帮我看看构建日志");
        assert_eq!(clean_mention("直接说事"), "直接说事");
        assert_eq!(clean_mention("  "), "");
    }

    #[test]
    fn signature_matches_spec_composition() {
        // sha256(ts + nonce + encrypt_key + body) 组合顺序
        let mut hasher = Sha256::new();
        hasher.update(b"1700000000");
        hasher.update(b"nonce1");
        hasher.update(b"key");
        hasher.update(b"body");
        let expect = format!("{:x}", hasher.finalize());
        let mut hasher2 = Sha256::new();
        hasher2.update(b"1700000000nonce1keybody");
        assert_eq!(expect, format!("{:x}", hasher2.finalize()));
    }
}
