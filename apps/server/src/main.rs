//! PiDock server entry: HTTP API + desktop/web WebSockets + ingest consumer.

mod auth;
mod config;
mod gateway;
mod ingest;
mod pipeline;
mod s3;
mod state;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, get_service, post},
    Json, Router,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document as BsonDoc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::borrow::Cow;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": msg.into()})))
}

// ------------------------------------------------------- embedded web console

/// apps/web/dist 嵌入（路径相对 apps/server）。debug 构建默认从磁盘实时读，
/// release 构建才是真嵌入——本地开发体验不变，发布产物单文件。
#[derive(rust_embed::RustEmbed)]
#[folder = "../web/dist"]
struct WebAssets;

fn embedded(p: &str) -> Option<(mime_guess::Mime, bytes::Bytes)> {
    WebAssets::get(p).map(|f| {
        let data = match f.data {
            Cow::Borrowed(b) => bytes::Bytes::from_static(b),
            Cow::Owned(v) => bytes::Bytes::from(v),
        };
        (mime_guess::from_path(p).first_or_octet_stream(), data)
    })
}

/// SPA 兜底：命中资源文件直接返回；末段带点的路径（.js/.css 等资源）缺失才 404；
/// 其余未知路径一律回 index.html。
async fn web_console(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some((mime, body)) = embedded(path).filter(|_| !path.is_empty()) {
        return ([(header::CONTENT_TYPE, mime.as_ref())], body).into_response();
    }
    let last_seg = path.rsplit('/').next().unwrap_or("");
    if !last_seg.contains('.') {
        if let Some((mime, body)) = embedded("index.html") {
            return ([(header::CONTENT_TYPE, mime.as_ref())], body).into_response();
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

#[derive(Deserialize)]
struct TokenQuery {
    token: String,
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"ok": true, "pipeline": state.cfg.pipeline}))
}

// ------------------------------------------------------------ attachments

#[derive(Deserialize)]
struct PresignBody {
    token: String,
    sha256: String,
    size: i64,
}

/// presign an attachment upload; content-addressed (sha256) so identical
/// payloads upload once. Ownership is bound on first upload.
async fn attachments_presign(
    State(state): State<AppState>,
    Json(body): Json<PresignBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &body.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    if body.sha256.len() != 64 || !body.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid sha256"));
    }
    let meta = state.mongo.collection::<BsonDoc>("attachment_meta");
    let existing = meta
        .find_one(doc! {"_id": &body.sha256})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let exists = existing.is_some();
    if let Some(doc) = existing {
        if doc.get_str("user_id") != Ok(user_id.as_str()) {
            // content hash collision across users: allow but keep first owner
            // (content is addressed by hash; access control is on download)
            return Err(err(
                StatusCode::CONFLICT,
                "attachment belongs to another account",
            ));
        }
    } else {
        let _ = meta
            .insert_one(doc! {
                "_id": &body.sha256,
                "user_id": &user_id,
                "size": body.size,
                "created_at": chrono::Utc::now().to_rfc3339(),
            })
            .await;
    }
    let url = state.s3.presign_put(&body.sha256, 900);
    Ok(Json(
        json!({"attachment_id": body.sha256, "upload_url": url, "exists": exists}),
    ))
}

async fn attachment_url(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let meta = state.mongo.collection::<BsonDoc>("attachment_meta");
    let doc = meta
        .find_one(doc! {"_id": &attachment_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "attachment not found"))?;
    if doc.get_str("user_id") != Ok(user_id.as_str()) {
        return Err(err(StatusCode::FORBIDDEN, "not your attachment"));
    }
    let url = state.s3.presign_get(&attachment_id, 900);
    Ok(Json(
        json!({"url": url, "size": doc.get_i64("size").unwrap_or(0)}),
    ))
}

async fn me(
    State(state): State<AppState>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let claims = auth::verify_token(&state, &q.token, "access")
        .await
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    Ok(Json(json!({"user_id": claims.sub})))
}

