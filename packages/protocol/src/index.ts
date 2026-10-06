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
      type: "image";
      /** base64，不带 data: 前缀 */
      data: string;
      mime: string;
    }
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

/** TodoWrite 工具提交的单条任务（LLM 参数 activeForm 归一为 snake_case） */
export interface TodoItem {
  content: string;
  status: "pending" | "in_progress" | "completed";
  active_form?: string;
}

/** 会话当前任务清单快照（全量替换，非增量） */
export interface TodoUpdatedPayload {
  todos: TodoItem[];
}

/** AskUserQuestion 的单个选项 */
export interface AskQuestionOption {
  label: string;
  description?: string;
}

/** AskUserQuestion 工具的单道问题（LLM 参数结构，host 归一化后下发） */
export interface AskQuestion {
  header: string;
  question: string;
  options: AskQuestionOption[];
  multiSelect?: boolean;
  /** 模型自荐的选项下标：卡片标「推荐」；full 权限模式下 host 直接采用它作答 */
  recommended?: number;
}

/** AskUserQuestion 提问事件载荷（一次一题，逐题作答） */
export interface AskUserQuestionPayload {
  ask_id: string;
  question: AskQuestion;
  /** 等待超时（秒），UI 据此显示倒计时；0/缺省 = 不限时 */
  timeout_sec?: number;
}

export interface ErrorPayload {
  message: string;
  fatal?: boolean;
}

// ---------------------------------------------------------------------------
// experts（专家智能体编排）
// ---------------------------------------------------------------------------

/** 专家 = 预编排的智能体档案：角色提示词 + 技能/工具白名单 + 知识库 + 默认参数 */
export interface Expert {
  id: string;
  /** 显示名，同时是 /expert:name 的调用名（唯一） */
  name: string;
  description?: string;
  /** remixicon 图标名（可选，未上传头像时的字形头像） */
  icon?: string;
  /** 自定义头像（data URL，host 缩存校验；设了它就不渲染字形头像） */
  avatar?: string;
  /** 字形头像的底色（#rrggbb；缺省用主题灰） */
  avatar_color?: string;
  /** 角色定位提示词，以 append 方式叠加在 pi 默认系统提示词之后 */
  prompt: string;
  /** 全局技能名白名单；空数组 = 不限制（全部可用） */
  skills: string[];
  /** 全局插件（含 MCP extension）文件名白名单（不含扩展名）；空数组 = 不限制 */
  extensions: string[];
  /** 内置工具白名单（read/bash/edit/write…）；空/缺省 = 不限制 */
  tools?: string[];
  /** 内置工具黑名单，在白名单之后生效 */
  exclude_tools?: string[];
  /** 知识库目录：会话内注入文件清单，模型按需用 read 读取 */
  knowledge_dirs: string[];
  /** 会话默认模型（provider/model-id）与思考级别 */
  model?: string;
  thinking_level?: string;
  /** 会话建议权限模式（缺省走全局默认 plan） */
  permission_mode?: "plan" | "confirm" | "full";
  created_at: string;
  updated_at: string;
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
  TODO_UPDATED: "todo_updated",
  TOOL_APPROVAL: "tool_approval",
  ASK_USER_QUESTION: "ask_user_question",
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
  SESSION_RENAME: "session.rename",
  /** 从注册表移除会话（UI 列表消失；磁盘 JSONL 保留） */
  SESSION_REMOVE: "session.remove",
  MODEL_OVERRIDE_SET: "config.model_override.set",
  WORKSPACE_FILES: "workspace.files",
  /** 读取工作区内文本文件（路径安全校验 + 大小上限 + 二进制检测），文件浏览预览用 */
  WORKSPACE_READ_FILE: "workspace.read_file",
  SESSION_SET_PERMISSION_MODE: "session.set_permission_mode",
  SESSION_RESOLVE_APPROVAL: "session.resolve_approval",
  SESSION_RESOLVE_ASK: "session.resolve_ask",
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
  CONFIG_SKILLS_INSTALL: "config.skills.install",
  CONFIG_EXTENSIONS_INSTALL: "config.extensions.install",
  CONFIG_PROVIDERS_CUSTOM_GET: "config.providers.custom.get",
  CONFIG_PROVIDERS_CUSTOM_SET: "config.providers.custom.set",
  CONFIG_PROVIDERS_CUSTOM_REMOVE: "config.providers.custom.remove",
  CONFIG_PROVIDERS_FETCH_MODELS: "config.providers.fetch_models",
  CONFIG_MCP_GET: "config.mcp.get",
  CONFIG_MCP_SET: "config.mcp.set",
  CONFIG_AGENTS_READ: "config.agents.read",
  CONFIG_AGENTS_WRITE: "config.agents.write",
  STATS_USAGE: "stats.usage",
  PIDOCK_SETTINGS_GET: "pidock.settings.get",
  PIDOCK_SETTINGS_SET: "pidock.settings.set",
  ATTACHMENT_GET: "attachment.get",
  EXPERTS_LIST: "experts.list",
  EXPERTS_GET: "experts.get",
  EXPERTS_SAVE: "experts.save",
  EXPERTS_DELETE: "experts.delete",
  EXPERTS_INSTALL_RESOURCE: "experts.install_resource",
  EXPERTS_PRIVATE_LIST: "experts.private_list",
  EXPERTS_REMOVE_RESOURCE: "experts.remove_resource",
  EXPERTS_READ_AVATAR_FILE: "experts.read_avatar_file",
} as const;

// payload size limits (mirrors pidock-protocol Rust crate)
export const ATTACHMENT_THRESHOLD_BYTES = 256 * 1024;
export const ATTACHMENT_PREVIEW_BYTES = 16 * 1024;
/** args/partial previews kept inline on ephemeral events */
export const INLINE_PREVIEW_BYTES = 4 * 1024;
