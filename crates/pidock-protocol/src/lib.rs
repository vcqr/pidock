//! PiDock unified wire protocol.
//!
//! Transport: newline-delimited JSON over stdio between the Tauri Rust core
//! and the pi-host daemon, and (message-level events) between desktop and the
//! PiDock server.
//!
//! Frames (one JSON object per line):
//! - core -> host:  [`Request`]  `{ id, method, params }`
//! - host -> core:  [`Response`] `{ id, ok, result?, error? }`
//! - host -> core:  [`Envelope`] `{ event_id, session_id, seq?, persist, ts, kind, payload }`
//!
//! Event kinds are open-ended strings; the authoritative payload schemas live
//! in `host/src/protocol.ts` (TS is the single source of truth for payloads).
//! The Rust side routes/stores envelopes without interpreting payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;
use uuid::Uuid;

/// core -> host request.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Request {
    pub id: Uuid,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// host -> core response to a [`Request`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Response {
    pub id: Uuid,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

/// Unified event envelope. `persist = true` events are append-only session
/// history (idempotency key = (session_id, seq, kind)) and get synced to the
/// cloud; ephemeral events are local/push only.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Envelope {
    pub event_id: Uuid,
    pub session_id: String,
    /// Anchor of persistable events: the pi session JSONL entry index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub persist: bool,
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub payload: Value,
}

/// host -> core frame (response or event).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HostFrame {
    Response(Response),
    Event(Envelope),
}

/// core -> host frame (request only in v1; no core-bound requests yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CoreFrame {
    Request(Request),
}

/// Large payloads are replaced by an attachment reference; full content lives
/// in object storage (RustFS/S3) keyed by content hash.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AttachmentRef {
    pub attachment_id: String,
    pub sha256: String,
    pub size: u64,
    pub preview: String,
    pub truncated: bool,
}

/// Payload size above which a value becomes an [`AttachmentRef`] (256 KiB).
pub const ATTACHMENT_THRESHOLD_BYTES: usize = 256 * 1024;
/// Preview text kept inline for attachment-ized payloads (16 KiB).
pub const ATTACHMENT_PREVIEW_BYTES: usize = 16 * 1024;

/// Persistable event kinds (anchored to JSONL entries, replayed on backfill).
pub mod event {
    pub const SESSION_META: &str = "session_meta";
    pub const MESSAGE_COMPLETE: &str = "message_complete";
    pub const COMPACTION_SUMMARY: &str = "compaction_summary";
    pub const SESSION_RESYNCED: &str = "session_resynced";
}

/// Ephemeral event kinds (push only, never persisted).
pub mod ephemeral {
    pub const MESSAGE_DELTA: &str = "message_delta";
    pub const MESSAGE_SNAPSHOT: &str = "message_snapshot";
    pub const TOOL_EXECUTION_START: &str = "tool_execution_start";
    pub const TOOL_EXECUTION_UPDATE: &str = "tool_execution_update";
    pub const TOOL_EXECUTION_END: &str = "tool_execution_end";
    pub const AGENT_STATE_CHANGED: &str = "agent_state_changed";
    pub const QUEUE_CHANGED: &str = "queue_changed";
    pub const AUTO_RETRY: &str = "auto_retry";
    pub const COMPACTION_LIFECYCLE: &str = "compaction_lifecycle";
    pub const TODO_UPDATED: &str = "todo_updated";
    pub const TOOL_APPROVAL: &str = "tool_approval";
    pub const ASK_USER_QUESTION: &str = "ask_user_question";
    pub const COMMAND_RESULT: &str = "command_result";
    /// 上下文用量推送（agent_settled/compaction_end/会话打开时由 host 发）
    pub const CONTEXT_USAGE: &str = "context_usage";
    pub const ERROR: &str = "error";
}

/// core -> host command methods（与 packages/protocol TS Method 全量同步，勿手改单边）
pub mod method {

    // ---- PING ----
    pub const PING: &str = "ping";

    // ---- SESSION ----
    pub const SESSION_CREATE: &str = "session.create";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_OPEN: &str = "session.open";
    pub const SESSION_CLOSE: &str = "session.close";
    pub const SESSION_EVENTS: &str = "session.events";
    pub const SESSION_RENAME: &str = "session.rename";
    pub const SESSION_FORK: &str = "session.fork";
    pub const SESSION_REMOVE: &str = "session.remove";

    // ---- MODEL ----
    pub const MODEL_OVERRIDE_SET: &str = "config.model_override.set";

    // ---- WORKSPACE ----
    pub const WORKSPACE_FILES: &str = "workspace.files";
    pub const WORKSPACE_READ_FILE: &str = "workspace.read_file";

    // ---- SESSION ----
    pub const SESSION_SET_PERMISSION_MODE: &str = "session.set_permission_mode";
    pub const SESSION_RESOLVE_APPROVAL: &str = "session.resolve_approval";
    pub const SESSION_RESOLVE_ASK: &str = "session.resolve_ask";
    pub const SESSION_SET_THINKING_LEVEL: &str = "session.set_thinking_level";
    pub const SESSION_SET_MODEL: &str = "session.set_model";
    pub const SESSION_FILE_CHANGES: &str = "session.file_changes";
    pub const SESSION_FILE_DIFF: &str = "session.file_diff";
    pub const SESSION_REVERT_FILES: &str = "session.revert_files";

