//! Ingest consumer: pipeline -> dedupe -> Mongo (persisted events + session
//! status) -> Redis pub/sub push to web clients.

use mongodb::bson::{doc, Document as BsonDoc};
use serde_json::{json, Value};

use crate::state::AppState;

/// session status derived from envelope kinds
fn derive_status(kind: &str, payload: &Value, current: Option<&str>) -> Option<String> {
    match kind {
        "message_complete" | "tool_execution_start" => Some("running".into()),
        "agent_state_changed" => payload
            .get("state")
            .and_then(|s| s.as_str())
            .map(|s| match s {
                "waiting_approval" => "waiting_approval".to_string(),
                "idle" => "idle".to_string(),
                _ => "running".to_string(),
            }),
        "error" => Some("error".into()),
        _ => current.map(str::to_string),
    }
}

pub async fn run(state: AppState) -> anyhow::Result<()> {
    // unique (session_id, seq) index: idempotent upserts across reconnect backfills
    let events = state.mongo.collection::<BsonDoc>("session_events");
    let index = mongodb::IndexModel::builder()
        .keys(doc! {"session_id": 1, "seq": 1})
        .options(
            mongodb::options::IndexOptions::builder()
                .unique(true)
                .build(),
        )
        .build();
    events.create_index(index).await?;

    // frames are processed strictly in order (a single worker): the
    // session_resynced marker must land before the re-upserts that follow it
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(4096);
    let worker_state = state.clone();
    tokio::spawn(async move {
        while let Some(payload) = rx.recv().await {
            handle_frame(&worker_state, &payload).await;
        }
    });

    let producer_state = state.clone();
    let handler = Box::new(move |payload: String| {
        let _ = producer_state.clone();
        let _ = tx.try_send(payload);
    }) as Box<dyn FnMut(String) + Send>;

    tracing::info!("ingest consumer running");
    crate::pipeline::run_source(&state.cfg, handler).await
}

async fn handle_frame(state: &AppState, payload: &str) {
    let frame: Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("ingest: bad frame: {e}");
            return;
        }
    };
    let user_id = frame
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let machine_id = frame
        .get("machine_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let envelope = frame.get("envelope").cloned().unwrap_or_else(|| json!({}));

    ingest_one(state, &user_id, &machine_id, &envelope).await;

    // realtime push to web clients of this user
    state.publish_to_user(&user_id, payload).await;
}

