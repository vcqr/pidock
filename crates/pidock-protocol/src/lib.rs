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
    pub const APPROVAL_REQUEST: &str = "approval_request";
    pub const COMMAND_RESULT: &str = "command_result";
    pub const ERROR: &str = "error";
}

/// core -> host command methods (v1 set).
pub mod method {
    pub const PING: &str = "ping";
    pub const SESSION_CREATE: &str = "session.create";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_OPEN: &str = "session.open";
    pub const SESSION_CLOSE: &str = "session.close";
    pub const SESSION_EVENTS: &str = "session.events";
    pub const AGENT_PROMPT: &str = "agent.prompt";
    pub const AGENT_STEER: &str = "agent.steer";
    pub const AGENT_FOLLOW_UP: &str = "agent.follow_up";
    pub const AGENT_ABORT: &str = "agent.abort";
    pub const CONFIG_GET: &str = "config.get";
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
