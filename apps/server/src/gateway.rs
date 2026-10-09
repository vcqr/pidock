//! Gateway WebSockets.
//!
//! /ws/desktop — PiDock desktop sync-agent: upstream message-level envelopes
//! (pushed into the pipeline), downstream user commands. Connection registers
//! the machine online (Redis + Mongo); the gateway renews the TTL itself
//! (desktop clients never send heartbeat frames).
//!
//! /ws/web — browser clients: downstream realtime push (Redis pub/sub of the
//! user's channel), upstream nothing in v1 (commands go via POST /commands).

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::auth::{user_active, verify_token};
use crate::state::AppState;

fn query_token(params: &HashMap<String, String>) -> Result<String, String> {
    params
        .get("token")
        .cloned()
        .ok_or_else(|| "missing token".to_string())
}

/// token 有效之外还要账号在册且未禁用（禁用立即断掉新连接）
async fn gate(state: &AppState, token: &str) -> Result<String, (axum::http::StatusCode, String)> {
    let claims = verify_token(state, token, "access")
        .await
        .map_err(|e| (axum::http::StatusCode::UNAUTHORIZED, e))?;
    if !user_active(state, &claims.sub).await {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "账号已被禁用".to_string(),
        ));
    }
    Ok(claims.sub)
}

/// WS 鉴权：`?ticket=`（/auth/ws-ticket 签发的一次性票据，60 秒有效）优先，
/// 回落 `?token=`（旧桌面端兼容，逐步淘汰——token 会落反代访问日志）。返回 user_id
async fn ws_auth_user(
    state: &AppState,
    params: &HashMap<String, String>,
) -> Result<String, (axum::http::StatusCode, String)> {
    if let Some(t) = params.get("ticket").filter(|t| !t.trim().is_empty()) {
        let mut conn = state.redis.clone();
        let uid: Option<String> = redis::cmd("GETDEL")
            .arg(format!("ws:ticket:{}", t.trim()))
            .query_async(&mut conn)
            .await
            .unwrap_or(None);
        return match uid {
            Some(uid) if user_active(state, &uid).await => Ok(uid),
            Some(_) => Err((axum::http::StatusCode::FORBIDDEN, "账号已被禁用".into())),
            None => Err((
                axum::http::StatusCode::UNAUTHORIZED,
                "ticket 已使用或过期".into(),
            )),
        };
    }
    let token = query_token(params).map_err(|e| (axum::http::StatusCode::UNAUTHORIZED, e))?;
    gate(state, &token).await
}

// ---------------------------------------------------------------- desktop ---

pub async fn ws_desktop(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let user_id = match ws_auth_user(&state, &params).await {
        Ok(u) => u,
        Err((s, e)) => return IntoResponse::into_response((s, e)),
    };
    upgrade
        .on_upgrade(move |socket| desktop_socket(state, user_id, socket))
        .into_response()
}

async fn desktop_socket(state: AppState, user_id: String, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let mut machine_id: Option<String> = None;
    // 在线 TTL 由服务端周期续期：桌面端从不发 heartbeat 帧，
    // 若只靠客户端心跳，任何连接都会在 90s TTL 过期后被误判离线
    let mut keepalive: Option<tokio::task::JoinHandle<()>> = None;

    // forward downstream frames (commands) to the websocket
    let forward = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if ws_tx
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    tracing::info!(user = %user_id, "desktop connected");
    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let frame: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad desktop frame: {e}");
                continue;
            }
        };
        match frame.get("ctrl").and_then(|v| v.as_str()) {
            Some("register") => {
                let mid = frame
                    .get("machine_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(crate::auth::random_suffix);
                if let Err(e) = state.machine_online(&user_id, &mid, &frame).await {
                    tracing::error!("machine_online failed: {e}");
                }
                state.routes.lock().await.insert(
                    mid.clone(),
                    crate::state::MachineRoute {
                        user_id: user_id.clone(),
                        tx: out_tx.clone(),
                    },
                );
                let _ = out_tx.send(json!({"ctrl":"registered","machine_id": mid}));
                machine_id = Some(mid.clone());
                // 每 30s 续 90s TTL（TCP 活着即在线；断开走 machine_offline 立即下线）
                if let Some(prev) = keepalive.take() {
                    prev.abort();
                }
                let hb_state = state.clone();
                let hb_mid = mid;
                keepalive = Some(tokio::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        tick.tick().await;
                        hb_state.machine_heartbeat(&hb_mid).await;
                    }
                }));
            }
            Some("heartbeat") => {
                if let Some(mid) = &machine_id {
                    state.machine_heartbeat(mid).await;
                }
            }
            _ => {
                // envelope: stamp user_id + machine_id, push into pipeline
                let mid = machine_id.clone().unwrap_or_default();
                let wrapped = crate::pipeline::wrap(&user_id, &mid, &frame);
                if let Err(e) = state.sink.publish(&mid, &wrapped.to_string()).await {
                    tracing::error!("pipeline publish failed: {e}");
                }
            }
        }
    }

    forward.abort();
    if let Some(hb) = keepalive.take() {
        hb.abort();
    }
    if let Some(mid) = &machine_id {
        state.routes.lock().await.remove(mid);
        state.machine_offline(&user_id, mid).await;
    }
    tracing::info!(user = %user_id, "desktop disconnected");
}

// -------------------------------------------------------------------- web ---

pub async fn ws_web(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let user_id = match ws_auth_user(&state, &params).await {
        Ok(u) => u,
        Err((s, e)) => return IntoResponse::into_response((s, e)),
    };
    upgrade
        .on_upgrade(move |socket| web_socket(state, user_id, socket))
        .into_response()
}

async fn web_socket(state: AppState, user_id: String, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // subscribe to the user's realtime channel
    let mut pubsub = match state.redis_client.get_async_pubsub().await {
        Ok(ps) => ps,
        Err(e) => {
            tracing::error!("pubsub connect failed: {e}");
            return;
        }
    };
    if pubsub
        .subscribe(AppState::user_events_channel(&user_id))
        .await
        .is_err()
    {
        return;
    }
    let mut stream = pubsub.into_on_message();
    let push = tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let payload: String = match msg.get_payload() {
                Ok(p) => p,
                Err(_) => break,
            };
            if ws_tx.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    });

    // web upstream is reserved for future control frames; drain to detect close
    while let Some(Ok(msg)) = ws_rx.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
    }
    push.abort();
}

/// route a command from the web to a connected desktop machine.
/// The machine must belong to the requesting user: routes carry the owner
/// stamped at register time, so cross-user command injection is refused here.
pub async fn route_command(
    state: &AppState,
    user_id: &str,
    machine_id: &str,
    command: &Value,
) -> Result<String, String> {
    let routes = state.routes.lock().await;
    let route = routes.get(machine_id).ok_or("refused_offline")?;
    if route.user_id != user_id {
        tracing::warn!(user = %user_id, machine = %machine_id, "command refused: not machine owner");
        return Err("forbidden".to_string());
    }
    let command_id = uuid::Uuid::now_v7().to_string();
    let frame = json!({
        "ctrl": "command",
        "command_id": command_id,
        "user_id": user_id,
        "command": command,
    });
    route
        .tx
        .send(frame)
        .map_err(|_| "refused_offline".to_string())?;
    Ok(command_id)
}
