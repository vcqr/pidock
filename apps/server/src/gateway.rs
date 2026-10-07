//! Gateway WebSockets.
//!
//! /ws/desktop — PiDock desktop sync-agent: upstream message-level envelopes
//! (pushed into the pipeline), downstream user commands. Connection registers
//! the machine online (Redis + Mongo), heartbeat keeps the TTL alive.
//!
//! /ws/web — browser clients: downstream realtime push (Redis pub/sub of the
//! user's channel), upstream nothing in v1 (commands go via POST /commands).

use axum::{
    extract::{Query, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::auth::verify_token;
use crate::state::AppState;

fn query_token(params: &HashMap<String, String>) -> Result<String, String> {
    params
        .get("token")
        .cloned()
        .ok_or_else(|| "missing token".to_string())
}

// ---------------------------------------------------------------- desktop ---

pub async fn ws_desktop(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let token = match query_token(&params) {
        Ok(t) => t,
        Err(e) => return IntoResponse::into_response((axum::http::StatusCode::UNAUTHORIZED, e)),
    };
    let claims = match verify_token(&state, &token, "access").await {
        Ok(c) => c,
        Err(e) => return IntoResponse::into_response((axum::http::StatusCode::UNAUTHORIZED, e)),
    };
    upgrade
        .on_upgrade(move |socket| desktop_socket(state, claims.sub, socket))
        .into_response()
}

async fn desktop_socket(state: AppState, user_id: String, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let mut machine_id: Option<String> = None;

    // forward downstream frames (commands) to the websocket
    let forward = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if ws_tx.send(Message::Text(frame.to_string().into())).await.is_err() {
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
                    crate::state::MachineRoute { user_id: user_id.clone(), tx: out_tx.clone() },
                );
                let _ = out_tx.send(json!({"ctrl":"registered","machine_id": mid}));
                machine_id = Some(mid);
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
    let token = match query_token(&params) {
        Ok(t) => t,
        Err(e) => return IntoResponse::into_response((axum::http::StatusCode::UNAUTHORIZED, e)),
    };
    let claims = match verify_token(&state, &token, "access").await {
        Ok(c) => c,
        Err(e) => return IntoResponse::into_response((axum::http::StatusCode::UNAUTHORIZED, e)),
    };
    upgrade
        .on_upgrade(move |socket| web_socket(state, claims.sub, socket))
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
    if pubsub.subscribe(AppState::user_events_channel(&user_id)).await.is_err() {
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
