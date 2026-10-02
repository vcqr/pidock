/**
 * PiDock wire protocol — TS side (authoritative for event/command payloads).
 *
 * Transport frames are newline-delimited JSON over stdio:
 * - core -> host:  { id, method, params }
 * - host -> core:  { id, ok, result?, error? }            (response)
 * - host -> core:  { event_id, session_id, seq?, persist, ts, kind, payload }
 *
 * `persist: true` events are anchored to pi session JSONL entries
 * (idempotency key = (session_id, seq, kind)) and get synced to the cloud.
 * Ephemeral events drive local UI / live push only.
 */

export interface Request {
  id: string;
  method: string;
  params?: unknown;
}

export interface RpcError {
  code: string;
  message: string;
}

export interface Response {
  id: string;
  ok: boolean;
  result?: unknown;
  error?: RpcError;
}

export interface Envelope {
  event_id: string;
  session_id: string;
  /** 1-based index of the pi session JSONL entry this event was derived from. */
  seq?: number;
  persist: boolean;
  ts: string;
  kind: string;
  payload: unknown;
}

// ---------------------------------------------------------------------------
// persistable payload schemas
// ---------------------------------------------------------------------------

export type Block =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string }
  | {
      type: "toolCall";
      callId: string;
      toolName: string;
      /** arguments JSON string, possibly attachment-ized */
      args: string;
      attachment?: AttachmentRef;
    }
  | {
      type: "toolResult";
      callId: string;
      toolName: string;
      isError: boolean;
      /** output text, possibly attachment-ized */
      output: string;
      attachment?: AttachmentRef;
    };

export interface UsageInfo {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
  costTotal: number;
}

export interface MessageCompletePayload {
  role: "user" | "assistant" | "toolResult";
  blocks: Block[];
  provider?: string;
  model?: string;
  usage?: UsageInfo;
  stopReason?: string;
  errorMessage?: string;
  /** stable pi entry id (also carried in the JSONL) */
  entry_id: string;
  /** streaming identity: matches the id used on message_delta / message_snapshot */
  message_id?: string;
}

export interface SessionMetaPayload {
  /** present on create/open */
  cwd?: string;
  provider?: string;
  model?: string;
  /** present on session_info entries */
  name?: string;
  thinking_level?: string;
  entry_id?: string;
}

export interface CompactionSummaryPayload {
  summary: string;
  tokens_before?: number;
  entry_id: string;
}

export interface SessionResyncedPayload {
  reason: string;
}

export interface AttachmentRef {
  attachment_id: string;
  sha256: string;
  size: number;
  preview: string;
  truncated: boolean;
}

// ---------------------------------------------------------------------------
// ephemeral payload schemas
// ---------------------------------------------------------------------------

export type AgentState =
  | "idle"
  | "thinking"
  | "responding"
  | "executing_tool"
  | "compacting"
  | "retrying";

export interface MessageDeltaPayload {
  message_id: string;
  part: "text" | "thinking";
  delta: string;
}

export interface MessageSnapshotPayload {
  message_id: string;
  text: string;
  thinking: string;
}

export interface ToolExecutionStartPayload {
  call_id: string;
  tool_name: string;
  args: string;
  attachment?: AttachmentRef;
}

export interface ToolExecutionUpdatePayload {
  call_id: string;
  tool_name: string;
  partial: string;
  attachment?: AttachmentRef;
}

export interface ToolExecutionEndPayload {
  call_id: string;
  tool_name: string;
  is_error: boolean;
  output: string;
  attachment?: AttachmentRef;
  duration_ms?: number;
}

export interface AgentStateChangedPayload {
  state: AgentState;
}

export interface QueueChangedPayload {
  steering_count: number;
  follow_up_count: number;
}

export interface AutoRetryPayload {
  phase: "start" | "end";
  attempt?: number;
  max_attempts?: number;
  error?: string;
  success?: boolean;
}

export interface CompactionLifecyclePayload {
  phase: "start" | "end";
  reason?: string;
  aborted?: boolean;
  error?: string;
}

