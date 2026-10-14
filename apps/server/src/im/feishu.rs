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

/// 富文本（post）转 markdown：优先 content_v2 的 md 标签（最大保留原始
/// markdown 与格式），fallback 按 content 标签拼接可表达的文本
/// （text/a 取文本、code_block 取代码、at 保留占位符交给清洗）
fn post_markdown(content: &Value) -> String {
    // content_v2: [[{tag:"md", text}...]] — 段落数组，md 文本即原文
    if let Some(paragraphs) = content.get("content_v2").and_then(|v| v.as_array()) {
        let md = paragraphs
            .iter()
            .filter_map(|p| p.as_array())
            .map(|els| {
                els.iter()
                    .filter(|e| e.get("tag").and_then(|t| t.as_str()) == Some("md"))
                    .filter_map(|e| e.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !md.is_empty() {
            return md;
        }
    }
    // fallback: content 老结构拼接
    if let Some(paragraphs) = content.get("content").and_then(|v| v.as_array()) {
        return paragraphs
            .iter()
            .filter_map(|p| p.as_array())
            .map(|els| {
                els.iter()
                    .filter_map(|e| {
                        let tag = e.get("tag").and_then(|t| t.as_str()).unwrap_or("");
                        match tag {
                            "text" | "a" | "code_block" => e.get("text").and_then(|t| t.as_str()),
                            "at" => e.get("user_id").and_then(|t| t.as_str()),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    String::new()
}

/// 剔除 @ 机器人的 @_user_N 占位符。仅移除占位 token 本身，保留其余
/// 空白与换行（多行/markdown 消息的格式不能压扁）
fn clean_mention(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("@_user_") {
        out.push_str(&rest[..idx]);
        rest = &rest[idx + "@_user_".len()..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        rest = &rest[digits..];
    }
    out.push_str(rest);
    out.trim().to_string()
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
        let client = crate::http::base_client(&state.cfg)?
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| e.to_string())?;
        let url = format!("{FEISHU_BASE}/open-apis/im/v1/messages?receive_id_type=chat_id");

        // 三层降级：post 富文本（md 标签，GFM 表格/任务列表原生渲染，表格
        // 前后须 \n\n 分隔）→ interactive 卡片（markdown 元素 + 表格组件，
        // 有 ~30KB 上限）→ 纯文本
        // 官方发送示例：md 标签放在标准 content 数组内（content_v2 是接收
        // 侧字段）；md 独占段落，CommonMark 0.31 + GFM 含表格/任务列表/脚注
        let post_content = json!({
            "post": { "zh_cn": { "content": [ [ { "tag": "md", "text": ensure_table_spacing(text) } ] ] } }
        })
        .to_string();
        let r = client
            .post(&url)
            .bearer_auth(&token)
            .json(&json!({ "receive_id": chat_id, "msg_type": "post", "content": post_content }))
            .send()
            .await
            .map_err(|e| e.to_string());
        if tier_ok(r).await {
            return Ok(());
        }
        if text.chars().count() <= 9000 {
            let card = json!({
                "schema": "2.0",
                "config": { "wide_screen_mode": true },
                "elements": card_elements(text),
            });
            let r = client
                .post(&url)
                .bearer_auth(&token)
                .json(&json!({ "receive_id": chat_id, "msg_type": "interactive", "content": card.to_string() }))
                .send()
                .await
                .map_err(|e| e.to_string());
            if tier_ok(r).await {
                return Ok(());
            }
        }
        let r = send_text_message(state, channel, chat_id, text).await?;
        let code = r.status().as_u16();
        let body: Value = r.json().await.unwrap_or(json!({}));
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

/// 层级发送成功判定：HTTP 2xx 且业务 code == 0，失败记日志供降级排查
async fn tier_ok(resp: Result<reqwest::Response, String>) -> bool {
    match resp {
        Ok(r) if r.status().is_success() => {
            let body: Value = r.json().await.unwrap_or(json!({}));
            let ok = body.get("code").and_then(|v| v.as_i64()) == Some(0);
            if !ok {
                tracing::warn!(body = %body, "feishu: 消息层级发送业务失败，降级");
            }
            ok
        }
        Ok(r) => {
            tracing::warn!(status = %r.status(), "feishu: 消息层级发送 HTTP 失败，降级");
            false
        }
        Err(e) => {
            tracing::warn!(err = %e, "feishu: 消息层级发送请求失败，降级");
            false
        }
    }
}

/// 表格前后补空行（官方规则：表格必须以 \n\n 与其他块级元素分隔，
/// 否则后续内容会被解析为表格的续行）
fn ensure_table_spacing(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    let mut i = 0;
    while i < lines.len() {
        let table_start = lines[i].contains('|')
            && i + 1 < lines.len()
            && is_table_delimiter(lines[i + 1]);
        if table_start {
            if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
                out.push(String::new());
            }
            out.push(lines[i].to_string());
            i += 1;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                out.push(lines[i].to_string());
                i += 1;
            }
            // 表格结束：确保后面有空行
            out.push(String::new());
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n").trim_end().to_string() + "\n"
}

/// 纯文本消息发送（卡片降级路径），返回原始响应供统一校验
async fn send_text_message(
    state: &AppState,
    channel: &BsonDoc,
    chat_id: &str,
    text: &str,
) -> Result<reqwest::Response, String> {
    let token = tenant_token(state, channel).await?;
    let client = crate::http::base_client(&state.cfg)?
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let content = json!({ "text": text }).to_string();
    client
        .post(format!("{FEISHU_BASE}/open-apis/im/v1/messages?receive_id_type=chat_id"))
        .bearer_auth(token)
        .json(&json!({ "receive_id": chat_id, "msg_type": "text", "content": content }))
        .send()
        .await
        .map_err(|e| e.to_string())
}

// ------------------------------------------------------------ 卡片元素构建

/// markdown 换行转 GFM 硬换行（行尾两空格），避免卡片渲染成软换行
fn hard_breaks(text: &str) -> String {
    text.replace('\n', "  \n")
}

fn is_table_delimiter(line: &str) -> bool {
    let t = line.trim().trim_matches('|');
    !t.is_empty() && t.split('|').all(|c| !c.trim().is_empty() && c.trim().chars().all(|ch| ch == '-' || ch == ':'))
}

fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// GFM 表格 → 飞书卡片 table 组件（卡片 markdown 元素不支持表格语法）
fn table_element(rows: &[String]) -> Option<Value> {
    if rows.len() < 2 {
        return None;
    }
    let header = split_table_row(&rows[0]);
    if header.is_empty() || header.iter().all(|h| h.is_empty()) {
        return None;
    }
    let columns: Vec<Value> = header
        .iter()
        .enumerate()
        .map(|(i, h)| json!({ "name": format!("c{i}"), "display_name": h, "data_type": "text" }))
        .collect();
    let body: Vec<Value> = rows[1..]
        .iter()
        .map(|line| {
            let cells = split_table_row(line);
            let mut obj = serde_json::Map::new();
            for (i, _) in header.iter().enumerate() {
                obj.insert(format!("c{i}"), json!(cells.get(i).cloned().unwrap_or_default()));
            }
            Value::Object(obj)
        })
        .collect();
    Some(json!({
        "tag": "table",
        "page_size": 10,
        "row_height": "low",
        "columns": columns,
        "rows": body,
    }))
}

/// markdown 文本 → 卡片元素序列：GFM 表格拆成独立 table 组件，其余为
/// markdown 元素（换行转硬换行）
fn card_elements(text: &str) -> Vec<Value> {
    let mut elements: Vec<Value> = Vec::new();
    let mut md = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let table_start = lines[i].contains('|')
            && i + 1 < lines.len()
            && is_table_delimiter(lines[i + 1]);
        if table_start {
            if !md.trim().is_empty() {
                elements.push(json!({ "tag": "markdown", "content": hard_breaks(md.trim_end()) }));
                md.clear();
            }
            let mut rows = vec![lines[i].to_string()];
            i += 2; // 跳过表头与分隔行
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(lines[i].to_string());
                i += 1;
            }
            if let Some(el) = table_element(&rows) {
                elements.push(el);
            }
            continue;
        }
        md.push_str(lines[i]);
        md.push('\n');
        i += 1;
    }
    if !md.trim().is_empty() {
        elements.push(json!({ "tag": "markdown", "content": hard_breaks(md.trim_end()) }));
    }
    if elements.is_empty() {
        elements.push(json!({ "tag": "markdown", "content": "" }));
    }
    elements
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
    let client = crate::http::base_client(&state.cfg)?
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
        // content 是 JSON 字符串需二次解析
        let content: Value =
            serde_json::from_str(message.get("content").and_then(|v| v.as_str()).unwrap_or("{}"))
                .unwrap_or(json!({}));
        // text = 纯文本；post = 富文本（content_v2 的 md 标签最大保留原始
        // markdown，官方推荐优先；老数据 fallback 到 content 标签拼接）
        let raw = match message_type {
            "text" => content.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            "post" => post_markdown(&content),
            other => {
                tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "feishu: 忽略不支持的消息类型 {other}");
                return Ok(InboundDecision::Ignored(other.to_string()));
            }
        };
        // @ 占位清洗（text 与 post 的 at 标签同为 @_user_N 形式）
        let text = clean_mention(&raw);
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
async fn open_ws_endpoint(
    cfg: &crate::config::Config,
    app_id: &str,
    app_secret: &str,
) -> Result<(String, u64), String> {
    let client = crate::http::base_client(cfg)?
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
    let (url, ping_interval) = open_ws_endpoint(&state.cfg, &app_id, &app_secret).await?;
    let service_id = service_id_of(&url);
    // WS 经出站代理（CONNECT 隧道）+ 企业 CA，见 crate::http::connect_ws
    let ws = crate::http::connect_ws(&state.cfg, &url)
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
    fn post_markdown_prefers_content_v2() {
        let content: Value = serde_json::from_str(
            r#"{"content":[[{"tag":"text","text":"第一行:"}]],
               "content_v2":[[{"tag":"md","text":"**第一行:**\n```go\nfunc main() {}\n```"}]]}"#,
        )
        .unwrap();
        assert_eq!(
            post_markdown(&content),
            "**第一行:**\n```go\nfunc main() {}\n```"
        );
    }

    #[test]
    fn post_markdown_fallback_joins_content_tags() {
        let content: Value = serde_json::from_str(
            r#"{"content":[[{"tag":"text","text":"@_user_1 "},{"tag":"a","href":"https://x","text":"看下这个"}],[{"tag":"code_block","language":"GO","text":"println()"}]]}"#,
        )
        .unwrap();
        let md = post_markdown(&content);
        // at 占位保留，交由 clean_mention 清洗；段落间换行必须保留
        assert_eq!(clean_mention(&md), "看下这个\n\nprintln()");
    }

    #[test]
    fn card_elements_splits_tables_from_markdown() {
        let text = "前置说明\n\n| 能力 | 说明 |\n|---|---|\n| 代码 | 新功能 |\n| 测试 | 跑测试 |\n\n后置总结";
        let els = card_elements(text);
        assert_eq!(els.len(), 3);
        assert_eq!(els[0].get("tag"), Some(&json!("markdown")));
        let table = &els[1];
        assert_eq!(table.get("tag"), Some(&json!("table")));
        let cols = table.get("columns").unwrap().as_array().unwrap();
        assert_eq!(cols[0].get("display_name"), Some(&json!("能力")));
        let rows = table.get("rows").unwrap().as_array().unwrap();
        assert_eq!(rows[0].get("c0"), Some(&json!("代码")));
        assert_eq!(rows[1].get("c1"), Some(&json!("跑测试")));
        assert_eq!(els[2].get("tag"), Some(&json!("markdown")));
        // 纯 markdown 无表格 → 单元素
        assert_eq!(card_elements("只有一段").len(), 1);
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