    // ---- AGENT ----
    pub const AGENT_PROMPT: &str = "agent.prompt";
    pub const AGENT_STEER: &str = "agent.steer";
    pub const AGENT_FOLLOW_UP: &str = "agent.follow_up";
    pub const AGENT_ABORT: &str = "agent.abort";
    pub const AGENT_COMPACT: &str = "agent.compact";
    pub const AGENT_CONTEXT_USAGE: &str = "agent.context_usage";
    pub const AGENT_CLEAR_QUEUE: &str = "agent.clear_queue";
    pub const AGENT_ABORT_RETRY: &str = "agent.abort_retry";

    // ---- SESSION ----
    pub const SESSION_EXPORT: &str = "session.export";
    pub const SESSION_SET_AUTO_COMPACTION: &str = "session.set_auto_compaction";
    pub const SESSION_SET_AUTO_RETRY: &str = "session.set_auto_retry";
    pub const SESSION_LIST_TOOLS: &str = "session.list_tools";
    pub const SESSION_SET_ACTIVE_TOOLS: &str = "session.set_active_tools";
    pub const SESSION_TREE: &str = "session.tree";
    pub const SESSION_NAVIGATE_TREE: &str = "session.navigate_tree";

    // ---- CONFIG ----
    pub const CONFIG_PROMPTS_LIST: &str = "config.prompts.list";

    // ---- SESSION ----
    pub const SESSION_THINKING_INFO: &str = "session.thinking_info";
    pub const SESSION_TRUST: &str = "session.trust";
    pub const SESSION_PENDING: &str = "session.pending";

    // ---- CONFIG ----
    pub const CONFIG_GET: &str = "config.get";
    pub const CONFIG_SETTINGS_SET: &str = "config.settings.set";
    pub const CONFIG_PROVIDERS_LIST: &str = "config.providers.list";
    pub const CONFIG_PROVIDER_SET_KEY: &str = "config.providers.set_key";
    pub const CONFIG_PROVIDER_REMOVE_KEY: &str = "config.providers.remove_key";
    pub const CONFIG_MODELS_LIST: &str = "config.models.list";
    pub const CONFIG_MODELS_SET_DEFAULT: &str = "config.models.set_default";
    pub const CONFIG_RELOAD_RUNTIME: &str = "config.reload_runtime";
    pub const CONFIG_EXTENSIONS_LIST: &str = "config.extensions.list";
    pub const CONFIG_EXTENSIONS_TOGGLE: &str = "config.extensions.toggle";
    pub const CONFIG_EXTENSION_READ: &str = "config.extensions.read";
    pub const CONFIG_SKILLS_LIST: &str = "config.skills.list";
    pub const CONFIG_SKILLS_TOGGLE: &str = "config.skills.toggle";
    pub const CONFIG_SKILL_FILES: &str = "config.skills.files";
    pub const CONFIG_SKILL_READ: &str = "config.skills.read";
    pub const CONFIG_SKILLS_INSTALL: &str = "config.skills.install";
    pub const CONFIG_EXTENSIONS_INSTALL: &str = "config.extensions.install";
    pub const CONFIG_PROVIDERS_CUSTOM_GET: &str = "config.providers.custom.get";
    pub const CONFIG_PROVIDERS_CUSTOM_SET: &str = "config.providers.custom.set";
    pub const CONFIG_PROVIDERS_CUSTOM_REMOVE: &str = "config.providers.custom.remove";
    pub const CONFIG_PROVIDERS_FETCH_MODELS: &str = "config.providers.fetch_models";
    pub const CONFIG_MCP_GET: &str = "config.mcp.get";
    pub const CONFIG_MCP_SET: &str = "config.mcp.set";
    pub const CONFIG_AGENTS_READ: &str = "config.agents.read";
    pub const CONFIG_AGENTS_WRITE: &str = "config.agents.write";

    // ---- STATS ----
    pub const STATS_USAGE: &str = "stats.usage";

    // ---- PIDOCK ----
    pub const PIDOCK_SETTINGS_GET: &str = "pidock.settings.get";
    pub const PIDOCK_SETTINGS_SET: &str = "pidock.settings.set";

    // ---- ATTACHMENT ----
    pub const ATTACHMENT_GET: &str = "attachment.get";

    // ---- EXPERTS ----
    pub const EXPERTS_LIST: &str = "experts.list";
    pub const EXPERTS_GET: &str = "experts.get";
    pub const EXPERTS_SAVE: &str = "experts.save";
    pub const EXPERTS_DELETE: &str = "experts.delete";
    pub const EXPERTS_INSTALL_RESOURCE: &str = "experts.install_resource";
    pub const EXPERTS_PRIVATE_LIST: &str = "experts.private_list";
    pub const EXPERTS_REMOVE_RESOURCE: &str = "experts.remove_resource";
    pub const EXPERTS_READ_AVATAR_FILE: &str = "experts.read_avatar_file";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_untagged_dispatch() {
        let resp: HostFrame =
            serde_json::from_str(r#"{"id":"0192b4a1-7c2f-7000-8000-000000000000","ok":true,"result":1}"#).unwrap();
        assert!(matches!(resp, HostFrame::Response(_)));

        let ev: HostFrame = serde_json::from_str(
            r#"{"event_id":"0192b4a1-7c2f-7000-8000-000000000001","session_id":"s1","persist":false,"ts":"2026-09-29T08:00:00Z","kind":"message_delta","payload":{}}"#,
        )
        .unwrap();
        assert!(matches!(ev, HostFrame::Event(_)));
    }

    #[test]
    fn ts_bindings_exported() {
        use ts_rs::TS;
        let out = "../../host/src/types/generated";
        Request::export_all_to(out).unwrap();
        Response::export_all_to(out).unwrap();
        RpcError::export_all_to(out).unwrap();
        Envelope::export_all_to(out).unwrap();
        AttachmentRef::export_all_to(out).unwrap();
    }
}
