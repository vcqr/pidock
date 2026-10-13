//! 飞书开放平台事件订阅适配器。
//!
//! 流程：加密体验签（X-Lark-Signature = sha256(ts+nonce+encrypt_key+body)）
//! 与解密（AES-256-CBC：key = SHA256(encrypt_key)，密文 = base64(iv+data)，
//! PKCS7）→ url_verification 原样回 challenge → 事件 token 比对
//! verification_token → message_id 去重（飞书失败重试会重复推送）→ 文本
//! 消息清洗 @ 占位 → 复用 webhook::deliver 投递节点 → 登记 kind=im_feishu
//! 的挂起回调，agent 空闲时经 post_callback 分发回这里调平台 API 回消息。

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mongodb::bson::{doc, Document as BsonDoc};
use sha2::{Digest, Sha256};
use serde_json::{json, Value};

use crate::state::AppState;
use crate::webhook;

const FEISHU_BASE: &str = "https://open.feishu.cn";
/// 单次调用飞书 API 的超时
const HTTP_TIMEOUT_SECS: u64 = 10;
/// 回调投递重试节奏（与 webhook 回调一致）
const RETRY_BACKOFF_SECS: [u64; 2] = [10, 60];

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ------------------------------------------------------------ 验签与解密

/// 配置 Encrypt Key 后飞书携带签名头；缺失时放行（token 校验兜底），不匹配即拒
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

// ------------------------------------------------------------ 事件处理

/// 事件订阅入口主体。业务失败统一回 200（飞书对非 2xx 会重试，重试救不了
/// 节点离线这类错误，反而造成重复推送）；防伪失败才回 4xx。
pub(crate) async fn handle_event(
    state: &AppState,
    channel: &BsonDoc,
    headers: &HeaderMap,
    raw_body: &str,
) -> Response {
    let channel_id = channel.get_str("_id").unwrap_or_default().to_string();

    // 1) 外层 JSON；加密体先验签再解密
    let mut payload: Value = match serde_json::from_str(raw_body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(channel = %channel_id, "feishu: 非法 JSON: {e}");
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false}))).into_response();
        }
    };
    if let Some(encrypted) = payload.get("encrypt").and_then(|v| v.as_str()) {
        let encrypt_key = channel.get_str("encrypt_key").unwrap_or_default();
        if encrypt_key.is_empty() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "收到加密事件但渠道未配置 Encrypt Key"})))
                .into_response();
        }
        if let Err(e) = verify_signature(encrypt_key, headers, raw_body) {
            tracing::warn!(channel = %channel_id, "feishu: {e}");
            return (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": e}))).into_response();
        }
        match decrypt_event(encrypt_key, encrypted) {
            Ok(v) => payload = v,
            Err(e) => {
                tracing::warn!(channel = %channel_id, "feishu: {e}");
                return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e}))).into_response();
            }
        }
    }

    // 2) 订阅地址验证：原样回 challenge（飞书要求 1s 内）
    if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        if let Err(e) = check_token(channel, payload.get("token").and_then(|v| v.as_str())) {
            tracing::warn!(channel = %channel_id, "feishu: url_verification {e}");
            return (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": e}))).into_response();
        }
        let challenge = payload.get("challenge").cloned().unwrap_or(Value::Null);
        tracing::info!(channel = %channel_id, "feishu: url_verification 通过");
        return Json(json!({ "challenge": challenge })).into_response();
    }

    // 3) 事件信封：token 校验 + 类型分发
    if check_token(channel, payload.pointer("/header/token").and_then(|v| v.as_str())).is_err() {
        return (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "token mismatch"}))).into_response();
    }
    let event_type = payload
        .pointer("/header/event_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if event_type != "im.message.receive_v1" {
        return Json(json!({"ok": true, "ignored": event_type})).into_response();
    }
    let message = payload.pointer("/event/message").cloned().unwrap_or(json!({}));
    let message_id = message.get("message_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let chat_id = message.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let message_type = message.get("message_type").and_then(|v| v.as_str()).unwrap_or("");
    if message_id.is_empty() || chat_id.is_empty() {
        return Json(json!({"ok": true, "ignored": "missing message_id/chat_id"})).into_response();
    }

    // 4) 幂等：飞书超时/失败会重推同一事件
    let dedup_key = format!("im:dedup:{channel_id}:{message_id}");
    let mut conn = state.redis.clone();
    let fresh: Result<i64, _> = redis::cmd("SET")
        .arg(&dedup_key)
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await;
    if fresh == Ok(0) {
        return Json(json!({"ok": true, "dedup": true})).into_response();
    }

    // 5) v1 只接文本；content 是 JSON 字符串需二次解析，@ 占位清洗
    if message_type != "text" {
        tracing::info!(channel = %channel_id, "feishu: 忽略非文本消息 {message_type}");
        return Json(json!({"ok": true, "ignored": message_type})).into_response();
    }
    let content: Value = serde_json::from_str(message.get("content").and_then(|v| v.as_str()).unwrap_or("{}"))
        .unwrap_or(json!({}));
    let text = clean_mention(content.get("text").and_then(|v| v.as_str()).unwrap_or(""));
    if text.is_empty() {
        return Json(json!({"ok": true, "ignored": "empty text"})).into_response();
    }

    // 6) 投递节点（复用 webhook 编排）+ 立即返回
    let user_id = channel.get_str("user_id").unwrap_or_default().to_string();
    let machine_id = channel.get_str("machine_id").unwrap_or_default().to_string();
    let started = std::time::Instant::now();
    match webhook::deliver(state, &user_id, channel, &text).await {
        Ok((session_id, _command_id)) => {
            audit(state, channel, &machine_id, &session_id, "ok", started).await;
            match register_pending(state, channel, &session_id, &text, &chat_id).await {
                Ok(_) => Json(json!({"ok": true, "session_id": session_id})).into_response(),
                Err(e) => {
                    tracing::warn!(channel = %channel_id, "feishu: 回调登记失败 {e}");
                    Json(json!({"ok": true, "session_id": session_id, "reply": false})).into_response()
                }
            }
        }
        Err(e) => {
            let status = if e == "refused_offline" { "refused_offline" } else { "error" };
            audit(state, channel, &machine_id, "", status, started).await;
            // 投递失败直接在群里说明（best effort），避免用户干等
            let hint = if e == "refused_offline" { "节点离线，消息未送达" } else { &e };
            let _ = reply_text(state, channel, &chat_id, &format!("⚠️ PiDock 投递失败：{hint}")).await;
            Json(json!({"ok": false, "error": e})).into_response()
        }
    }
}