export interface ErrorPayload {
  message: string;
  fatal?: boolean;
}

// ---------------------------------------------------------------------------
// event kinds
// ---------------------------------------------------------------------------

export const Event = {
  // persistable
  SESSION_META: "session_meta",
  MESSAGE_COMPLETE: "message_complete",
  COMPACTION_SUMMARY: "compaction_summary",
  SESSION_RESYNCED: "session_resynced",
  // ephemeral
  MESSAGE_DELTA: "message_delta",
  MESSAGE_SNAPSHOT: "message_snapshot",
  TOOL_EXECUTION_START: "tool_execution_start",
  TOOL_EXECUTION_UPDATE: "tool_execution_update",
  TOOL_EXECUTION_END: "tool_execution_end",
  AGENT_STATE_CHANGED: "agent_state_changed",
  QUEUE_CHANGED: "queue_changed",
  AUTO_RETRY: "auto_retry",
  COMPACTION_LIFECYCLE: "compaction_lifecycle",
  TOOL_APPROVAL: "tool_approval",
  ERROR: "error",
} as const;

// ---------------------------------------------------------------------------
// methods
// ---------------------------------------------------------------------------

export const Method = {
  PING: "ping",
  SESSION_CREATE: "session.create",
  SESSION_LIST: "session.list",
  SESSION_OPEN: "session.open",
  SESSION_CLOSE: "session.close",
  SESSION_EVENTS: "session.events",
  SESSION_SET_PERMISSION_MODE: "session.set_permission_mode",
  SESSION_RESOLVE_APPROVAL: "session.resolve_approval",
  SESSION_SET_THINKING_LEVEL: "session.set_thinking_level",
  SESSION_SET_MODEL: "session.set_model",
  SESSION_FILE_CHANGES: "session.file_changes",
  SESSION_FILE_DIFF: "session.file_diff",
  SESSION_REVERT_FILES: "session.revert_files",
  AGENT_PROMPT: "agent.prompt",
  AGENT_STEER: "agent.steer",
  AGENT_FOLLOW_UP: "agent.follow_up",
  AGENT_ABORT: "agent.abort",
  CONFIG_GET: "config.get",
  CONFIG_SETTINGS_SET: "config.settings.set",
  CONFIG_PROVIDERS_LIST: "config.providers.list",
  CONFIG_PROVIDER_SET_KEY: "config.providers.set_key",
  CONFIG_PROVIDER_REMOVE_KEY: "config.providers.remove_key",
  CONFIG_MODELS_LIST: "config.models.list",
  CONFIG_MODELS_SET_DEFAULT: "config.models.set_default",
  CONFIG_EXTENSIONS_LIST: "config.extensions.list",
  CONFIG_EXTENSIONS_TOGGLE: "config.extensions.toggle",
  CONFIG_EXTENSION_READ: "config.extensions.read",
  CONFIG_SKILLS_LIST: "config.skills.list",
  CONFIG_SKILLS_TOGGLE: "config.skills.toggle",
  CONFIG_SKILL_FILES: "config.skills.files",
  CONFIG_SKILL_READ: "config.skills.read",
  CONFIG_PROVIDERS_CUSTOM_GET: "config.providers.custom.get",
  CONFIG_PROVIDERS_CUSTOM_SET: "config.providers.custom.set",
  CONFIG_PROVIDERS_CUSTOM_REMOVE: "config.providers.custom.remove",
  CONFIG_PROVIDERS_FETCH_MODELS: "config.providers.fetch_models",
  CONFIG_MCP_GET: "config.mcp.get",
  CONFIG_MCP_SET: "config.mcp.set",
  ATTACHMENT_GET: "attachment.get",
} as const;

// payload size limits (mirrors pidock-protocol Rust crate)
export const ATTACHMENT_THRESHOLD_BYTES = 256 * 1024;
export const ATTACHMENT_PREVIEW_BYTES = 16 * 1024;
/** args/partial previews kept inline on ephemeral events */
export const INLINE_PREVIEW_BYTES = 4 * 1024;
