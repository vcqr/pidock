//! Desktop sync-agent: WebSocket uplink to the PiDock server.
//!
//! - upstream: host events (filtered: persist + live-state kinds) and
//!   command_result envelopes
//! - downstream: user commands -> host requests -> result back as
//!   command_result
//! - lifecycle: login (HTTP) -> WS connect with JWT -> register machine ->
//!   heartbeat 30s -> backfill open sessions on (re)connect -> reconnect with
//!   backoff

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::host::Supervisor;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyncConfig {
    pub server_url: String,
    pub email: String,
    pub refresh_token: String,
    pub machine_id: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyncStatus {
    pub enabled: bool,
    pub connected: bool,
    pub machine_id: String,
    pub error: Option<String>,
}

pub enum SyncControl {
    Restart,
    Stop,
}

pub struct SyncManager {
    pub cfg_path: std::path::PathBuf,
    pub cfg: Mutex<SyncConfig>,
    pub status: Arc<Mutex<SyncStatus>>,
    pub control: mpsc::Sender<SyncControl>,
}

impl SyncManager {
    pub fn new(cfg_path: std::path::PathBuf, control: mpsc::Sender<SyncControl>) -> Self {
        let cfg = std::fs::read(&cfg_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SyncConfig>(&bytes).ok())
            .unwrap_or_default();
        Self {
            cfg_path,
            cfg: Mutex::new(cfg),
            status: Arc::new(Mutex::new(SyncStatus::default())),
            control,
        }
    }

    fn save(&self, cfg: &SyncConfig) {
        if let Some(parent) = self.cfg_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(cfg) {
            let _ = std::fs::write(&self.cfg_path, bytes);
        }
    }

    pub async fn set_status(&self, patch: SyncStatus) {
        let mut st = self.status.lock().await;
        if patch.connected != st.connected {
            st.connected = patch.connected;
        }
        st.machine_id = patch.machine_id;
        st.error = patch.error;
        st.enabled = patch.enabled;
    }
}

async fn http_json(method: &str, url: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let req = match method {
        "POST" => client.post(url),
        _ => client.get(url),
    };
    let resp = req
        .json(body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("{method} {url} failed: {e}"))?;
    let status = resp.status();
    let data: Value = resp.json().await.map_err(|e| format!("bad json from {url}: {e}"))?;
    if !status.is_success() {
        let msg = data.get("error").and_then(|v| v.as_str()).unwrap_or("request failed");
        return Err(format!("{status}: {msg}"));
    }
    Ok(data)
}

/// refresh the access token; returns (access, new_refresh) — the server
/// ROTATES refresh tokens on every call, so the new one must be persisted
async fn refresh_access(cfg: &SyncConfig) -> Result<(String, String), String> {
    let data = http_json(
        "POST",
        &format!("{}/auth/refresh", cfg.server_url.trim_end_matches('/')),
        &json!({"refresh_token": cfg.refresh_token}),
    )
    .await?;
    let access = data
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("no access_token in refresh response")?
        .to_string();
    let new_refresh = data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("no refresh_token in refresh response")?
        .to_string();
    Ok((access, new_refresh))
}

/// kinds that get mirrored to the cloud (persist events are always mirrored)
fn cloud_worthy(kind: &str) -> bool {
    matches!(
        kind,
        "message_snapshot"
            | "session_meta"
            | "agent_state_changed"
            | "tool_execution_start"
            | "tool_execution_end"
            | "tool_approval"
            | "ask_user_question"
            | "auto_retry"
            | "compaction_lifecycle"
            | "todo_updated"
            | "command_result"
            | "error"
    )
}

/// collect content-addressed attachment refs from an arbitrary payload
fn collect_attachments(value: &Value, out: &mut Vec<(String, i64)>) {
    match value {
        Value::Object(map) => {
            let sha = map.get("attachment_id").and_then(|v| v.as_str());
            let truncated = map.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false);
            if let (Some(sha), Some(_)) = (sha, map.get("size")) {
                if truncated && sha.len() == 64 {
                    out.push((sha.to_string(), map.get("size").and_then(|v| v.as_i64()).unwrap_or(0)));
                    return;
                }
            }
            for (_, v) in map {
                collect_attachments(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_attachments(v, out);
            }
        }
        _ => {}
    }
}

async fn upload_attachments(
    app: &AppHandle,
    access: &str,
    server_url: &str,
    envelope: &Value,
    uploaded: &mut HashSet<String>,
) {
    let mut refs = Vec::new();
    collect_attachments(envelope, &mut refs);
    if refs.is_empty() {
        return;
    }
    let agent_dir = {
        let supervisor = app.state::<Supervisor>();
        match supervisor.request(app.clone(), "config.get".into(), json!({})).await {
            Ok(v) => v
                .get("agent_dir")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            Err(_) => return,
        }
    };
    let client = reqwest::Client::new();
    for (sha, size) in refs {
        if uploaded.contains(sha.as_str()) {
            continue;
        }
        let path = PathBuf::from(&agent_dir).join("pidock").join("attachments").join(&sha);
        let presign = json!({"token": access, "sha256": sha, "size": size});
        let resp = client
            .post(format!("{server_url}/attachments/presign"))
            .json(&presign)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;
        let data = match resp {
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("attachment presign parse failed: {e}");
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("attachment presign failed: {e}");
                continue;
            }
        };
        let Some(upload_url) = data.get("upload_url").and_then(|v| v.as_str()) else {
            continue;
        };
        if data.get("exists").and_then(|v| v.as_bool()) != Some(true) {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if let Err(e) = client.put(upload_url).body(bytes).send().await {
                        tracing::warn!("attachment upload failed: {e}");
                        continue;
                    }
                }
                Err(e) => {
                    tracing::warn!("attachment local file missing ({}): {e}", path.display());
                    continue;
                }
            }
        }
        let sha_short: String = sha.chars().take(12).collect();
        uploaded.insert(sha);
        tracing::info!("attachment {sha_short}… uploaded");
    }
}