/// @ 机器人的文本带 @_user_1 占位符，按空白段剔除
fn clean_mention(text: &str) -> String {
    text.split_whitespace()
        .filter(|seg| !seg.starts_with("@_user_"))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn audit(
    state: &AppState,
    channel: &BsonDoc,
    machine_id: &str,
    session_id: &str,
    status: &str,
    started: std::time::Instant,
) {
    let _ = state
        .mongo
        .collection::<mongodb::bson::Document>("commands")
        .insert_one(doc! {
            "command_id": uuid::Uuid::now_v7().to_string(),
            "user_id": channel.get_str("user_id").unwrap_or_default(),
            "machine_id": machine_id,
            "session_id": session_id,
            "type": "im.invoke",
            "payload": channel.get_str("name").unwrap_or_default(),
            "source": "im",
            "endpoint_id": channel.get_str("_id").unwrap_or_default(),
            "status": status,
            "duration_ms": started.elapsed().as_millis() as i64,
            "ts": now_rfc3339(),
        })
        .await;
}

/// 登记 IM 挂起回调：agent 空闲时 post_callback 按 kind 分发回 reply_completed
async fn register_pending(
    state: &AppState,
    channel: &BsonDoc,
    session_id: &str,
    text: &str,
    chat_id: &str,
) -> Result<String, String> {
    let cb_id = uuid::Uuid::now_v7().to_string();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(webhook::CALLBACK_TTL_SECS)).to_rfc3339();
    state
        .mongo
        .collection::<BsonDoc>("webhook_pending_callbacks")
        .insert_one(doc! {
            "_id": &cb_id,
            "kind": "im_feishu",
            "im_channel_id": channel.get_str("_id").unwrap_or_default(),
            "user_id": channel.get_str("user_id").unwrap_or_default(),
            "machine_id": channel.get_str("machine_id").unwrap_or_default(),
            "session_id": session_id,
            "callback_url": "",
            "callback_secret": "",
            "reply_chat_id": chat_id,
            "text": text,
            "status": "pending",
            "created_at": now_rfc3339(),
            "expires_at": &expires_at,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(cb_id)
}

// ------------------------------------------------------------ 回消息

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

/// 发一条文本到会话（chat_id），返回是否成功
async fn reply_text(state: &AppState, channel: &BsonDoc, chat_id: &str, text: &str) -> Result<(), String> {
    let token = tenant_token(state, channel).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    // content 同事件方向一样是 JSON 字符串
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

/// post_callback 的 im_feishu 分支：agent 一轮结束/超时，把结果发回群聊
pub(crate) async fn reply_completed(state: &AppState, cb: &BsonDoc, timed_out: bool) {
    let cb_id = cb.get_str("_id").unwrap_or_default().to_string();
    let channel_id = cb.get_str("im_channel_id").unwrap_or_default().to_string();
    let chat_id = cb.get_str("reply_chat_id").unwrap_or_default().to_string();
    let session_id = cb.get_str("session_id").unwrap_or_default().to_string();

    let channel = match state
        .mongo
        .collection::<BsonDoc>("im_channels")
        .find_one(doc! { "_id": &channel_id })
        .await
    {
        Ok(Some(c)) => c,
        _ => {
            webhook::finish_pending(state, &cb_id, "failed", 0, None, "im channel missing").await;
            return;
        }
    };

    let (status, text) = if timed_out {
        ("timeout", "⏱️ PiDock 任务超过 30 分钟未结束，已放弃等待。".to_string())
    } else {
        match webhook::fetch_last_assistant(state, &session_id).await {
            Some(m) => {
                let t = m.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if t.is_empty() {
                    ("no_output", "PiDock 任务已结束，本轮没有文本输出。".to_string())
                } else {
                    ("ok", t)
                }
            }
            None => ("no_output", "PiDock 任务已结束，本轮没有文本输出。".to_string()),
        }
    };

    let mut attempts = 0u32;
    let mut last_code: Option<u16> = None;
    let mut last_error = String::new();
    for i in 0..=RETRY_BACKOFF_SECS.len() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(RETRY_BACKOFF_SECS[i - 1])).await;
        }
        attempts += 1;
        match reply_text(state, &channel, &chat_id, &text).await {
            Ok(()) => {
                webhook::finish_pending(state, &cb_id, "delivered", attempts, last_code, "").await;
                tracing::info!(delivery = %cb_id, "feishu 回复投递成功");
                return;
            }
            Err(e) => {
                last_error = e;
                last_code = None;
            }
        }
    }
    webhook::finish_pending(state, &cb_id, "failed", attempts, last_code, &last_error).await;
    tracing::warn!(delivery = %cb_id, status = %status, "feishu 回复投递失败: {last_error}");
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
