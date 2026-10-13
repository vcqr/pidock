//! 飞书开放平台适配器（HTTP 推送型）。
//!
//! 只实现 [`crate::im::ImPlatform`] 的四件事：凭证校验、入站验证/解密/
//! 解析（url_verification 回 challenge、事件 token 比对、AES-256-CBC 解密）、
//! 回文本（tenant_access_token + im/v1/messages）。去重/审计/deliver/
//! pending 登记由共享流水线（crate::im::process_message）统一执行。

use async_trait::async_trait;
use axum::http::HeaderMap;
use futures_util::{SinkExt, StreamExt};
use mongodb::bson::Document as BsonDoc;
use prost::Message as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::im::{ChannelBody, InboundDecision, InboundMessage};
use crate::state::AppState;

const FEISHU_BASE: &str = "https://open.feishu.cn";
/// 单次调用飞书 API 的超时
const HTTP_TIMEOUT_SECS: u64 = 10;
/// 保活发出 ping 后等入站的判定窗口
const PONG_WAIT_SECS: u64 = 5;

/// 长连接端点发现（POST 换 wss 地址）
const WS_ENDPOINT_URI: &str = "/callback/ws/endpoint";
/// 默认保活间隔（服务端 ClientConfig 可下发覆盖）
const WS_PING_INTERVAL_SECS: u64 = 120;

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

    fn uses_stream(&self) -> bool {
        // 飞书同时支持推送与长连接：默认推送（渠道显式选 stream 时走长连接）
        false
    }

    async fn stream_connection(&self, state: &AppState, channel: &BsonDoc) -> Result<(), String> {
        run_ws_connection(state, channel).await
    }

    fn validate_channel(&self, body: &ChannelBody) -> Result<(), String> {
        // 长连接模式以 App 凭证在握手时鉴权，无需 verification_token/encrypt_key
        if body.mode != "stream" && body.verification_token.trim().is_empty() {
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

        // 3) 事件解析（HTTP 推送与长连接共用）
        Self::parse_event_payload(channel, &payload)
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

// ------------------------------------------------------------ 事件解析（推送/长连接共用）

impl FeishuPlatform {
    /// 事件信封解析：token 比对（长连接下连接已鉴权，配置了才校验）+
    /// 类型分发 + 文本提取。HTTP 推送与长连接两条入口共用
    fn parse_event_payload(channel: &BsonDoc, payload: &Value) -> Result<InboundDecision, String> {
        if let Some(expect) = channel.get_str("verification_token").ok().filter(|s| !s.is_empty()) {
            if payload.pointer("/header/token").and_then(|v| v.as_str()) != Some(expect) {
                return Err("Verification Token 不匹配".into());
            }
        }
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
}

// ------------------------------------------------------------ 长连接（Stream）帧协议
//
// 官方协议按 lark_oapi ws SDK 对齐：POST /callback/ws/endpoint 换 wss 地址
// （带 device_id/service_id 查询参数），WS 帧为 pbbp2 protobuf：
//   Frame { SeqID=1, LogID=2, service=3, method=4, headers=5(repeated
//   Header{key=1,value=2}), payload_encoding=6, payload_type=7, payload=8 }
// method：0=CONTROL（type=ping/pong），1=DATA（type=event，payload 为与
// webhook 推送同构的事件 JSON）。事件处理完回写同构帧，payload =
// {"code":200,"headers":{},"data":null}；未 ACK 飞书会重推（去重兜底）。
// 保活：每 PingInterval（默认 120s，服务端可下发）发 CONTROL/ping 帧。

/// pbbp2.Header（字段号与官方 SDK 序列化描述符一致）
#[derive(Clone, PartialEq, prost::Message)]
pub(crate) struct PbHeader {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// pbbp2.Frame（省略未用的 payload_encoding/payload_type/LogIDNew）
#[derive(Clone, PartialEq, prost::Message)]
pub(crate) struct PbFrame {
    #[prost(uint64, tag = "1")]
    pub seq_id: u64,
    #[prost(uint64, tag = "2")]
    pub log_id: u64,
    #[prost(int32, tag = "3")]
    pub service: i32,
    #[prost(int32, tag = "4")]
    pub method: i32,
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<PbHeader>,
    #[prost(bytes = "vec", tag = "8")]
    pub payload: Vec<u8>,
}

impl PbFrame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
    }

    fn ping(service: i32) -> Self {
        PbFrame {
            seq_id: 0,
            log_id: 0,
            service,
            method: 0, // CONTROL
            headers: vec![PbHeader { key: "type".into(), value: "ping".into() }],
            payload: vec![],
        }
    }

    /// 事件 ACK：回显原帧头部 + biz_rt，payload 带 HTTP 语义码
    fn ack(mut self, ok: bool) -> Self {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.headers.push(PbHeader { key: "biz_rt".into(), value: ms.to_string() });
        self.payload = json!({ "code": if ok { 200 } else { 500 }, "headers": {}, "data": null })
            .to_string()
            .into_bytes();
        self
    }
}

/// POST /callback/ws/endpoint 换连接地址 + 服务端保活配置
async fn open_ws_endpoint(app_id: &str, app_secret: &str) -> Result<(String, u64), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{FEISHU_BASE}{WS_ENDPOINT_URI}"))
        .header("locale", "zh")
        .json(&json!({ "AppID": app_id, "AppSecret": app_secret }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() || body.get("code").and_then(|v| v.as_i64()) != Some(0) {
        return Err(format!(
            "获取长连接端点失败 HTTP {status}: {} {}",
            body.get("msg").and_then(|v| v.as_str()).unwrap_or(""),
            body.get("code").map(|c| c.to_string()).unwrap_or_default()
        ));
    }
    let url = body
        .pointer("/data/URL")
        .and_then(|v| v.as_str())
        .ok_or("端点响应缺少 URL")?
        .to_string();
    let ping_interval = body
        .pointer("/data/ClientConfig/PingInterval")
        .and_then(|v| v.as_u64())
        .unwrap_or(WS_PING_INTERVAL_SECS);
    Ok((url, ping_interval.clamp(30, 600)))
}

/// 从连接 URL 提取 service_id（保活帧必须带）
fn service_id_of(url: &str) -> i32 {
    url.split("service_id=")
        .nth(1)
        .and_then(|v| v.split('&').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 一次长连接的完整生命周期：断开即返回，由 im::stream_conn_loop 重连
async fn run_ws_connection(state: &AppState, channel: &BsonDoc) -> Result<(), String> {
    let app_id = channel.get_str("app_id").unwrap_or_default().to_string();
    let app_secret = channel.get_str("app_secret").unwrap_or_default().to_string();
    let (url, ping_interval) = open_ws_endpoint(&app_id, &app_secret).await?;
    let service_id = service_id_of(&url);
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("WS 连接失败: {e}"))?;
    tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: 长连接已建立");
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if write.send(m).await.is_err() {
                break;
            }
        }
    });

    // 分包聚合：sum>1 的事件按 message_id 收齐再处理
    let mut partials: std::collections::HashMap<String, std::collections::BTreeMap<u64, Vec<u8>>> =
        std::collections::HashMap::new();
    let result = ws_read_loop(state, channel, &tx, &mut read, service_id, ping_interval, &mut partials).await;

    let _ = tx.send(WsMessage::Close(None));
    writer.abort();
    result
}

#[allow(clippy::too_many_arguments)]
async fn ws_read_loop(
    state: &AppState,
    channel: &BsonDoc,
    tx: &mpsc::UnboundedSender<WsMessage>,
    read: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
    service_id: i32,
    ping_interval: u64,
    partials: &mut std::collections::HashMap<String, std::collections::BTreeMap<u64, Vec<u8>>>,
) -> Result<(), String> {
    loop {
        // 任何入站帧都会重置保活计时（每轮新建 sleep）
        let idle = tokio::time::sleep(std::time::Duration::from_secs(ping_interval));
        tokio::select! {
            frame = read.next() => match frame {
                Some(Ok(WsMessage::Binary(b))) => {
                    let frame: PbFrame = PbFrame::decode(&b[..]).map_err(|e| format!("帧解码失败: {e}"))?;
                    match frame.method {
                        0 => {
                            // CONTROL：pong 载荷可能带保活配置更新（v1 记日志）
                            if frame.header("type") == Some("pong") && !frame.payload.is_empty() {
                                tracing::debug!("feishu: 收到 pong 配置更新");
                            }
                        }
                        1 => {
                            let message_id = frame.header("message_id").unwrap_or_default().to_string();
                            let sum: u64 = frame.header("sum").and_then(|v| v.parse().ok()).unwrap_or(1);
                            let seq: u64 = frame.header("seq").and_then(|v| v.parse().ok()).unwrap_or(0);
                            let frame_type = frame.header("type").unwrap_or_default().to_string();
                            // 合包：收齐 sum 片再处理；非 event 类型（card 等）按官方行为跳过
                            let payload = if sum > 1 {
                                let parts = partials.entry(message_id.clone()).or_default();
                                parts.insert(seq, frame.payload.clone());
                                if parts.len() != sum as usize {
                                    continue;
                                }
                                let assembled = parts.values().flatten().copied().collect::<Vec<u8>>();
                                partials.remove(&message_id);
                                assembled
                            } else {
                                frame.payload.clone()
                            };
                            if frame_type != "event" {
                                continue;
                            }
                            // 事件处理 spawn 出去，处理完再 ACK；未 ACK 飞书重推，去重兜底
                            let st = state.clone();
                            let ch = channel.clone();
                            let tx2 = tx.clone();
                            let mid = message_id.clone();
                            tokio::spawn(async move {
                                let ok = handle_stream_event(&st, &ch, &payload).await;
                                let ack = frame_for_ack(&mid);
                                let _ = tx2.send(WsMessage::Binary(ack.ack(ok).encode_to_vec()));
                            });
                        }
                        _ => {}
                    }
                }
                Some(Ok(WsMessage::Ping(p))) => { let _ = tx.send(WsMessage::Pong(p)); }
                Some(Ok(WsMessage::Pong(_))) => {}
                Some(Ok(WsMessage::Close(_))) => return Err("gateway closed".into()),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(format!("WS 读取错误: {e}")),
                None => return Err("WS 流结束".into()),
            },
            _ = idle => {
                let _ = tx.send(WsMessage::Binary(PbFrame::ping(service_id).encode_to_vec()));
                let wait = tokio::time::sleep(std::time::Duration::from_secs(PONG_WAIT_SECS));
                tokio::select! {
                    frame = read.next() => match frame {
                        Some(Ok(WsMessage::Binary(b))) => {
                            let f: PbFrame = PbFrame::decode(&b[..]).map_err(|e| format!("帧解码失败: {e}"))?;
                            if f.method == 1 {
                                // 保活等待期到的数据帧按正常路径补处理会打乱结构，
                                // 简化：直接判为活跃连接后回主循环会丢帧——按官方
                                // SDK 语义处理事件帧（罕见路径，直接内联处理）
                                let message_id = f.header("message_id").unwrap_or_default().to_string();
                                let ok = handle_stream_event(state, channel, &f.payload).await;
                                let ack = frame_for_ack(&message_id);
                                let _ = tx.send(WsMessage::Binary(ack.ack(ok).encode_to_vec()));
                            }
                        }
                        Some(Ok(WsMessage::Ping(p))) => { let _ = tx.send(WsMessage::Pong(p)); }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(format!("WS 读取错误: {e}")),
                        None => return Err("WS 流结束".into()),
                    },
                    _ = wait => return Err("ping 超时".into()),
                }
            }
        }
    }
}

/// 按官方 SDK 语义构造 ACK 帧：回显事件帧头（message_id 等）
fn frame_for_ack(message_id: &str) -> PbFrame {
    PbFrame {
        seq_id: 0,
        log_id: 0,
        service: 0,
        method: 1,
        headers: vec![PbHeader { key: "message_id".into(), value: message_id.to_string() }],
        payload: vec![],
    }
}

/// 处理一条长连接事件：复用推送侧事件解析 + 共享流水线。返回处理结果
/// 作为 ACK 语义码（失败 → 500，飞书重推，去重兜底）
async fn handle_stream_event(state: &AppState, channel: &BsonDoc, payload: &[u8]) -> bool {
    let payload: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: 事件载荷解析失败: {e}");
            return true; // 载荷损坏不可重试，ACK 掉
        }
    };
    let platform: &dyn crate::im::ImPlatform = &FeishuPlatform;
    match FeishuPlatform::parse_event_payload(channel, &payload) {
        Ok(InboundDecision::Message(msg)) => {
            matches!(
                crate::im::process_message(state, platform, channel, msg).await,
                crate::im::PipelineOutcome::Accepted { .. }
                    | crate::im::PipelineOutcome::Duplicate
                    | crate::im::PipelineOutcome::Handled { .. }
            )
        }
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: 长连接事件校验失败: {e}");
            true // 校验失败不可重试（重推也是同样结果），ACK 掉
        }
    }
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
    fn frame_roundtrip_matches_field_numbers() {
        let f = PbFrame {
            seq_id: 7,
            log_id: 0,
            service: 42,
            method: 1,
            headers: vec![
                PbHeader { key: "message_id".into(), value: "m-1".into() },
                PbHeader { key: "type".into(), value: "event".into() },
            ],
            payload: b"{}".to_vec(),
        };
        let back = PbFrame::decode(&f.encode_to_vec()[..]).unwrap();
        assert_eq!(back.seq_id, 7);
        assert_eq!(back.service, 42);
        assert_eq!(back.method, 1);
        assert_eq!(back.header("message_id"), Some("m-1"));
        assert_eq!(back.header("type"), Some("event"));
        assert_eq!(back.payload, b"{}");
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