fn command_result_envelope(session_id: &str, command_id: &str, result: &Result<Option<Value>, String>) -> Value {
    let payload = match result {
        Ok(r) => json!({"command_id": command_id, "ok": true, "result": r.clone().unwrap_or(json!({}))}),
        Err(e) => json!({"command_id": command_id, "ok": false, "error": e}),
    };
    json!({
        "event_id": Uuid::now_v7().to_string(),
        "session_id": session_id,
        "persist": false,
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": "command_result",
        "payload": payload,
    })
}

/// translate a server command into a host request
async fn execute_command(
    app: &AppHandle,
    session_id: &str,
    cmd_type: &str,
    payload: &Value,
) -> Result<Option<Value>, String> {
    let supervisor = app.state::<Supervisor>();
    let (method, params) = match cmd_type {
        // 图片/文档附件原样透传（web 端共享同一 UI，发送载荷含 images/attachments）
        "agent.prompt" => {
            let mut p = json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")});
            for key in ["images", "attachments"] {
                if let Some(v) = payload.get(key) {
                    if v.is_array() && !v.as_array().unwrap().is_empty() {
                        p[key] = v.clone();
                    }
                }
            }
            ("agent.prompt", p)
        }
        "agent.steer" => ("agent.steer", json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")})),
        "agent.follow_up" => ("agent.follow_up", json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")})),
        "agent.abort" => ("agent.abort", json!({"session_id": session_id})),
        "session.create" => ("session.create", json!({"cwd": payload.get("cwd").and_then(|t| t.as_str()).unwrap_or(""), "model": payload.get("model")})),
        "session.close" => ("session.close", json!({"session_id": session_id})),
        "session.resolve_approval" => ("session.resolve_approval", json!({
            "approval_id": payload.get("approval_id").and_then(|v| v.as_str()).unwrap_or(""),
            "approved": payload.get("approved").and_then(|v| v.as_bool()).unwrap_or(false),
        })),
        "session.resolve_ask" => ("session.resolve_ask", {
            let mut p = json!({
                "ask_id": payload.get("ask_id").and_then(|v| v.as_str()).unwrap_or(""),
                "interrupted": payload.get("interrupted").and_then(|v| v.as_bool()).unwrap_or(false),
            });
            // labels/text 可选：取消回答时两者皆缺
            if let Some(v) = payload.get("labels") {
                if v.is_array() { p["labels"] = v.clone(); }
            }
            if let Some(v) = payload.get("text") {
                if v.is_string() { p["text"] = v.clone(); }
            }
            p
        }),
        other => {
            return Err(format!("command type \"{other}\" not supported by this desktop"));
        }
    };
    supervisor
        .request(app.clone(), method.to_string(), params)
        .await
        .map(Some)
}

/// run the sync loop until disabled or control message
async fn sync_loop(app: AppHandle, mut event_rx: broadcast::Receiver<pidock_protocol::Envelope>, mut control_rx: mpsc::Receiver<SyncControl>) {
    let supervisor = app.state::<Supervisor>();
    let sync = app.state::<SyncManager>();
    let mut backoff_secs = 1u64;
    let mut uploaded: HashSet<String> = HashSet::new();

    'outer: loop {
        let (mut cfg, enabled) = {
            let c = sync.cfg.lock().await;
            (c.clone(), c.enabled && !c.refresh_token.is_empty())
        };
        if !enabled {
            sync.set_status(SyncStatus { enabled: false, connected: false, machine_id: cfg.machine_id.clone(), error: None }).await;
            // wait for re-enable
            match control_rx.recv().await {
                Some(SyncControl::Restart) => continue,
                _ => continue,
            }
        }

        // 1) access token — persist the rotated refresh token immediately,
        // otherwise the next reconnect fails with "revoked or expired"
        let (access, new_refresh) = match refresh_access(&cfg).await {
            Ok(pair) => pair,
            Err(e) => {
                let user_msg = if e.contains("revoked") || e.contains("expired") || e.contains("Unauthorized") {
                    // the stored token is dead: drop it so the next configure
                    // requires a fresh password login
                    let mut c = sync.cfg.lock().await;
                    c.refresh_token.clear();
                    sync.save(&c);
                    cfg.refresh_token.clear();
                    "登录已过期，请重新输入密码后点击「连接并同步」".to_string()
                } else {
                    e.clone()
                };
                sync.set_status(SyncStatus { enabled: true, connected: false, machine_id: cfg.machine_id.clone(), error: Some(user_msg.clone()) }).await;
                tracing::warn!("sync: refresh failed: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
                backoff_secs *= 2;
                continue;
            }
        };
        {
            let mut c = sync.cfg.lock().await;
            c.refresh_token = new_refresh;
            sync.save(&c);
            cfg = c.clone();
        }

        // 2) ws connect
        let ws_url = format!(
            "{}/ws/desktop?token={}",
            cfg.server_url.trim_end_matches('/').replacen("http", "ws", 1),
            access
        );
        let (ws, _) = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok(pair) => pair,
            Err(e) => {
                sync.set_status(SyncStatus { enabled: true, connected: false, machine_id: cfg.machine_id.clone(), error: Some(format!("connect: {e}")) }).await;
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
                backoff_secs *= 2;
                continue;
            }
        };
        backoff_secs = 1;
        let (mut ws_sink, mut ws_stream) = ws.split();

        // 3) register machine
        let hostname = hostname();
        let register = json!({
            "ctrl": "register",
            "machine_id": cfg.machine_id,
            "hostname": hostname,
            "os": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION"),
        });
        if ws_sink.send(WsMessage::Text(register.to_string().into())).await.is_err() {
            continue;
        }
        sync.set_status(SyncStatus { enabled: true, connected: true, machine_id: cfg.machine_id.clone(), error: None }).await;
        tracing::info!("sync: connected to {}", cfg.server_url);

        // 4) backfill open sessions (server dedupes via (session_id, seq) upsert)
        {
            if let Ok(list) = supervisor
                .request(app.clone(), "session.list".into(), json!({}))
                .await
            {
                if let Some(sessions) = list.get("sessions").and_then(|v| v.as_array()) {
                    for s in sessions {
                        // backfill ALL known sessions so the web list is complete,
                        // not just the ones currently open on this desktop
                        let sid = s.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if sid.is_empty() {
                            continue;
                        }
                        if let Ok(replay) = supervisor
                            .request(app.clone(), "session.events".into(), json!({"session_id": sid}))
                            .await
                        {
                            if let Some(events) = replay.get("events").and_then(|v| v.as_array()) {
                                for ev in events {
                                    let env = json!({
                                        "event_id": Uuid::now_v7().to_string(),
                                        "session_id": sid,
                                        "seq": ev.get("seq").cloned().unwrap_or(json!(0)),
                                        "persist": true,
                                        "ts": ev.get("ts").cloned().unwrap_or(json!("")),
                                        "kind": ev.get("kind").cloned().unwrap_or(json!("")),
                                        "payload": ev.get("payload").cloned().unwrap_or(json!({})),
                                    });
                                    upload_attachments(&app, &access, &cfg.server_url, &env, &mut uploaded).await;
                                    let _ = ws_sink.send(WsMessage::Text(env.to_string().into())).await;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 5) pump: host events + heartbeats + downstream commands
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut closed = false;
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if ws_sink.send(WsMessage::Text(json!({"ctrl":"heartbeat"}).to_string().into())).await.is_err() {
                        closed = true;
                    }
                }
                ev = event_rx.recv() => {
                    match ev {
                        Ok(envelope) => {
                            let persist = envelope.persist;
                            if persist || cloud_worthy(&envelope.kind) {
                                let serialized = serde_json::to_value(&envelope).unwrap_or(json!({}));
                                upload_attachments(&app, &access, &cfg.server_url, &serialized, &mut uploaded).await;
                                if ws_sink.send(WsMessage::Text(serialized.to_string().into())).await.is_err() {
                                    closed = true;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("sync: lagged, skipped {n} host events");
                        }
                        Err(broadcast::error::RecvError::Closed) => closed = true,
                    }
                }
                frame = ws_stream.next() => {
                    match frame {
                        Some(Ok(WsMessage::Text(text))) => {
                            if let Ok(frame) = serde_json::from_str::<Value>(&text) {
                                if frame.get("ctrl").and_then(|v| v.as_str()) == Some("command") {
                                    let command_id = frame.get("command_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let command = frame.get("command").cloned().unwrap_or(json!({}));
                                    let session_id = command.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let cmd_type = command.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let result = execute_command(&app, &session_id, &cmd_type, command.get("payload").unwrap_or(&json!({}))).await;
                                    let reply = command_result_envelope(&session_id, &command_id, &result);
                                    let _ = ws_sink.send(WsMessage::Text(reply.to_string().into())).await;
                                } else if frame.get("ctrl").and_then(|v| v.as_str()) == Some("registered") {
                                    if let Some(mid) = frame.get("machine_id").and_then(|v| v.as_str()) {
                                        let mut c = sync.cfg.lock().await;
                                        if c.machine_id != mid {
                                            c.machine_id = mid.to_string();
                                            sync.save(&c.clone());
                                            drop(c);
                                            sync.set_status(SyncStatus{enabled: true, connected: true, machine_id: mid.to_string(), error: None}).await;
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(_)) => {}
                        _ => { closed = true; }
                    }
                }
                ctrl = control_rx.recv() => {
                    match ctrl {
                        Some(SyncControl::Stop) => {
                            let _ = ws_sink.send(WsMessage::Close(None)).await;
                            sync.set_status(SyncStatus{enabled: false, connected: false, machine_id: cfg.machine_id.clone(), error: None}).await;
                            continue 'outer; // back to the idle wait; receiver stays alive
                        }
                        Some(SyncControl::Restart) => {
                            let _ = ws_sink.send(WsMessage::Close(None)).await;
                            continue 'outer;
                        }
                        None => break 'outer,
                    }
                }
            }
            if closed {
                sync.set_status(SyncStatus{enabled: true, connected: false, machine_id: cfg.machine_id.clone(), error: Some("connection lost".into())}).await;
                break; // reconnect via backoff
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

// ------------------------------------------------------------- tauri API ---

#[derive(Deserialize)]
pub struct SyncConfigureBody {
    pub server_url: String,
    pub email: String,
    #[serde(default)]
    pub password: String,
}

#[tauri::command]
pub async fn sync_configure(
    _app: AppHandle,
    state: tauri::State<'_, SyncManager>,
    body: SyncConfigureBody,
) -> Result<Value, String> {
    let server_url = body.server_url.trim_end_matches('/').to_string();
    let mut cfg = state.cfg.lock().await.clone();
    cfg.server_url = server_url.clone();
    cfg.email = body.email.clone();
    cfg.enabled = true;

    if !body.password.is_empty() {
        let data = http_json(
            "POST",
            &format!("{server_url}/auth/login"),
            &json!({"email": body.email, "password": body.password}),
        )
        .await?;
        cfg.refresh_token = data
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .ok_or("login response missing refresh_token")?
            .to_string();
    }
    if cfg.refresh_token.is_empty() {
        return Err("没有可用的登录凭据，请输入密码".into());
    }

    state.save(&cfg);
    *state.cfg.lock().await = cfg;
    let _ = state.control.send(SyncControl::Restart).await;
    Ok(json!({"ok": true}))
}

#[tauri::command]
pub async fn sync_disable(state: tauri::State<'_, SyncManager>) -> Result<Value, String> {
    let mut cfg = state.cfg.lock().await.clone();
    cfg.enabled = false;
    state.save(&cfg);
    *state.cfg.lock().await = cfg;
    let _ = state.control.send(SyncControl::Stop).await;
    Ok(json!({"ok": true}))
}

#[tauri::command]
pub async fn sync_status(state: tauri::State<'_, SyncManager>) -> Result<Value, String> {
    let cfg = state.cfg.lock().await.clone();
    let st = state.status.lock().await.clone();
    Ok(json!({
        "enabled": st.enabled,
        "connected": st.connected,
        "machine_id": st.machine_id,
        "email": cfg.email,
        "server_url": cfg.server_url,
        "error": st.error,
    }))
}

/// spawn the sync loop once at app startup; the loop lives for the whole app
pub fn spawn(
    app: AppHandle,
    event_rx: broadcast::Receiver<pidock_protocol::Envelope>,
    control_rx: mpsc::Receiver<SyncControl>,
) {
    tauri::async_runtime::spawn(async move {
        sync_loop(app, event_rx, control_rx).await;
    });
}
