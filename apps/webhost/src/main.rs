//! PiDock webhost：桌面核心的无头壳。
//!
//! 与 Tauri 桌面壳共用 pidock-core（supervisor / 调度器 / 云同步）：
//! - axum 静态托管桌面 UI（release 构建把 apps/desktop/dist 嵌进二进制，
//!   debug 构建直接读磁盘便于前端开发）；
//! - `/ws` 是 IPC 桥：浏览器的 `ipc(cmd, args)` 请求转发给 `CoreCtx::handle`，
//!   core 事件广播实时推给所有客户端——与 Tauri `invoke`/`pidock:event` 同语义；
//! - 可选 `PIDOCK_WEB_TOKEN`：设置后 `/ws` 必须带匹配 token（`?token=`），
//!   否则 401。令牌不匹配无法建立事件流，静态资源仅是空壳 UI。
//!
//! 环境变量：`PIDOCK_WEB_BIND`（默认 0.0.0.0:8090）、`PIDOCK_WEB_TOKEN`、
//! `PIDOCK_DATA_DIR`（默认 `~/.pidock-web`，放 jobs.json / sync.json / desktop.json）、
//! `PIDOCK_HOST_CMD` / `PIDOCK_HOST_DIR` / `PIDOCK_HOST_ARGS`（与桌面端一致，
//! release 下默认拉起与二进制同目录的 `pidock-host` sidecar）。

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    extract::{
        ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade},
        RawQuery, State,
    },
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use pidock_core::{CoreCtx, CorePaths};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

/// apps/desktop/dist 嵌入（路径相对 apps/webhost）。debug 构建默认从磁盘实时读，
/// release 构建才是真嵌入——本地开发体验不变，发布产物单文件。
#[derive(rust_embed::RustEmbed)]
#[folder = "../../apps/desktop/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    ctx: Arc<CoreCtx>,
    token: Option<String>,
}

fn asset_response(name: &str, data: impl Into<Vec<u8>>) -> Response {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    ([(header::CONTENT_TYPE, mime.to_string())], data.into()).into_response()
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(path) {
        return asset_response(path, file.data);
    }
    // SPA fallback：非资源路径一律回 index.html（前端无路由，双保险）
    match Assets::get("index.html") {
        Some(f) => asset_response("index.html", f.data),
        None => (
            StatusCode::NOT_FOUND,
            "frontend assets missing: build apps/desktop first (pnpm --filter @pidock/desktop build)",
        )
            .into_response(),
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Response {
    // token 鉴权：配置了 PIDOCK_WEB_TOKEN 时 ?token= 必须匹配。
    // 先完成握手再以 4401 关闭，让前端能区分「令牌错误」与「服务不可达」。
    let authorized = match &state.token {
        None => true,
        Some(expected) => {
            let provided = query
                .as_deref()
                .unwrap_or_default()
                .split('&')
                .find_map(|kv| kv.strip_prefix("token="))
                .unwrap_or_default();
            percent_decode_eq(provided, expected)
        }
    };
    ws.on_upgrade(move |socket| async move {
        if !authorized {
            let mut socket = socket;
            let _ = socket
                .send(WsMessage::Close(Some(CloseFrame {
                    code: 4401,
                    reason: "unauthorized".into(),
                })))
                .await;
            return;
        }
        ws_bridge(socket, state).await;
    })
}

/// 极简 percent-decode 后比较（token 等值判断；非法序列按字面量处理）
fn percent_decode_eq(raw: &str, expected: &str) -> bool {
    let b = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out) == expected
}

async fn ws_bridge(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    // 单写者：请求回复（各异步任务）与事件推送（select 循环）都经此通道串行发出
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(WsMessage::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // core 事件广播 -> 该客户端
    let mut ev_rx = state.ctx.events.subscribe();

    loop {
        tokio::select! {
            frame = stream.next() => {
                match frame {
                    Some(Ok(WsMessage::Text(text))) => {
                        handle_client_frame(&state, text.as_str(), &tx).await;
                    }
                    Some(Ok(_)) => {} // 二进制/ping 忽略
                    Some(Err(e)) => {
                        tracing::debug!("ws client error: {e}");
                        break;
                    }
                    None => break, // 客户端断开
                }
            }
            ev = ev_rx.recv() => {
                match ev {
                    Ok(envelope) => {
                        let text = serde_json::to_string(&json!({"t": "event", "envelope": envelope}))
                            .unwrap_or_default();
                        if tx.send(text).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    writer.abort(); // tx 在此 drop，writer 也会自然退出
}

async fn handle_client_frame(state: &AppState, text: &str, tx: &mpsc::UnboundedSender<String>) {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("ws: unparseable frame: {text:.200}");
        return;
    };
    if frame.get("t").and_then(|v| v.as_str()) != Some("req") {
        return;
    }
    let id = frame.get("id").cloned().unwrap_or(Value::Null);
    let cmd = frame
        .get("cmd")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = frame.get("args").cloned().unwrap_or(json!({}));
    let ctx = state.ctx.clone();
    let tx = tx.clone();
    // 并发处理：长请求（agent.prompt 上限 180s）不阻塞后续请求与事件推送；
    // 回复带 id，乱序到达由前端按 id 匹配
    tokio::spawn(async move {
        let reply = match ctx.handle(&cmd, args).await {
            Ok(result) => json!({"t": "res", "id": id, "ok": true, "result": result}),
            Err(e) => json!({"t": "res", "id": id, "ok": false, "error": e}),
        };
        let _ = tx.send(reply.to_string());
    });
}

#[tokio::main]
async fn main() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let bind: SocketAddr = std::env::var("PIDOCK_WEB_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8091".into())
        .parse()
        .expect("PIDOCK_WEB_BIND 无效");
    let token = std::env::var("PIDOCK_WEB_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    if token.is_some() {
        tracing::info!("ws auth: enabled (PIDOCK_WEB_TOKEN)");
    } else {
        tracing::warn!(
            "ws auth: disabled — 仅建议可信内网使用，公网/不可信网络请设置 PIDOCK_WEB_TOKEN"
        );
    }

    let data_dir = std::env::var("PIDOCK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".into());
            PathBuf::from(base).join(".pidock-web")
        });
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        tracing::warn!("创建数据目录 {} 失败: {e}", data_dir.display());
    }

    let (sync_tx, sync_rx) = mpsc::channel::<pidock_core::sync::SyncControl>(8);
    let ctx = CoreCtx::new(
        CorePaths {
            jobs: data_dir.join("jobs.json"),
            sync_cfg: data_dir.join("sync.json"),
            desktop_cfg: data_dir.join("desktop.json"),
        },
        sync_tx,
    );
    ctx.start(sync_rx).await;

    let state = AppState { ctx, token };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", get(ws_handler))
        .fallback(get(static_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await.expect("bind");
    tracing::info!("pidock-webhost listening on http://{bind}");
    axum::serve(listener, app).await.expect("serve");
}