/// machines list: Mongo archive + Redis online status merged
async fn machines(
    State(state): State<AppState>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let docs = state
        .mongo
        .collection::<BsonDoc>("machines")
        .find(doc! {"user_id": &user_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut conn = state.redis.clone();
    let mut out = Vec::new();
    for d in docs {
        let machine_id = d.get_str("_id").unwrap_or_default().to_string();
        let online: bool = redis::cmd("EXISTS")
            .arg(format!("online:machine:{machine_id}"))
            .query_async(&mut conn)
            .await
            .unwrap_or(0)
            == 1;
        out.push(json!({
            "machine_id": machine_id,
            "hostname": d.get_str("hostname").unwrap_or(""),
            "os": d.get_str("os").unwrap_or(""),
            "version": d.get_str("version").unwrap_or(""),
            "local_ip": d.get_str("local_ip").unwrap_or(""),
            "last_seen": d.get_str("last_seen").unwrap_or(""),
            "online": online,
        }));
    }
    Ok(Json(json!({"machines": out})))
}

/// delete a machine and all its synced data (web machine management)
async fn delete_machine(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let machines = state.mongo.collection::<BsonDoc>("machines");
    let owner = machines
        .find_one(doc! {"_id": &machine_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "machine not found"))?;
    if owner.get_str("user_id") != Ok(user_id.as_str()) {
        return Err(err(StatusCode::FORBIDDEN, "not your machine"));
    }
    machines
        .delete_one(doc! {"_id": &machine_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let sessions = state.mongo.collection::<BsonDoc>("sessions");
    sessions
        .delete_many(doc! {"user_id": &user_id, "machine_id": &machine_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state
        .mongo
        .collection::<BsonDoc>("session_events")
        .delete_many(doc! {"user_id": &user_id, "machine_id": &machine_id})
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut conn = state.redis.clone();
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(format!("online:machine:{machine_id}"))
        .query_async(&mut conn)
        .await;
    let _: Result<i64, _> = redis::cmd("SREM")
        .arg(format!("online:user:{user_id}"))
        .arg(&machine_id)
        .query_async(&mut conn)
        .await;
    tracing::info!(user = %user_id, machine = %machine_id, "machine deleted with all synced data");
    Ok(Json(json!({"ok": true})))
}

/// sessions for a machine (web list view)
async fn machine_sessions(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    // 机器注册帧带来的 host 主目录（侧栏「项目/任务」拆分依据；旧机器行没有该字段则为空）
    let home = state
        .mongo
        .collection::<BsonDoc>("machines")
        .find_one(doc! {"_id": &machine_id})
        .await
        .ok()
        .flatten()
        .and_then(|d| d.get_str("home").ok().map(str::to_string))
        .unwrap_or_default();
    let docs = state
        .mongo
        .collection::<BsonDoc>("sessions")
        .find(doc! {"user_id": &user_id, "machine_id": &machine_id})
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! {"updated_at": -1})
                .limit(200)
                .build(),
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let sessions: Vec<Value> = docs
        .iter()
        .map(|d| {
            json!({
                "session_id": d.get_str("_id").unwrap_or(""),
                "title": d.get_str("title").unwrap_or(""),
                "status": d.get_str("status").unwrap_or("idle"),
                "model": d.get_str("model").unwrap_or(""),
                "cwd": d.get_str("cwd").unwrap_or(""),
                "expert_id": d.get_str("expert_id").unwrap_or(""),
                "expert_name": d.get_str("expert_name").unwrap_or(""),
                "parent_session_id": d.get_str("parent_session_id").unwrap_or(""),
                "created_at": d.get_str("created_at").unwrap_or(""),
                "updated_at": d.get_str("updated_at").unwrap_or(""),
            })
        })
        .collect();
    Ok(Json(json!({"sessions": sessions, "home": home})))
}

/// 删除镜像里的单个会话及其事件（web 端「移除项目/删除会话」时，
/// 桌面端命令删掉 host 注册表条目后调这里同步清镜像）
async fn delete_machine_session(
    State(state): State<AppState>,
    Path((machine_id, session_id)): Path<(String, String)>,
    Query(q): Query<TokenQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let owner = doc! {"_id": &session_id, "user_id": &user_id, "machine_id": &machine_id};
    let sessions = state.mongo.collection::<BsonDoc>("sessions");
    let res = sessions
        .delete_one(owner.clone())
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if res.deleted_count == 0 {
        return Err(err(StatusCode::NOT_FOUND, "session not found"));
    }
    state
        .mongo
        .collection::<BsonDoc>("session_events")
        .delete_many(
            doc! {"session_id": &session_id, "user_id": &user_id, "machine_id": &machine_id},
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({"deleted": true})))
}

/// persisted history replay for one session
#[derive(Deserialize)]
struct EventsQuery {
    token: String,
    after_seq: Option<i64>,
}

async fn session_events(
    State(state): State<AppState>,
    Path((machine_id, session_id)): Path<(String, String)>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &q.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let mut filter =
        doc! {"user_id": &user_id, "machine_id": &machine_id, "session_id": &session_id};
    if let Some(after) = q.after_seq {
        filter.insert("seq", doc! {"$gt": after});
    }
    let docs = state
        .mongo
        .collection::<BsonDoc>("session_events")
        .find(filter)
        .with_options(
            mongodb::options::FindOptions::builder()
                .sort(doc! {"seq": 1})
                .limit(2000)
                .build(),
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let events: Vec<Value> = docs
        .iter()
        .filter_map(|d| {
            let payload_str = d.get_str("payload").ok()?;
            Some(json!({
                "seq": d.get_i64("seq").ok()?,
                "ts": d.get_str("ts").ok()?,
                "kind": d.get_str("kind").ok()?,
                "payload": serde_json::from_str::<Value>(payload_str).unwrap_or(json!({})),
            }))
        })
        .collect();
    Ok(Json(json!({"events": events})))
}

/// web -> desktop command (prompt / steer / abort / approve), online-only
#[derive(Deserialize)]
struct CommandBody {
    token: String,
    machine_id: String,
    session_id: String,
    #[serde(rename = "type")]
    command_type: String,
    payload: Value,
}

async fn post_command(
    State(state): State<AppState>,
    Json(body): Json<CommandBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = auth::verify_token(&state, &body.token, "access")
        .await
        .map(|c| c.sub)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, e))?;
    let command = json!({
        "session_id": body.session_id,
        "type": body.command_type,
        "payload": body.payload,
    });
    match gateway::route_command(&state, &user_id, &body.machine_id, &command).await {
        Ok(command_id) => {
            // audit trail
            let _ = state
                .mongo
                .collection::<BsonDoc>("commands")
                .insert_one(doc! {
                    "command_id": &command_id,
                    "user_id": &user_id,
                    "machine_id": &body.machine_id,
                    "session_id": &body.session_id,
                    "type": &body.command_type,
                    "payload": serde_json::to_string(&body.payload).unwrap_or_default(),
                    "status": "sent",
                    "ts": chrono::Utc::now().to_rfc3339(),
                })
                .await;
            Ok(Json(json!({"command_id": command_id, "status": "sent"})))
        }
        Err(e) => {
            let status = if e == "refused_offline" {
                "refused_offline"
            } else if e == "forbidden" {
                "forbidden"
            } else {
                "error"
            };
            Ok(Json(json!({"status": status})))
        }
    }
}

/// 解析 --config <path> / --config=<path>；不指定时 Config::load 自行兜底
fn cli_config_path() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--config=") {
            return Some(v.to_string());
        }
        if a == "--config" {
            return args.get(i + 1).cloned();
        }
    }
    None
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pidock_server=info,tower_http=warn".into()),
        )
        .init();

    let cfg = config::Config::load(cli_config_path()).expect("配置加载失败");
    let web_dir = cfg.web_dir.clone(); // cfg 随后 move 进 AppState，先取出
    let sink = match pipeline::make_sink(&cfg).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("pipeline init failed: {e}");
            std::process::exit(1);
        }
    };
    let state = match AppState::connect(cfg, sink).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("state connect failed: {e}");
            std::process::exit(1);
        }
    };
    let state_for_ingest = state.clone();

    // object storage: create the attachments bucket up front
    if let Err(e) = state.s3.ensure_bucket().await {
        tracing::warn!("s3 bucket ensure failed (attachments disabled until fixed): {e}");
    }

    // ensure unique index + start ingest consumer in background
    tokio::spawn(async move {
        if let Err(e) = ingest::run(state_for_ingest).await {
            tracing::error!("ingest exited: {e}");
        }
    });

    // web UI static hosting：PIDOCK_WEB_DIR / 配置文件 [web].dir 显式指定目录时走
    // ServeDir（本地改前端即生效）；否则用编译期嵌入的 apps/web/dist——debug 构建走
    // 磁盘、release 构建真嵌入，产物即单文件。API 路由优先，未命中路径交给兜底。

    let app = Router::new()
        .route("/health", get(health))
        .route("/me", get(me))
        .route("/machines", get(machines))
        .route("/machines/{machine_id}", delete(delete_machine))
        .route("/machines/{machine_id}/sessions", get(machine_sessions))
        .route(
            "/machines/{machine_id}/sessions/{session_id}",
            delete(delete_machine_session),
        )
        .route(
            "/machines/{machine_id}/sessions/{session_id}/events",
            get(session_events),
        )
        .route("/commands", post(post_command))
        .route("/attachments/presign", post(attachments_presign))
        .route("/attachments/{attachment_id}/url", get(attachment_url))
        .route("/ws/desktop", get(gateway::ws_desktop))
        .route("/ws/web", get(gateway::ws_web))
        .merge(auth::router())
        .layer(CorsLayer::permissive())
        .with_state(state.clone());
    let app = if web_dir.is_empty() {
        tracing::info!("hosting web console from embedded assets");
        app.fallback(web_console)
    } else {
        tracing::info!("hosting web console from {web_dir}");
        app.fallback_service(get_service(
            ServeDir::new(&web_dir).append_index_html_on_directories(true),
        ))
    };

    let bind = state.cfg.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    tracing::info!("pidock-server listening on {bind}");
    axum::serve(listener, app).await.expect("serve");
}

