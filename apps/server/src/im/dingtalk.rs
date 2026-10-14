//! 钉钉适配器（Stream 模式，官方协议按 open-dingtalk/dingtalk-stream-sdk-go 对齐）。
//!
//! 与飞书的 HTTP 推送不同，Stream 模式**不需要公网回调地址**：服务端作 WS
//! 客户端主动连接钉钉网关。流程：
//! 1. `POST /v1.0/gateway/connections/open`（clientId/clientSecret + 订阅列表）
//!    换 `endpoint` + `ticket`；
//! 2. WSS 连 `{endpoint}?ticket={ticket}`；
//! 3. 下行帧 `{specVersion, type(SYSTEM|CALLBACK|EVENT), headers{topic,
//!    messageId,...}, data(JSON 字符串)}`，按帧回 ACK `{code:200,
//!    headers{messageId}}`；SYSTEM/ping 回 pong（data 原样回显），SYSTEM/
//!    disconnect 先 ACK 后断开由外层重连；
//! 4. 机器人消息 topic `/v1.0/im/bot/messages/get`：data 为
//!    BotCallbackData（msgId/text.content/sessionWebhook...），业务处理
//!    **spawn 出去**（不阻塞读循环与心跳），处理完再 ACK，未 ACK 钉钉会重推；
//! 5. 保活：空闲 60s 发 WS Ping，5s 内无任何入站即判死重连。
//!
//! 回复直接 POST 消息载荷里的 `sessionWebhook`（临时回调地址，免 access_token），
//! 过期时间通常远大于回调等待上限（30min）。

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use mongodb::bson::{doc, Document as BsonDoc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::im::{ChannelBody, InboundDecision, InboundMessage, PipelineOutcome};
use crate::state::AppState;

const GATEWAY_OPEN: &str = "https://api.dingtalk.com/v1.0/gateway/connections/open";
/// 机器人消息（单聊 + 群聊 @机器人）
const TOPIC_BOT_MESSAGE: &str = "/v1.0/im/bot/messages/get";
const SUB_SYSTEM: &str = "SYSTEM";
const FRAME_CALLBACK: &str = "CALLBACK";
const TOPIC_PING: &str = "ping";
const TOPIC_DISCONNECT: &str = "disconnect";
/// ACK 语义码（与 SDK DataFrameResponseStatusCode 对齐）
const ACK_OK: i32 = 200;
const ACK_HANDLER_NOT_FOUND: i32 = 404;
const ACK_INTERNAL_ERROR: i32 = 500;
/// 保活节奏（对齐 SDK 默认 120s 空闲 + 5s pong 等待；这里取 60s 更保守）
const KEEPALIVE_IDLE_SECS: u64 = 60;
const PONG_WAIT_SECS: u64 = 5;
/// HTTP 调用超时
const HTTP_TIMEOUT_SECS: u64 = 10;

pub(crate) struct DingTalkPlatform;

// ------------------------------------------------------------ 线路结构

#[derive(Deserialize)]
struct DataFrame {
    #[serde(rename = "type", default)]
    frame_type: String,
    #[serde(default)]
    headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    data: String,
}

/// 机器人消息载荷（data 字段内的 JSON 字符串）
#[derive(Deserialize, Default)]
struct BotCallbackData {
    #[serde(default, rename = "msgId")]
    msg_id: String,
    #[serde(default, rename = "conversationId")]
    conversation_id: String,
    #[serde(default, rename = "robotCode")]
    robot_code: String,
    #[serde(default)]
    text: TextPart,
    #[serde(default, rename = "sessionWebhook")]
    session_webhook: String,
}

#[derive(Deserialize, Default)]
struct TextPart {
    #[serde(default)]
    content: String,
}

fn ack_json(code: i32, message_id: &str, message: &str, data: &str) -> WsMessage {
    WsMessage::Text(
        json!({
            "code": code,
            "headers": {
                "contentType": "application/json",
                "messageId": message_id,
            },
            "message": message,
            "data": data,
        })
        .to_string(),
    )
}

// ------------------------------------------------------------ 连接管理

/// 换取连接端点：POST connections/open
async fn open_endpoint(cfg: &crate::config::Config, channel: &BsonDoc) -> Result<(String, String), String> {
    let client = crate::http::base_client(cfg)?
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(GATEWAY_OPEN)
        .json(&json!({
            "clientId": channel.get_str("app_id").unwrap_or_default(),
            "clientSecret": channel.get_str("app_secret").unwrap_or_default(),
            "subscriptions": [
                { "type": SUB_SYSTEM, "topic": TOPIC_PING },
                { "type": SUB_SYSTEM, "topic": TOPIC_DISCONNECT },
                { "type": "CALLBACK", "topic": TOPIC_BOT_MESSAGE },
            ],
            "ua": "pidock-stream/0.1",
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() {
        return Err(format!(
            "获取连接端点失败 HTTP {status}: {}",
            body.get("message").and_then(|v| v.as_str()).unwrap_or("")
        ));
    }
    let endpoint = body
        .get("endpoint")
        .and_then(|v| v.as_str())
        .ok_or("网关响应缺少 endpoint")?
        .to_string();
    let ticket = body
        .get("ticket")
        .and_then(|v| v.as_str())
        .ok_or("网关响应缺少 ticket")?
        .to_string();
    Ok((endpoint, ticket))
}

/// 一次连接的完整生命周期：断开即返回，由 im::stream_conn_loop 负责重连
async fn run_connection(state: &AppState, channel: &BsonDoc) -> Result<(), String> {
    let (endpoint, ticket) = open_endpoint(&state.cfg, channel).await?;
    let url = format!("{endpoint}?ticket={ticket}");
    // WS 经出站代理（CONNECT 隧道）+ 企业 CA，见 crate::http::connect_ws
    let ws = crate::http::connect_ws(&state.cfg, &url)
        .await
        .map_err(|e| format!("WS 连接失败: {e}"))?;
    tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "dingtalk: 流连接已建立");
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if write.send(m).await.is_err() {
                break;
            }
        }
    });

    let result = read_loop(state, channel, &tx, &mut read).await;

    let _ = tx.send(WsMessage::Close(None));
    writer.abort();
    result
}