async fn ingest_one(state: &AppState, user_id: &str, machine_id: &str, envelope: &Value) {
    let session_id = envelope
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kind = envelope.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let events = state.mongo.collection::<BsonDoc>("session_events");
    let sessions = state.mongo.collection::<BsonDoc>("sessions");

    // compaction marker: rebuild this session's event history
    if kind == "session_resynced" {
        if let Err(e) = events.delete_many(doc! {"session_id": session_id}).await {
            tracing::error!("ingest: resync delete failed: {e}");
        }
        let _ = sessions.delete_one(doc! {"_id": session_id}).await;
        tracing::info!("ingest: session {session_id} resynced, history cleared");
        return;
    }

    // removal marker: drop the mirror row + events entirely (session 已从 host 注册表移除)
    if kind == "session_removed" {
        if let Err(e) = events.delete_many(doc! {"session_id": session_id}).await {
            tracing::error!("ingest: remove delete failed: {e}");
        }
        let _ = sessions.delete_one(doc! {"_id": session_id}).await;
        tracing::info!("ingest: session {session_id} removed, mirror cleared");
        return;
    }

    let persist = envelope
        .get("persist")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if persist {
        let seq = envelope.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let event_doc = doc! {
            "user_id": user_id,
            "machine_id": machine_id,
            "session_id": session_id,
            "seq": seq,
            "event_id": envelope.get("event_id").and_then(|v| v.as_str()).unwrap_or(""),
            "kind": kind,
            "payload": serde_json::to_string(envelope.get("payload").unwrap_or(&json!({}))).unwrap_or_default(),
            "ts": envelope.get("ts").and_then(|v| v.as_str()).unwrap_or(""),
            "ingested_at": chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = events
            .update_one(
                doc! {"session_id": session_id, "seq": seq},
                doc! {"$set": event_doc, "$setOnInsert": {"created_at": chrono::Utc::now().to_rfc3339()}},
            )
            .with_options(mongodb::options::UpdateOptions::builder().upsert(true).build())
            .await
        {
            tracing::error!("ingest: event upsert failed: {e}");
            return;
        }
    }

    let existing = sessions
        .find_one(doc! {"_id": session_id})
        .await
        .ok()
        .flatten();
    let prev_status = existing
        .as_ref()
        .and_then(|d| d.get_str("status").ok())
        .map(str::to_string);
    // 显式命名旗标：重命名/自动标题（session_meta.name）落地后，message_complete
    // 的首行标题推导不再覆盖用户起的名字
    let named = existing
        .as_ref()
        .and_then(|d| d.get_bool("named").ok())
        .unwrap_or(false);
    let status = derive_status(
        kind,
        envelope.get("payload").unwrap_or(&json!({})),
        existing.as_ref().and_then(|d| d.get_str("status").ok()),
    );
    let mut set = doc! {
        "user_id": user_id,
        "machine_id": machine_id,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    };
    if let Some(s) = &status {
        set.insert("status", s);
    }
    if kind == "message_complete" && !named {
        if let Some(text) = envelope
            .pointer("/payload/blocks/0/text")
            .and_then(|t| t.as_str())
        {
            set.insert("title", text.chars().take(60).collect::<String>());
        }
    }
    // session_meta 携带 cwd/model/命名/专家/分叉溯源（web 端项目分组、文件浏览、
    // 侧栏分叉与专家徽标依赖；桌面端回填时补发）
    if kind == "session_meta" {
        let empty_payload = json!({});
        let payload = envelope.get("payload").unwrap_or(&empty_payload);
        if let Some(cwd) = payload.get("cwd").and_then(|t| t.as_str()) {
            set.insert("cwd", cwd);
        }
        if let Some(m) = payload.get("model").and_then(|t| t.as_str()) {
            set.insert("model", m);
        }
        match payload.get("name").and_then(|t| t.as_str()) {
            Some(n) if !n.is_empty() => {
                set.insert("title", n.to_string());
                set.insert("named", true);
            }
            Some(_) => {
                // 清空名字 = 恢复默认标题：解除命名旗标，等下一条 message_complete 重推
                set.insert("named", false);
            }
            None => {}
        }
        for field in ["parent_session_id", "expert_id", "expert_name"] {
            if let Some(v) = payload.get(field).and_then(|t| t.as_str()) {
                set.insert(field, v.to_string());
            }
        }
    }
    // 回合文件变更快照：镜像为会话行状态（最新胜出），节点离线时 web 兜底
    // 展示。存 JSON 字符串与 session_events.payload 同风格；host 侧已裁剪，
    // 这里再设防御上限避免异常大载荷进 Mongo
    if kind == "file_changes" {
        let empty_payload = json!({});
        let payload = envelope.get("payload").unwrap_or(&empty_payload);
        let raw = serde_json::to_string(payload).unwrap_or_default();
        if raw.len() <= 128 * 1024 {
            set.insert("file_changes", raw);
        }
    }
    if let Err(e) = sessions
        .update_one(
            doc! {"_id": session_id},
            doc! {
                "$set": set.clone(),
                "$setOnInsert": {"created_at": chrono::Utc::now().to_rfc3339()},
            },
        )
        .with_options(
            mongodb::options::UpdateOptions::builder()
                .upsert(true)
                .build(),
        )
        .await
    {
        tracing::error!("ingest: session upsert failed: {e}");
        return;
    }

    // web 侧栏实时更新：session_upserted 控制帧（此前无人发送，web 只能靠 30s 轮询）。
    // 只在有可见变化时推：消息落库 / 元数据到达 / 状态翻转，快照与工具事件不推。
    let status_changed = match (&status, &prev_status) {
        (Some(s), Some(p)) => s != p,
        (Some(_), None) => true,
        _ => false,
    };
    if kind == "message_complete" || kind == "session_meta" || status_changed {
        let now = chrono::Utc::now().to_rfc3339();
        let get = |field: &str| -> String {
            if let Some(v) = set.get(field).and_then(|v| v.as_str()) {
                return v.to_string();
            }
            existing
                .as_ref()
                .and_then(|d| d.get_str(field).ok())
                .unwrap_or_default()
                .to_string()
        };
        let session = json!({
            "session_id": session_id,
            "title": get("title"),
            "status": status.clone().or(prev_status.clone()).unwrap_or_else(|| "idle".into()),
            "model": get("model"),
            "cwd": get("cwd"),
            "expert_id": get("expert_id"),
            "expert_name": get("expert_name"),
            "parent_session_id": get("parent_session_id"),
            "created_at": existing
                .as_ref()
                .and_then(|d| d.get_str("created_at").ok())
                .unwrap_or(&now),
            "updated_at": &now,
        });
        let frame = json!({
            "ctrl": "session_upserted",
            "machine_id": machine_id,
            "session": session,
        });
        state.publish_to_user(user_id, &frame.to_string()).await;
    }
}