#[cfg(test)]
mod web_console_tests {
    use super::*;
    use axum::http::Uri;

    async fn status_of(target: &str) -> (u16, String) {
        let uri = target.parse::<Uri>().unwrap();
        let resp = web_console(uri).await;
        let ct = resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        (resp.status().as_u16(), ct)
    }

    #[tokio::test]
    async fn root_serves_index_html() {
        let (status, ct) = status_of("/").await;
        assert_eq!(status, 200);
        assert!(ct.starts_with("text/html"), "content-type = {ct}");
    }

    #[tokio::test]
    async fn spa_route_falls_back_to_index() {
        let (status, ct) = status_of("/some/spa/route").await;
        assert_eq!(status, 200);
        assert!(ct.starts_with("text/html"), "content-type = {ct}");
    }

    #[tokio::test]
    async fn missing_asset_404s() {
        let (status, _) = status_of("/assets/definitely-missing.js").await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn js_asset_has_js_content_type() {
        // 占位 dist（只有 index.html）时无 js 资源，跳过
        let Some(js) = WebAssets::iter().find(|p| p.ends_with(".js")) else {
            return;
        };
        let (status, ct) = status_of(&format!("/{js}")).await;
        assert_eq!(status, 200);
        assert!(
            ct.starts_with("text/javascript") || ct.starts_with("application/javascript"),
            "content-type = {ct}"
        );
    }
}