async fn read_loop(
    state: &AppState,
    channel: &BsonDoc,
    tx: &mpsc::UnboundedSender<WsMessage>,
    read: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> Result<(), String> {
    // 首个 tick 立即触发，先吞掉
    let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(KEEPALIVE_IDLE_SECS));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await;

    loop {
        let idle = tokio::time::sleep(std::time::Duration::from_secs(KEEPALIVE_IDLE_SECS));
        tokio::select! {
            frame = read.next() => match frame {
                Some(Ok(WsMessage::Text(t))) => handle_frame(state, channel, tx, &t).await?,
                Some(Ok(WsMessage::Ping(p))) => { let _ = tx.send(WsMessage::Pong(p)); }
                Some(Ok(WsMessage::Pong(_))) => {}
                Some(Ok(WsMessage::Close(_))) => return Err("gateway closed".into()),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(format!("WS 读取错误: {e}")),
                None => return Err("WS 流结束".into()),
            },
            _ = idle => {
                // 与 Go SDK 一致：Ping 后 5s 内无任何入站即判连接死亡
                let _ = tx.send(WsMessage::Ping(vec![]));
                let wait = tokio::time::sleep(std::time::Duration::from_secs(PONG_WAIT_SECS));
                tokio::select! {
                    frame = read.next() => match frame {
                        Some(Ok(WsMessage::Text(t))) => handle_frame(state, channel, tx, &t).await?,
                        Some(Ok(WsMessage::Ping(p))) => { let _ = tx.send(WsMessage::Pong(p)); }
                        Some(Ok(WsMessage::Pong(_))) => {}
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

/// 处理一个下行业务帧并回 ACK。Err = 连接应断开（disconnect / 协议错误）
async fn handle_frame(
    state: &AppState,
    channel: &BsonDoc,
    tx: &mpsc::UnboundedSender<WsMessage>,
    text: &str,
) -> Result<(), String> {
    let df: DataFrame = serde_json::from_str(text).map_err(|e| format!("帧解析失败: {e}"))?;
    let topic = df.headers.get("topic").cloned().unwrap_or_default();
    let message_id = df.headers.get("messageId").cloned().unwrap_or_default();

    match (df.frame_type.as_str(), topic.as_str()) {
        // 系统 ping：pong 回显 data
        (SUB_SYSTEM, TOPIC_PING) => {
            let _ = tx.send(ack_json(ACK_OK, &message_id, "ok", &df.data));
        }
        // 网关指令断开：先 ACK 再退出，外层循环重连
        (SUB_SYSTEM, TOPIC_DISCONNECT) => {
            let _ = tx.send(ack_json(ACK_OK, &message_id, "ok", ""));
            return Err("gateway disconnect".into());
        }
        (FRAME_CALLBACK, t) if t == TOPIC_BOT_MESSAGE => {
            // 业务处理 spawn 出去：deliver 可能耗时数十秒，绝不能阻塞读循环
            // 与心跳；处理完再 ACK，未 ACK 期间钉钉会重推（去重兜底）
            let st = state.clone();
            let ch = channel.clone();
            let tx2 = tx.clone();
            let data = df.data.clone();
            let mid = message_id.clone();
            tokio::spawn(async move {
                let outcome = on_bot_message(&st, &ch, &data).await;
                let code = match &outcome {
                    Some(PipelineOutcome::Failed { .. }) => ACK_INTERNAL_ERROR,
                    _ => ACK_OK,
                };
                let _ = tx2.send(ack_json(code, &mid, "ok", ""));
            });
        }
        _ => {
            let _ = tx.send(ack_json(ACK_HANDLER_NOT_FOUND, &message_id, "handler not registered", ""));
        }
    }
    Ok(())
}

/// 机器人消息 → 共享流水线。载荷非法返回 None（ACK 200 跳过，不可重试的错误
/// 不应触发钉钉重推）
async fn on_bot_message(state: &AppState, channel: &BsonDoc, data: &str) -> Option<PipelineOutcome> {
    let payload: BotCallbackData = serde_json::from_str(data).unwrap_or_default();
    let text = payload.text.content.trim().to_string();
    if payload.msg_id.is_empty() || payload.session_webhook.is_empty() || text.is_empty() {
        tracing::info!(channel = %channel.get_str("_id").unwrap_or_default(), "dingtalk: 忽略空/非文本消息");
        return None;
    }
    let platform: &dyn crate::im::ImPlatform = &DingTalkPlatform;
    Some(
        crate::im::process_message(
            state,
            platform,
            channel,
            InboundMessage {
                message_id: payload.msg_id,
                text,
                // conversationId 是会话内稳定标识（sessionWebhook 是临时的，
                // 不能做 auto 策略的绑定键）
                chat_key: payload.conversation_id.clone(),
                reply_ctx: json!({
                    "session_webhook": payload.session_webhook,
                    "conversation_id": payload.conversation_id,
                    // robotCode ≠ AppKey（以载荷为准），主动推送群消息必需
                    "robot_code": payload.robot_code,
                }),
            },
        )
        .await,
    )
}

// ------------------------------------------------------------ 主动推送

/// sessionWebhook 发文本（临时地址，返回 {errcode: 0}）
/// 钉钉 markdown 预处理：换行要求行尾两个空格（裸 \n 会塌成一段）；
/// 语法不支持代码块，反引号原样显示反而碍眼，直接剥掉
fn dingtalk_md(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\n', "  \n")
        .replace('`', "")
}

async fn webhook_post(state: &AppState, url: &str, text: &str) -> Result<(), String> {
    let client = crate::http::base_client(&state.cfg)?
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(url)
        .json(&json!({ "msgtype": "markdown", "markdown": { "title": "PiDock", "text": dingtalk_md(text) } }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if code / 100 == 2 && body.get("errcode").and_then(|v| v.as_i64()) == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "钉钉 sessionWebhook 回复失败 HTTP {code}: {}",
            body.get("errmsg").and_then(|v| v.as_str()).unwrap_or("")
        ))
    }
}

/// 企业内部应用 access_token：AppKey/AppSecret 换取，Redis 缓存到过期前 5 分钟
async fn access_token(state: &AppState, channel: &BsonDoc) -> Result<String, String> {
    let channel_id = channel.get_str("_id").unwrap_or_default().to_string();
    let cache_key = format!("im:dingtalk:token:{channel_id}");
    let mut conn = state.redis.clone();
    use redis::AsyncCommands;
    if let Ok(Some(cached)) = conn.get::<_, Option<String>>(&cache_key).await {
        if !cached.is_empty() {
            return Ok(cached);
        }
    }
    let client = crate::http::base_client(&state.cfg)?
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
        .json(&json!({
            "appKey": channel.get_str("app_id").unwrap_or_default(),
            "appSecret": channel.get_str("app_secret").unwrap_or_default(),
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    let token = body
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "获取 access_token 失败: {}",
                body.get("message").and_then(|v| v.as_str()).unwrap_or("")
            )
        })?
        .to_string();
    let expire = body.get("expireIn").and_then(|v| v.as_i64()).unwrap_or(7200);
    let ttl = expire.saturating_sub(300).clamp(60, 7200) as u64;
    let _: Result<(), _> = conn.set_ex(&cache_key, &token, ttl).await;
    Ok(token)
}

/// 机器人群消息主动发送（不依赖临时 sessionWebhook）
async fn group_send_text(
    state: &AppState,
    channel: &BsonDoc,
    conversation_id: &str,
    robot_code: &str,
    text: &str,
) -> Result<(), String> {
    let token = access_token(state, channel).await?;
    let client = crate::http::base_client(&state.cfg)?
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post("https://api.dingtalk.com/v1.0/robot/groupMessages/send")
        .header("x-acs-dingtalk-access-token", token)
        .json(&json!({
            "openConversationId": conversation_id,
            "robotCode": robot_code,
            "msgKey": "sampleMarkdown",
            "msgParam": json!({ "title": "PiDock", "content": dingtalk_md(text) }).to_string(),
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if code / 100 == 2 {
        Ok(())
    } else {
        let body: Value = resp.json().await.unwrap_or(json!({}));
        Err(format!(
            "钉钉群消息发送失败 HTTP {code}: {}",
            body.get("message").and_then(|v| v.as_str()).unwrap_or("")
        ))
    }
}

// ------------------------------------------------------------ 适配器实现

#[async_trait]
impl crate::im::ImPlatform for DingTalkPlatform {
    fn kind(&self) -> &'static str {
        "dingtalk"
    }

    fn validate_channel(&self, _body: &ChannelBody) -> Result<(), String> {
        // Stream 模式凭 clientId/clientSecret 建连，无 verification_token/encrypt_key
        Ok(())
    }

    fn http_inbound(&self) -> bool {
        false
    }

    async fn parse_inbound(
        &self,
        _state: &AppState,
        _channel: &BsonDoc,
        _headers: &axum::http::HeaderMap,
        _body: &str,
    ) -> Result<InboundDecision, String> {
        Err("钉钉为流模式，无 HTTP 入站".into())
    }

    async fn reply_text(
        &self,
        state: &AppState,
        channel: &BsonDoc,
        reply_ctx: &Value,
        text: &str,
    ) -> Result<(), String> {
        // sessionWebhook 优先：临时地址但随每条 IM 消息刷新，实测最可靠；
        // 群消息 API（robot/groupMessages/send）作兜底——需要应用在钉钉后台
        // 开通「机器人发送群消息」权限，未开通时报 resource.not.found/robot 不存在
        let mut last_err = String::from("reply_ctx 缺少回复定位");
        if let Some(url) = reply_ctx
            .get("session_webhook")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            match webhook_post(state, url, text).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // webhook 过期等场景，继续尝试群消息 API
                    last_err = e;
                }
            }
        }
        let conversation_id = reply_ctx
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .or_else(|| reply_ctx.get("chat_key").and_then(|v| v.as_str()))
            .unwrap_or_default();
        let robot_code = reply_ctx
            .get("robot_code")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| channel.get_str("app_id").unwrap_or_default());
        if !conversation_id.is_empty() && !robot_code.is_empty() {
            match group_send_text(state, channel, conversation_id, robot_code, text).await {
                Ok(()) => return Ok(()),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn uses_stream(&self) -> bool {
        true
    }

    async fn stream_connection(&self, state: &AppState, channel: &BsonDoc) -> Result<(), String> {
        run_connection(state, channel).await
    }
}

// ------------------------------------------------------------ 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_payload_parses_official_shape() {
        let raw = r#"{
            "conversationId": "cidXxx==",
            "atUsers": [{"dingtalkId": "$:LWCP_v1:$xxx", "staffId": "u1"}],
            "msgId": "msgX9xx",
            "senderNick": "张三",
            "senderStaffId": "u1",
            "sessionWebhookExpiredTime": 1700000000000,
            "conversationType": "2",
            "isInAtList": true,
            "sessionWebhook": "https://oapi.dingtalk.com/robot/send?sessionWebhook=xxx",
            "text": {"content": " 帮我看看构建日志 "},
            "msgtype": "text"
        }"#;
        let d: BotCallbackData = serde_json::from_str(raw).unwrap();
        assert_eq!(d.msg_id, "msgX9xx");
        assert_eq!(d.conversation_id, "cidXxx==");
        assert_eq!(d.text.content.trim(), "帮我看看构建日志");
        assert!(d.session_webhook.starts_with("https://"));
    }

    #[test]
    fn data_frame_parses_topic_and_message_id() {
        let raw = r#"{
            "specVersion": "1.0",
            "type": "CALLBACK",
            "headers": {
                "topic": "/v1.0/im/bot/messages/get",
                "messageId": "mid-1",
                "eventId": "evt-1",
                "contentType": "application/json"
            },
            "data": "{\"msgtype\":\"text\"}"
        }"#;
        let df: DataFrame = serde_json::from_str(raw).unwrap();
        assert_eq!(df.frame_type, "CALLBACK");
        assert_eq!(df.headers.get("topic").map(String::as_str), Some(TOPIC_BOT_MESSAGE));
        assert_eq!(df.headers.get("messageId").map(String::as_str), Some("mid-1"));
    }

    #[test]
    fn ack_frame_shape_matches_sdk() {
        let ack = ack_json(ACK_OK, "mid-1", "ok", "echo");
        let v: Value = serde_json::from_str(match &ack {
            WsMessage::Text(s) => s.as_str(),
            _ => panic!(),
        })
        .unwrap();
        assert_eq!(v.get("code").and_then(|x| x.as_i64()), Some(200));
        assert_eq!(
            v.pointer("/headers/messageId").and_then(|x| x.as_str()),
            Some("mid-1")
        );
        assert_eq!(v.get("data").and_then(|x| x.as_str()), Some("echo"));
    }
}
