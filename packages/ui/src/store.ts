import { reactive, ref, watch } from "vue";
import type { DataBus } from "./databus.js";
import type { AskQuestion, TodoItem, PendingApprovalInfo, PendingAskInfo } from "@pidock/protocol";
import { appConfirm } from "./confirm.js";

/**
 * Reactive agent store consuming DataBus events.
 *
 * Merging rules:
 * - `message_delta` / `message_snapshot` (delta channel) append/replace the
 *   streaming assistant item keyed by turn id.
 * - `message_complete` (persisted) finalizes the streaming item when
 *   `message_id` matches, and appends canonical user/toolResult messages.
 * - Live tool cards come from `tool_execution_*`; they are replaced by the
 *   canonical `toolResult` message when it arrives.
 * - History (loadHistory) replays persisted events only — no live cards.
 */

export interface UiBlock {
  type: string;
  text?: string;
  thinking?: string;
  callId?: string;
  toolName?: string;
  args?: string;
  output?: string;
  isError?: boolean;
  /** 图片块（用户消息随附，base64 不带前缀） */
  data?: string;
  mime?: string;
  attachment?: { attachment_id: string; size: number; preview: string; truncated: boolean };
}

export interface UiMessageItem {
  kind: "message";
  key: string;
  role: "user" | "assistant" | "toolResult";
  text: string;
  thinking: string;
  blocks: UiBlock[] | null;
  streaming: boolean;
  pending?: boolean;
  /** 思考流开始时间（streaming 计时用），结束写回 thinkingMs */
  thinkingStartedAt?: number;
  thinkingMs?: number;
  /** 消息时间戳（历史回放来自 session 条目），用于回合耗时统计 */
  ts?: string;
  /** 回合 id（本条用户消息的 session 条目官方 id，回合分组/变更卡片聚合键） */
  turnId?: string;
  /** 消息的 session 条目官方 id（用户消息分叉锚点） */
  entryId?: string;
  /** 助手消息 token 用量与花费（message_complete 携带，元信息条展示） */
  usage?: { input: number; output: number; cacheRead: number; cacheWrite: number; totalTokens: number; costTotal: number };
  /** 助手消息模型（provider/model-id） */
  model?: string;
  /** 助手消息生成耗时（毫秒，host 从条目时间戳差值计算） */
  durationMs?: number;
  /** 助手消息停止原因（length=截断 / aborted=中止，异常值在元信息条出角标） */
  stopReason?: string;
  /** 用户消息随附图片（data URL，发送时展示用） */
  imageUrls?: string[];
  /** 用户消息随附文档附件（仅名称与大小，乐观气泡展示用；全文由 host 注入消息文本） */
  attachmentNames?: Array<{ name: string; size?: number }>;
  /** 助手消息错误（模型/供应商返回的 errorMessage，空回复时也要可见） */
  errorMessage?: string;
}

export interface UiToolItem {
  kind: "tool";
  key: string;
  callId: string;
  toolName: string;
  status: "running" | "done" | "error";
  args: string;
  partial: string;
  output: string;
}

export type UiItem = UiMessageItem | UiToolItem;

/** 会话待用户处理的交互（审批 / AskUserQuestion 提问），见 pendingBySession */
export interface PendingInteraction {
  kind: "approval" | "ask";
  approvalId?: string;
  askId?: string;
  toolName?: string;
  args?: string;
  question?: AskQuestion;
  timeoutSec?: number;
}

export interface SessionSummaryUi {
  session_id: string;
  file: string;
  cwd: string;
  provider?: string;
  model?: string;
  name?: string;
  /** 创建时雇佣的专家（重开由 host 按注册表重放） */
  expert_id?: string;
  expert_name?: string;
  /** 分叉来源会话（fork 产生的会话带父会话 id，侧栏出分叉标识） */
  parent_session_id?: string;
  created_at: string;
  updated_at?: string;
  open: boolean;
  state: string;
}

/** 专家档案（第三栏信息卡 / 雇佣流程共用） */
export interface ExpertInfo {
  id: string;
  name: string;
  description?: string;
  icon?: string;
  avatar?: string;
  avatar_color?: string;
  prompt: string;
  skills: string[];
  extensions: string[];
  exclude_tools?: string[];
  knowledge_dirs: string[];
}

let keyCounter = 0;
const nextKey = (): string => `k${++keyCounter}`;

/** config.models.list 返回的单个模型（全量可用模型，来自已配置供应商） */
export interface ModelInfo {
  provider: string;
  id: string;
  name: string;
  reasoning: boolean;
  /** 模型输入能力（["text"] 或 ["text","image"]） */
  input?: string[];
  contextWindow?: number;
  maxTokens?: number;
}

/** 工具调用权限模式（与 host pool.ts 保持一致） */
export type PermissionMode = "plan" | "confirm" | "edit-auto" | "full";

/** 上下文用量（host context_usage 事件 / agent.context_usage RPC 载荷） */
export interface ContextUsageInfo {
  /** 估算上下文 token（刚压缩完/尚未响应时为 null） */
  tokens: number | null;
  percent: number | null;
  context_window: number | null;
  auto_compaction?: boolean;
  auto_retry?: boolean;
  steering_count?: number;
  follow_up_count?: number;
  stats?: {
    user_messages: number;
    assistant_messages: number;
    tool_calls: number;
    tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number };
    cost: number;
  };
}

/** 会话工具清单项（session.list_tools） */
export interface ToolInfoUi {
  name: string;
  description: string;
  active: boolean;
  namespace: string | null;
  source: string;
}

/** 思考能力（session.thinking_info） */
export interface ThinkingInfoUi {
  supports: boolean;
  levels: string[];
  current: string;
}

/** 会话分支树的用户消息节点（session.tree） */
export interface TreeNodeUi {
  id: string;
  parent_id: string | null;
  text: string;
  ts: string;
}

/**
 * Push an item and return its REACTIVE proxy. Mutating the raw object that
 * was pushed would bypass Vue's proxies and never re-render the UI.
 */
function pushItem(items: UiItem[], item: UiItem): UiItem {
  items.push(item);
  return items[items.length - 1] as UiItem;
}

export function createAgentStore(bus: DataBus) {
  const sessions = ref<SessionSummaryUi[]>([]);
  const activeId = ref<string | null>(null);
  const items = ref<UiItem[]>([]);
  const agentState = ref<string>("idle");
  const lastError = ref<string | null>(null);
  const loadingHistory = ref(false);
  /** host 主目录（session.list 下发）；null = 未知（web 快照），侧栏不做项目/任务拆分 */
  const homeDir = ref<string | null>(null);
  /** 每会话权限模式（缺省 plan = 计划模式，与 host 新会话默认一致） */
  const permissionModes = ref<Record<string, PermissionMode>>({});
  /** 每会话思考级别（缺省 medium） */
  const thinkingLevels = ref<Record<string, string>>({});
  /** 待用户审批的工具调用（仅活动会话展示） */
  const pendingApproval = ref<{ approvalId: string; toolName: string; args: string } | null>(null);
  /** 待用户回答的 AskUserQuestion 问题卡（仅活动会话展示；timeoutSec=倒计时秒数，0/缺省不限时） */
  const pendingAsk = ref<{ askId: string; question: AskQuestion; timeoutSec?: number } | null>(null);
  /** 每会话待处理交互（审批/提问），不分活动/后台——后台会话的等待不再被
   * activeId 过滤器丢弃，切会话回来时审批卡/问题卡能从中恢复；
   * host 对离开 waiting 状态会补发 agent_state_changed，据此清除 */
  const pendingBySession = ref<Record<string, PendingInteraction>>({});
  /** 当前回合开始时间；结束后保留并写入 turnEndedAt（「已工作 · 耗时」折叠行用） */
  const turnStartedAt = ref<number | null>(null);
  const turnEndedAt = ref<number | null>(null);
  /** 各回合的文件变更（按轮累积，turnId = 回合官方 id），回合结束后拉取 */
  const turnFileChanges = ref<Array<{ turnId: string; files: Array<{ path: string; added: number; removed: number; isNew: boolean }> }>>([]);
  /** 全量可用模型（已配置供应商，config.models.list），供模型下拉选择 */
  const allModels = ref<ModelInfo[]>([]);
  /**
   * 每会话任务清单（TodoWrite 工具推送，全量快照）。
   * 按 session_id 存、不走 activeId 过滤：openSession 时 host 补发的恢复
   * 事件可能先于 activeId 赋值到达，按活动会话过滤会把它丢掉。
   */
  const todosBySession = ref<Record<string, TodoItem[]>>({});
  /** 每会话上下文用量（context_usage 事件 + agent.context_usage RPC） */
  const contextBySession = ref<Record<string, ContextUsageInfo>>({});
  /** 每会话排队消息计数（queue_changed 事件；context_usage 载荷同样携带） */
  const queueBySession = ref<Record<string, { steering: number; follow_up: number }>>({});
  /** 每会话思考能力（session.thinking_info，openSession 时拉取） */
  const thinkingBySession = ref<Record<string, ThinkingInfoUi>>({});
  /** 每会话自动重试状态（auto_retry 事件；非 null = 重试中），对话顶部提示条 */
  const autoRetryBySession = ref<
    Record<string, { attempt?: number; max_attempts?: number; error?: string } | null>
  >({});
  /** 每会话压缩进行中（compaction_lifecycle start/end），上下文条显示进度态 */
  const compactingBySession = ref<Record<string, boolean>>({});
  /** 每会话最近一次压缩摘要（compaction_summary 落库事件），对话顶部可展开查看 */
  const compactionBySession = ref<Record<string, { summary: string; tokens_before?: number }>>({});

  let liveTools = new Map<string, UiToolItem>();
  let streaming: UiMessageItem | null = null;

  /** 浅拷贝并移除一个 key（按会话清理缓存 map 用） */
  function withoutKey<T>(rec: Record<string, T>, sid: string): Record<string, T> {
    const next = { ...rec };
    delete next[sid];
    return next;
  }

  function ensureStreaming(messageId: string): UiMessageItem {
    if (streaming && streaming.key === messageId) return streaming;
    streaming = pushItem(items.value, {
      kind: "message",
      key: messageId,
      role: "assistant",
      text: "",
      thinking: "",
      blocks: null,
      streaming: true,
    }) as UiMessageItem;
    return streaming;
  }

  function asBlocks(payload: any): UiBlock[] {
    return Array.isArray(payload?.blocks) ? payload.blocks : [];
  }

  function applyMessageComplete(p: any, ts?: string): void {
    const itemTs = ts ?? new Date().toISOString();
    const role = p?.role;
    if (role === "assistant") {
      const meta = {
        ...(p.usage
          ? {
              usage: {
                input: Number(p.usage.input) || 0,
                output: Number(p.usage.output) || 0,
                cacheRead: Number(p.usage.cacheRead) || 0,
                cacheWrite: Number(p.usage.cacheWrite) || 0,
                totalTokens: Number(p.usage.totalTokens) || 0,
                costTotal: Number(p.usage.costTotal) || 0,
              },
            }
          : {}),
        ...(p.model ? { model: String(p.model) } : {}),
        ...(Number(p.duration_ms) > 0 ? { durationMs: Number(p.duration_ms) } : {}),
        ...(p.stopReason ? { stopReason: String(p.stopReason) } : {}),
      };
      if (streaming && p.message_id && streaming.key === p.message_id) {
        streaming.blocks = asBlocks(p);
        streaming.ts = itemTs;
        streaming.streaming = false;
        streaming.errorMessage = p.errorMessage || undefined;
        Object.assign(streaming, meta);
        streaming = null;
        return;
      }
      // history replay or missed stream: canonical item
      const blocks = asBlocks(p);
      const hasText = !!firstText(p);
      if (!blocks.length && !hasText && !p.errorMessage) return; // 空回复（回合尾部空消息）不生成条目
      items.value.push({
        kind: "message",
        key: nextKey(),
        role: "assistant",
        text: "",
        thinking: "",
        blocks,
        streaming: false,
        ts: itemTs,
        ...(p.errorMessage ? { errorMessage: p.errorMessage } : {}),
        ...meta,
      });
      return;
    }
    if (role === "user") {
      // confirm the optimistic copy if present (same text, still pending)
      const pendingUser = items.value.find(
        (it): it is UiMessageItem =>
          it.kind === "message" && it.role === "user" && it.pending === true && it.text === firstText(p),
      );
      const imageUrls = asBlocks(p)
        .filter((b) => b.type === "image")
        .map((b) => `data:${(b as any).mime || "image/png"};base64,${(b as any).data}`);
      if (pendingUser) {
        pendingUser.pending = false;
        pendingUser.turnId = p.entry_id || pendingUser.turnId;
        pendingUser.entryId = p.entry_id || pendingUser.entryId;
        if (imageUrls.length) pendingUser.imageUrls = imageUrls;
        if (!pendingUser.ts) pendingUser.ts = itemTs;
        return;
      }
      pushItem(items.value, {
        kind: "message",
        key: p.entry_id ?? nextKey(),
        role: "user",
        text: firstText(p) ?? "",
        thinking: "",
        blocks: null,
        streaming: false,
        ts: itemTs,
        turnId: p.entry_id || undefined,
        entryId: p.entry_id || undefined,
        ...(imageUrls.length ? { imageUrls } : {}),
      });
      return;
    }
    if (role === "toolResult") {
      const callId = firstBlock(p, "toolResult")?.callId;
      if (callId) {
        const idx = items.value.findIndex((it) => it.kind === "tool" && it.callId === callId);
        if (idx >= 0) items.value.splice(idx, 1);
      }
      // AskUserQuestion 工具出结果（含取消/打断）→ 问题卡使命完成
      if (firstBlock(p, "toolResult")?.toolName === "AskUserQuestion") pendingAsk.value = null;
      items.value.push({
        kind: "message",
        key: nextKey(),
        role: "toolResult",
        text: "",
        thinking: "",
        blocks: asBlocks(p),
        streaming: false,
        ts: itemTs,
      });
    }
  }

  function firstText(p: any): string | undefined {
    const t = (asBlocks(p).find((b) => b.type === "text")?.text as string) ?? undefined;
    return t;
  }

  function firstBlock(p: any, type: string): UiBlock | undefined {
    return asBlocks(p).find((b) => b.type === type);
  }

  function applyEnvelope(e: { session_id: string; kind: string; payload: any }): void {
    if (e.kind === "todo_updated") {
      const todos = Array.isArray(e.payload?.todos) ? (e.payload.todos as TodoItem[]) : [];
      todosBySession.value = { ...todosBySession.value, [e.session_id]: todos };
      return;
    }
    if (e.kind === "context_usage") {
      const info = e.payload as ContextUsageInfo;
      contextBySession.value = { ...contextBySession.value, [e.session_id]: info };
      // context_usage 载荷同时携带排队计数（host 单一数据源）
      queueBySession.value = {
        ...queueBySession.value,
        [e.session_id]: {
          steering: Number(info?.steering_count) || 0,
          follow_up: Number(info?.follow_up_count) || 0,
        },
      };
      return;
    }
    if (e.kind === "queue_changed") {
      queueBySession.value = {
        ...queueBySession.value,
        [e.session_id]: {
          steering: Number(e.payload?.steering_count) || 0,
          follow_up: Number(e.payload?.follow_up_count) || 0,
        },
      };
      return;
    }
    if (e.kind === "session_settings_changed") {
      // 会话级设置变化（权限模式/思考级别）：web 改 → 桌面跟随，反向亦然
      const p = e.payload ?? {};
      if (e.session_id && typeof p.permission_mode === "string" && p.permission_mode) {
        permissionModes.value = { ...permissionModes.value, [e.session_id]: p.permission_mode as PermissionMode };
      }
      if (e.session_id && typeof p.thinking_level === "string" && p.thinking_level) {
        thinkingLevels.value = { ...thinkingLevels.value, [e.session_id]: p.thinking_level };
      }
      return;
    }
    if (e.kind === "session_removed") {
      // 会话已从 host 注册表移除（本端或另一端发起）：实时摘除，不等轮询
      const sid = e.session_id;
      sessions.value = sessions.value.filter((s) => s.session_id !== sid);
      todosBySession.value = withoutKey(todosBySession.value, sid);
      queueBySession.value = withoutKey(queueBySession.value, sid);
      contextBySession.value = withoutKey(contextBySession.value, sid);
      thinkingBySession.value = withoutKey(thinkingBySession.value, sid);
      pendingBySession.value = withoutKey(pendingBySession.value, sid);
      autoRetryBySession.value = withoutKey(autoRetryBySession.value, sid);
      compactingBySession.value = withoutKey(compactingBySession.value, sid);
      const { [sid]: _stale, ...restCompaction } = compactionBySession.value;
      compactionBySession.value = restCompaction;
      if (activeId.value === sid) {
        activeId.value = null;
        items.value = [];
        liveTools = new Map();
        streaming = null;
        agentState.value = "idle";
        lastError.value = null;
        pendingApproval.value = null;
        pendingAsk.value = null;
      }
      return;
    }
    // ---- 全局层：不依赖活动会话（后台会话等待确认不再丢失） ----
    if (e.kind === "agent_state_changed") {
      const st = e.payload?.state;
      if (st !== "waiting_approval" && st !== "waiting_ask" && pendingBySession.value[e.session_id]) {
        const next = { ...pendingBySession.value };
        delete next[e.session_id];
        pendingBySession.value = next;
      }
    }
    if (e.kind === "tool_approval") {
      // 空载荷 = 结算信号（远端已审批/中止）：清审批等待，不新立卡
      if (!e.payload?.approval_id) {
        pendingBySession.value = withoutKey(pendingBySession.value, e.session_id);
        if (e.session_id === activeId.value) pendingApproval.value = null;
        return;
      }
      pendingBySession.value = {
        ...pendingBySession.value,
        [e.session_id]: {
          kind: "approval",
          approvalId: e.payload.approval_id,
          toolName: e.payload?.tool_name ?? "",
          args: e.payload?.args ?? "",
        },
      };
      if (e.session_id !== activeId.value) return;
    }
    if (e.kind === "ask_user_question") {
      const q = e.payload?.question;
      const valid = !!(e.payload?.ask_id && q && Array.isArray(q.options) && q.options.length);
      const next = { ...pendingBySession.value };
      if (valid) {
        next[e.session_id] = {
          kind: "ask",
          askId: e.payload.ask_id,
          question: q as AskQuestion,
          ...(Number(e.payload.timeout_sec) > 0
            ? { timeoutSec: Number(e.payload.timeout_sec) }
            : {}),
        };
      } else {
        delete next[e.session_id];
      }
      pendingBySession.value = next;
      if (e.session_id !== activeId.value) return;
    }
    if (e.session_id !== activeId.value) return;
    switch (e.kind) {
      case "message_delta": {
        const { message_id, part, delta } = e.payload;
        const item = ensureStreaming(message_id);
        if (part === "text") {
          // 思考流结束 → 固定耗时
          if (item.thinkingStartedAt && item.thinkingMs === undefined) {
            item.thinkingMs = Date.now() - item.thinkingStartedAt;
            item.thinkingStartedAt = undefined;
          }
          item.text += delta;
        } else {
          if (!item.thinkingStartedAt) item.thinkingStartedAt = Date.now();
          item.thinking += delta;
        }
        break;
      }
      case "message_snapshot": {
        const { message_id, text, thinking } = e.payload;
        const item = ensureStreaming(message_id);
        if ((text ?? "").length >= item.text.length) {
          if (item.thinkingStartedAt && item.thinkingMs === undefined) {
            item.thinkingMs = Date.now() - item.thinkingStartedAt;
            item.thinkingStartedAt = undefined;
          }
          item.text = text ?? "";
        }
        if ((thinking ?? "").length >= item.thinking.length) {
          if (!item.thinkingStartedAt) item.thinkingStartedAt = Date.now();
          item.thinking = thinking ?? "";
        }
        break;
      }
      case "message_complete":
        applyMessageComplete(e.payload);
        break;
      case "tool_execution_start": {
        // AskUserQuestion 的交互在输入区上方问题卡里，不在消息流重复出运行卡
        if (e.payload.tool_name === "AskUserQuestion") break;
        const item: UiToolItem = {
          kind: "tool",
          key: nextKey(),
          callId: e.payload.call_id,
          toolName: e.payload.tool_name,
          status: "running",
          args: e.payload.args ?? "",
          partial: "",
          output: "",
        };
        liveTools.set(item.callId, item);
        items.value.push(item);
        break;
      }
      case "tool_execution_update": {
        const item = liveTools.get(e.payload.call_id);
        if (item) item.partial = e.payload.partial ?? "";
        break;
      }
      case "tool_execution_end": {
        const item = liveTools.get(e.payload.call_id);
        if (item) {
          item.status = e.payload.is_error ? "error" : "done";
          item.output = e.payload.output ?? "";
        }
        break;
      }
      case "agent_state_changed":
        agentState.value = e.payload?.state ?? "idle";
        break;
      case "tool_approval":
        if (e.session_id && e.session_id === activeId.value) {
          // 空载荷 = 结算信号：清除审批卡（与 ask_user_question 空载荷语义一致）
          pendingApproval.value = e.payload?.approval_id
            ? {
                approvalId: e.payload.approval_id,
                toolName: e.payload?.tool_name ?? "",
                args: e.payload?.args ?? "",
              }
            : null;
        }
        break;
      case "ask_user_question": {
        const q = e.payload?.question;
        pendingAsk.value =
          e.payload?.ask_id && q && Array.isArray(q.options) && q.options.length
            ? {
                askId: e.payload.ask_id,
                question: q as AskQuestion,
                ...(Number(e.payload.timeout_sec) > 0
                  ? { timeoutSec: Number(e.payload.timeout_sec) }
                  : {}),
              }
            : null;
        break;
      }
      case "session_meta": {
        // host 用首条用户消息自动生成标题后实时推送；set_model 推送模型变化
        const name = e.payload?.name;
        if (e.session_id && typeof name === "string" && name) {
          const s = sessions.value.find((x) => x.session_id === e.session_id);
          if (s) s.name = name;
        }
        const model = e.payload?.model;
        if (e.session_id && typeof model === "string" && model) {
          const s = sessions.value.find((x) => x.session_id === e.session_id);
          if (s) s.model = model;
        }
        break;
      }
      case "auto_retry": {
        // 自动重试生命周期：start 挂提示条（含尝试次数/错误），end 清除
        if (e.session_id) {
          const p = e.payload ?? {};
          autoRetryBySession.value = {
            ...autoRetryBySession.value,
            [e.session_id]:
              p?.phase === "start"
                ? { attempt: p.attempt, max_attempts: p.max_attempts, error: p.error }
                : null,
          };
        }
        break;
      }
      case "compaction_lifecycle": {
        if (e.session_id) {
          compactingBySession.value = {
            ...compactingBySession.value,
            [e.session_id]: e.payload?.phase === "start",
          };
        }
        break;
      }
      case "compaction_summary": {
        if (e.session_id) {
          compactionBySession.value = {
            ...compactionBySession.value,
            [e.session_id]: {
              summary: String(e.payload?.summary ?? ""),
              ...(Number.isFinite(e.payload?.tokens_before)
                ? { tokens_before: Number(e.payload.tokens_before) }
                : {}),
            },
          };
        }
        break;
      }
      case "session_resynced": {
        // 压缩/分叉重同步：镜像与本地内存态作废，host 会从 seq=0 重灌全量条目重建
        const sid = e.session_id;
        todosBySession.value = { ...todosBySession.value, [sid]: [] };
        const { [sid]: _stale, ...restCompaction } = compactionBySession.value;
        compactionBySession.value = restCompaction;
        if (activeId.value === sid) {
          items.value = [];
          liveTools = new Map();
          streaming = null;
        }
        break;
      }
      case "error":
        lastError.value = e.payload?.message ?? "unknown error";
        break;
      default:
        break;
    }
  }

  async function start(): Promise<void> {
  const unsubscribe = await bus.onEvent(applyEnvelope);
  void unsubscribe; // lives for the app lifetime
  // 回合结束 → 记录结束时间（折叠行显示总耗时）；新回合发送时重置
  watch(agentState, (state) => {
    if (state === "idle") {
      if (turnStartedAt.value && !turnEndedAt.value) turnEndedAt.value = Date.now();
    } else {
      turnEndedAt.value = null;
    }
  });
  await refreshSessions();
  void refreshModels();
  }

  async function refreshSessions(): Promise<void> {
    const r = await bus.request("session.list");
    sessions.value = r.sessions ?? [];
    // host 主目录：用于区分「不在项目中」的会话（web 端服务器快照无此信息，保持 null）
    homeDir.value = typeof r.home === "string" && r.home ? r.home : null;
  }

  /** 拉取全量可用模型（失败静默，下拉退回会话历史模型） */
  async function refreshModels(): Promise<void> {
    try {
      const r = await bus.request("config.models.list");
      allModels.value = r?.models ?? [];
    } catch {
      // 旧 host / 未配置供应商时忽略
    }
  }

  /** merge a pushed session snapshot (web live updates) into the list */
  function upsertSessionSummary(summary: Partial<SessionSummaryUi> & { session_id: string }): void {
    const normalized: SessionSummaryUi = {
      session_id: summary.session_id,
      file: summary.file ?? "",
      cwd: summary.cwd ?? "",
      provider: summary.provider,
      model: summary.model,
      name: summary.name,
      expert_id: summary.expert_id,
      expert_name: summary.expert_name,
      parent_session_id: summary.parent_session_id,
      created_at: summary.created_at ?? new Date().toISOString(),
      open: summary.open ?? true,
      state: summary.state ?? "idle",
    };
    const idx = sessions.value.findIndex((s) => s.session_id === summary.session_id);
    if (idx >= 0) sessions.value[idx] = { ...sessions.value[idx], ...normalized };
    else sessions.value.unshift(normalized);
  }

  function applyPersisted(ev: { kind: string; payload: any; ts?: string }): void {
    if (ev.kind === "message_complete") applyMessageComplete(ev.payload, ev.ts);
    else if (ev.kind === "session_meta" && ev.payload?.name !== undefined) {
      /* name shown from sessions list */
    } else if (ev.kind === "compaction_summary" && activeId.value) {
      // 历史回放恢复压缩摘要（与实时 applyEnvelope 同一落点）
      compactionBySession.value = {
        ...compactionBySession.value,
        [activeId.value]: {
          summary: String(ev.payload?.summary ?? ""),
          ...(Number.isFinite(ev.payload?.tokens_before)
            ? { tokens_before: Number(ev.payload.tokens_before) }
            : {}),
        },
      };
    }
  }

  /** host 错误串带 "trust_required" 前缀（desktop/web 两条链路都这样透传） */
  function isTrustError(err: unknown): boolean {
    const s = err instanceof Error ? err.message : String(err ?? "");
    return s.includes("trust_required");
  }

  /**
   * trust_required 之后的确认框：信任 → 决定写回 host 并重试；
   * 取消 → 写回不信任（会话仍可建，项目自带资源不加载），同样重试。
   * 返回 true = 用户已做出决定（无论哪种），调用方重试一次。
   */
  async function confirmTrust(cwd: string): Promise<boolean> {
    const trust = await appConfirm({
      title: "信任此项目？",
      message: `${cwd}\n\n该项目自带扩展 / 技能 / prompts 等可执行资源，信任后它们将随会话自动加载（等同于运行其中的代码）。不信任则仅加载全局资源，会话仍可继续。`,
      confirmText: "信任并加载",
      cancelText: "不信任继续",
      danger: true,
    });
    try {
      await bus.request("session.trust", { session_id: activeId.value ?? "", cwd, decision: trust });
    } catch {
      // 决定写入失败不阻塞（host 下次会再询问）
    }
    return true;
  }

  async function openSession(sessionId: string): Promise<void> {
    // 切走前把当前会话未处理的审批/提问存回全局表（托盘跳回时可恢复）
    const prev = activeId.value;
    if (prev && prev !== sessionId) {
      if (pendingApproval.value) {
        pendingBySession.value = {
          ...pendingBySession.value,
          [prev]: {
            kind: "approval",
            approvalId: pendingApproval.value.approvalId,
            toolName: pendingApproval.value.toolName,
            args: pendingApproval.value.args,
          },
        };
      }
      if (pendingAsk.value) {
        pendingBySession.value = {
          ...pendingBySession.value,
          [prev]: {
            kind: "ask",
            askId: pendingAsk.value.askId,
            question: pendingAsk.value.question,
            ...(pendingAsk.value.timeoutSec ? { timeoutSec: pendingAsk.value.timeoutSec } : {}),
          },
        };
      }
    }
    const summary = sessions.value.find((s) => s.session_id === sessionId);
    if (summary && !summary.open && summary.file) {
      try {
        await bus.request("session.open", { file: summary.file });
      } catch (err) {
        if (!isTrustError(err) || !summary.cwd || !(await confirmTrust(summary.cwd))) throw err;
        await bus.request("session.open", { file: summary.file });
      }
      await refreshSessions();
    }
    activeId.value = sessionId;
    items.value = [];
    liveTools = new Map();
    streaming = null;
    agentState.value = "idle";
    lastError.value = null;
    pendingApproval.value = null;
    pendingAsk.value = null;
    // 清掉旧清单，等 host 重开补发（老 host 无此事件 → 面板隐藏）
    delete todosBySession.value[sessionId];
    loadingHistory.value = true;
    try {
      const history = await bus.loadHistory(sessionId);
      for (const ev of history) applyPersisted(ev);
    } finally {
      loadingHistory.value = false;
    }
    // 重启后打开会话：从落盘快照恢复文件变更卡片（无快照时为空）
    void fetchFileChanges();
    // 上下文用量 + 模型思考能力（事件兜底之外的主动拉取，web 链路依赖此处）
    void fetchContextUsage();
    void fetchThinkingInfo();
    // 恢复该会话未处理的审批/提问（后台等待时切走又切回；host 不重放这类事件）
    const rec = pendingBySession.value[sessionId];
    if (rec?.kind === "approval" && rec.approvalId) {
      pendingApproval.value = {
        approvalId: rec.approvalId,
        toolName: rec.toolName ?? "",
        args: rec.args ?? "",
      };
    } else if (rec?.kind === "ask" && rec.askId && rec.question) {
      pendingAsk.value = {
        askId: rec.askId,
        question: rec.question,
        ...(rec.timeoutSec ? { timeoutSec: rec.timeoutSec } : {}),
      };
    }
    // 以 host 实际挂起状态对账兜底（web 刷新后内存 stash 为空；后台可能已解决/超时）
    void fetchPending();
  }

  /**
   * 拉取活动会话挂起的审批/提问（session.pending），并同步回 pendingBySession：
   * host 是唯一事实源 —— 有则恢复卡片（timeout_sec 为剩余秒数），无则清掉本地残留。
   * 旧 host 无此 RPC 时静默跳过。
   */
  async function fetchPending(): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    let r: { approval?: PendingApprovalInfo | null; ask?: PendingAskInfo | null };
    try {
      r = await bus.request("session.pending", { session_id: sid });
    } catch {
      return;
    }
    const next = { ...pendingBySession.value };
    if (r?.approval?.approval_id) {
      pendingApproval.value = {
        approvalId: r.approval.approval_id,
        toolName: r.approval.tool_name ?? "",
        args: r.approval.args ?? "",
      };
      next[sid] = {
        kind: "approval",
        approvalId: r.approval.approval_id,
        toolName: r.approval.tool_name ?? "",
        args: r.approval.args ?? "",
      };
    } else {
      pendingApproval.value = null;
      if (next[sid]?.kind === "approval") delete next[sid];
    }
    if (r?.ask?.ask_id && r.ask.question) {
      pendingAsk.value = {
        askId: r.ask.ask_id,
        question: r.ask.question,
        ...(r.ask.timeout_sec ? { timeoutSec: r.ask.timeout_sec } : {}),
      };
      next[sid] = {
        kind: "ask",
        askId: r.ask.ask_id,
        question: r.ask.question,
        ...(r.ask.timeout_sec ? { timeoutSec: r.ask.timeout_sec } : {}),
      };
    } else {
      pendingAsk.value = null;
      if (next[sid]?.kind === "ask") delete next[sid];
    }
    pendingBySession.value = next;
  }

  async function newSession(cwd?: string, model?: string, expertId?: string): Promise<void> {
    // cwd 省略 = 「不在项目中工作」，host 会落到用户主目录
    const params = {
      ...(cwd ? { cwd } : {}),
      ...(model ? { model } : {}),
      ...(expertId ? { expert_id: expertId } : {}),
    };
    let created: { session_id: string };
    try {
      created = await bus.request("session.create", params);
    } catch (err) {
      if (!isTrustError(err) || !cwd || !(await confirmTrust(cwd))) throw err;
      created = await bus.request("session.create", params);
    }
    await refreshSessions();
    await openSession(created.session_id);
  }

  /**
   * 从指定用户消息条目分叉出新会话：历史保留到该条之前，消息原文随
   * selected_text 返回（调用方回填输入框）；成功后自动切换到新会话。
   */
  async function forkSession(
    entryId: string,
    position: "before" | "at" = "before",
  ): Promise<{ session_id: string; file: string; selected_text?: string }> {
    const sid = activeId.value;
    if (!sid) throw new Error("没有打开的会话");
    const r = await bus.request("session.fork", { session_id: sid, entry_id: entryId, position });
    await refreshSessions();
    await openSession(r.session_id);
    return r;
  }

  async function send(
    text: string,
    images?: Array<{ data: string; mime_type: string }>,
    attachments?: Array<{ name: string; mime_type: string; size: number; data: string }>,
  ): Promise<void> {
    if (!activeId.value || !text.trim()) return;
    if (!turnStartedAt.value || turnEndedAt.value) {
      turnStartedAt.value = Date.now();
      turnEndedAt.value = null;
    }
    items.value.push({
      kind: "message",
      key: nextKey(),
      role: "user",
      text,
      thinking: "",
      blocks: null,
      streaming: false,
      pending: true,
      ts: new Date().toISOString(),
      ...(images?.length ? { imageUrls: images.map((i) => `data:${i.mime_type};base64,${i.data}`) } : {}),
      ...(attachments?.length
        ? { attachmentNames: attachments.map((a) => ({ name: a.name, size: a.size })) }
        : {}),
    });
    try {
      await bus.request("agent.prompt", {
        session_id: activeId.value,
        text,
        ...(images?.length ? { images } : {}),
        ...(attachments?.length ? { attachments } : {}),
      });
      lastError.value = null;
    } catch (err) {
      // remove the optimistic bubble and surface the failure
      const idx = items.value.findIndex(
        (it) => it.kind === "message" && it.role === "user" && it.text === text && it.pending,
      );
      if (idx >= 0) items.value.splice(idx, 1);
      lastError.value = err instanceof Error ? err.message : String(err);
    }
  }

  async function abort(): Promise<void> {
    if (!activeId.value) return;
    await bus.request("agent.abort", { session_id: activeId.value });
  }

  /** 切换活动会话的权限模式（host 侧内置权限扩展实时生效） */
  async function setPermissionMode(mode: string): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.set_permission_mode", { session_id: sid, mode });
    permissionModes.value = { ...permissionModes.value, [sid]: mode as PermissionMode };
  }

  /** 重命名会话（host 写注册表 name；空名恢复默认标题） */
  async function renameSession(sessionId: string, name: string): Promise<void> {
    try {
      await bus.request("session.rename", { session_id: sessionId, name });
      const s = sessions.value.find((x) => x.session_id === sessionId);
      if (s) s.name = name || undefined;
      lastError.value = null;
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
    }
  }

  /**
   * 从列表移除会话（host 只删注册表条目，磁盘 JSONL 保留）。
   * 同步清理各组件的 UI 本地偏好（侧栏置顶、第三栏收起记忆、输入卡自定义项目），
   * 活动会话在删除之列时复位回首页。
   */
  async function removeSessions(ids: string[]): Promise<void> {
    const valid = [...new Set(ids.filter((x) => typeof x === "string" && x))];
    if (!valid.length) return;
    const cwds = new Set(
      sessions.value.filter((s) => valid.includes(s.session_id)).map((s) => s.cwd).filter(Boolean),
    );
    try {
      await bus.request("session.remove", { session_ids: valid });
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
      return;
    }
    const idSet = new Set(valid);
    const scrub = (key: string, drop: (v: string) => boolean): void => {
      try {
        const raw = JSON.parse(localStorage.getItem(key) ?? "[]");
        if (!Array.isArray(raw)) return;
        const next = raw.filter((x) => typeof x === "string" && !drop(x));
        if (next.length !== raw.length) localStorage.setItem(key, JSON.stringify(next));
      } catch {
        // localStorage 不可用时忽略
      }
    };
    scrub("pidock.pinnedSessions", (v) => idSet.has(v));
    scrub("pidock.progressChips", (v) => idSet.has(v));
    if (cwds.size) scrub("pidock.customProjects", (v) => cwds.has(v));
    if (activeId.value && idSet.has(activeId.value)) {
      activeId.value = null;
      items.value = [];
      liveTools = new Map();
      streaming = null;
      agentState.value = "idle";
      lastError.value = null;
      pendingApproval.value = null;
      pendingAsk.value = null;
    }
    await refreshSessions();
  }

  /** 回复工具审批请求 */
  async function resolveApproval(approved: boolean): Promise<void> {
    const p = pendingApproval.value;
    if (!p) return;
    pendingApproval.value = null;
    if (activeId.value && pendingBySession.value[activeId.value]?.approvalId === p.approvalId) {
      const next = { ...pendingBySession.value };
      delete next[activeId.value];
      pendingBySession.value = next;
    }
    try {
      await bus.request("session.resolve_approval", { approval_id: p.approvalId, approved });
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
    }
  }

  /** 回答 AskUserQuestion 提问（labels=选中项 / text=自由输入 / interrupted=取消） */
  async function resolveAsk(answer: {
    labels?: string[];
    text?: string;
    interrupted?: boolean;
  }): Promise<void> {
    const p = pendingAsk.value;
    if (!p) return;
    pendingAsk.value = null;
    if (activeId.value && pendingBySession.value[activeId.value]?.askId === p.askId) {
      const next = { ...pendingBySession.value };
      delete next[activeId.value];
      pendingBySession.value = next;
    }
    try {
      await bus.request("session.resolve_ask", {
        session_id: activeId.value,
        ask_id: p.askId,
        ...answer,
      });
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
    }
  }

  /** 切换思考级别（off/minimal/low/medium/high） */
  async function setThinkingLevel(level: string): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.set_thinking_level", { session_id: sid, level });
    thinkingLevels.value = { ...thinkingLevels.value, [sid]: level };
  }

  /** 按名称切换会话模型（provider/model-id 唯一匹配） */
  async function setModel(model: string): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    try {
      const r = await bus.request("session.set_model", { session_id: sid, model });
      const s = sessions.value.find((x) => x.session_id === sid);
      if (s && r?.model) s.model = r.model;
      lastError.value = null;
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
    }
  }

  /** 列出工作区文件（输入框 @ 提及选择器）；失败返回空列表 */
  async function listWorkspaceFiles(
    cwd: string,
  ): Promise<Array<{ path: string; name: string; dir: boolean }>> {
    try {
      const r = await bus.request("workspace.files", { cwd });
      return r?.files ?? [];
    } catch {
      return [];
    }
  }

  /** 读取工作区内文本文件（文件浏览预览）；失败抛错由调用方展示 */
  async function readWorkspaceFile(
    cwd: string,
    path: string,
  ): Promise<{ text: string; truncated: boolean; binary: boolean; size: number }> {
    return bus.request("workspace.read_file", { cwd, path });
  }

  /** 设置/清除内置模型覆盖配置（写入 models.json modelOverrides，null 清除） */
  async function setModelOverride(
    provider: string,
    model: string,
    override: Record<string, unknown> | null,
  ): Promise<void> {
    await bus.request("config.model_override.set", { provider, model, override: override ?? null });
    await refreshModels();
  }

  /** 常规设置里「技能斜杠命令」开关（pi 端展开 /skill: 的前提）；缓存 60s */
  let skillsCommandsCache: { value: boolean; at: number } | null = null;
  async function skillsCommandsEnabled(): Promise<boolean> {
    if (skillsCommandsCache && Date.now() - skillsCommandsCache.at < 60_000) {
      return skillsCommandsCache.value;
    }
    let enabled = true;
    try {
      const r = await bus.request("config.get");
      const v = r?.settings?.enableSkillCommands;
      enabled = typeof v === "boolean" ? v : true;
    } catch {
      enabled = true; // 读不到设置时按可用处理
    }
    skillsCommandsCache = { value: enabled, at: Date.now() };
    return enabled;
  }

  /** 技能列表（$ 选择技能与 / 弹窗共用），仅返回启用的；全局开关关闭时为空 */
  async function listSkills(
    cwd?: string,
  ): Promise<Array<{ name: string; description: string }>> {
    try {
      if (!(await skillsCommandsEnabled())) return [];
      const r = await bus.request("config.skills.list", cwd ? { cwd } : {});
      return (r?.skills ?? [])
        .filter((s: any) => s.enabled !== false)
        .map((s: any) => ({ name: String(s.name ?? ""), description: String(s.description ?? "") }));
    } catch {
      return [];
    }
  }

  /** 专家列表（雇佣 chip 与 / 弹窗专家段共用）；失败静默返回空 */
  async function listExperts(): Promise<Array<{ id: string; name: string; description?: string; icon?: string; avatar?: string }>> {
    try {
      const r = await bus.request("experts.list");
      return (r?.experts ?? []).map((e: any) => ({
        id: String(e.id ?? ""),
        name: String(e.name ?? ""),
        description: e.description ? String(e.description) : undefined,
        icon: e.icon ? String(e.icon) : undefined,
        avatar: e.avatar ? String(e.avatar) : undefined,
      }));
    } catch {
      return [];
    }
  }

  /** 单个专家详情（第三栏信息卡用）；专家已删除时返回 null */
  async function getExpert(id: string): Promise<ExpertInfo | null> {
    if (!id) return null;
    try {
      const r = await bus.request("experts.get", { id });
      const e = r?.expert;
      if (!e) return null;
      return {
        id: String(e.id ?? ""),
        name: String(e.name ?? ""),
        description: e.description ? String(e.description) : undefined,
        icon: e.icon ? String(e.icon) : undefined,
        avatar: e.avatar ? String(e.avatar) : undefined,
        avatar_color: e.avatar_color ? String(e.avatar_color) : undefined,
        prompt: String(e.prompt ?? ""),
        skills: Array.isArray(e.skills) ? e.skills.map(String) : [],
        extensions: Array.isArray(e.extensions) ? e.extensions.map(String) : [],
        exclude_tools: Array.isArray(e.exclude_tools) ? e.exclude_tools.map(String) : undefined,
        knowledge_dirs: Array.isArray(e.knowledge_dirs) ? e.knowledge_dirs.map(String) : [],
      };
    } catch {
      return null;
    }
  }

  /** 回合结束后拉取各回合文件变更（按轮分组） */
  async function fetchFileChanges(): Promise<void> {
    if (!activeId.value) return;
    try {
      const r = await bus.request("session.file_changes", { session_id: activeId.value });
      turnFileChanges.value = (r.turns ?? []).map((t: any) => ({
        turnId: t.turn_id ?? "",
        files: t.files ?? [],
      }));
    } catch {
      turnFileChanges.value = [];
    }
  }

  /** 单文件快照 vs 当前内容的 diff（可指定回合，缺省最近回合） */
  async function fileDiff(
    path: string,
    turnId?: string,
  ): Promise<{ oldText: string; newText: string }> {
    return bus.request("session.file_diff", {
      session_id: activeId.value,
      path,
      ...(turnId ? { turn_id: turnId } : {}),
    });
  }

  /** 撤销指定回合（缺省最近回合）的文件更改，成功后重新拉取 */
  async function revertFiles(turnId?: string): Promise<void> {
    if (!activeId.value) return;
    await bus.request("session.revert_files", {
      session_id: activeId.value,
      ...(turnId ? { turn_id: turnId } : {}),
    });
    await fetchFileChanges();
  }

  // ------------------------------------------------- context/queue/export

  /** 拉取指定会话（缺省活动会话）的上下文用量；事件推送之外的兜底 */
  async function fetchContextUsage(sessionId?: string): Promise<ContextUsageInfo | null> {
    const sid = sessionId ?? activeId.value;
    if (!sid) return null;
    try {
      const r = await bus.request("agent.context_usage", { session_id: sid });
      const info = r as ContextUsageInfo;
      contextBySession.value = { ...contextBySession.value, [sid]: info };
      if (r?.steering_count !== undefined || r?.follow_up_count !== undefined) {
        queueBySession.value = {
          ...queueBySession.value,
          [sid]: { steering: Number(r.steering_count) || 0, follow_up: Number(r.follow_up_count) || 0 },
        };
      }
      return info;
    } catch {
      return null;
    }
  }

  /** 手动压缩上下文（host fire-and-forget；进度看 compaction_lifecycle/summary 事件） */
  async function compactSession(instructions?: string): Promise<boolean> {
    const sid = activeId.value;
    if (!sid) return false;
    try {
      await bus.request("agent.compact", { session_id: sid, ...(instructions ? { instructions } : {}) });
      lastError.value = null;
      return true;
    } catch (err) {
      lastError.value = err instanceof Error ? err.message : String(err);
      return false;
    }
  }

  /** 清空当前会话排队中的消息，返回被清除的文本（输入框可回填） */
  async function clearQueue(): Promise<string[]> {
    const sid = activeId.value;
    if (!sid) return [];
    try {
      const r = await bus.request("agent.clear_queue", { session_id: sid });
      queueBySession.value = { ...queueBySession.value, [sid]: { steering: 0, follow_up: 0 } };
      return Array.isArray(r?.texts) ? r.texts.map(String) : [];
    } catch {
      return [];
    }
  }

  /** 开关自动压缩 */
  async function setAutoCompaction(enabled: boolean): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.set_auto_compaction", { session_id: sid, enabled });
    const cur = contextBySession.value[sid];
    if (cur) contextBySession.value = { ...contextBySession.value, [sid]: { ...cur, auto_compaction: enabled } };
  }

  /** 开关自动重试 */
  async function setAutoRetry(enabled: boolean): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.set_auto_retry", { session_id: sid, enabled });
    const cur = contextBySession.value[sid];
    if (cur) contextBySession.value = { ...contextBySession.value, [sid]: { ...cur, auto_retry: enabled } };
  }

  /** 导出会话（format: html | jsonl），返回落盘路径 */
  async function exportSession(format: "html" | "jsonl"): Promise<string> {
    const sid = activeId.value;
    if (!sid) throw new Error("没有打开的会话");
    const r = await bus.request("session.export", { session_id: sid, format });
    return String(r?.path ?? "");
  }

  // -------------------------------------------------------- tools/tree

  /** 会话工具清单（工具管理对话框） */
  async function listTools(sessionId?: string): Promise<ToolInfoUi[]> {
    const sid = sessionId ?? activeId.value;
    if (!sid) return [];
    try {
      const r = await bus.request("session.list_tools", { session_id: sid });
      return r?.tools ?? [];
    } catch {
      return [];
    }
  }

  /** 设置启用工具子集 */
  async function setActiveTools(names: string[]): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.set_active_tools", { session_id: sid, active_names: names });
  }

  /** 会话分支树的用户消息节点 */
  async function fetchSessionTree(sessionId?: string): Promise<{ nodes: TreeNodeUi[]; leafId: string | null }> {
    const sid = sessionId ?? activeId.value;
    if (!sid) return { nodes: [], leafId: null };
    try {
      const r = await bus.request("session.tree", { session_id: sid });
      return { nodes: r?.nodes ?? [], leafId: r?.leaf_id ?? null };
    } catch {
      return { nodes: [], leafId: null };
    }
  }

  /**
   * 就地切换到树中某条用户消息（其后的对话退出上下文）。切换后重开会话
   * 刷新视图；重开前后 session_id 不变（同一 JSONL 文件）。
   */
  async function navigateTree(entryId: string): Promise<void> {
    const sid = activeId.value;
    if (!sid) return;
    await bus.request("session.navigate_tree", { session_id: sid, entry_id: entryId });
    await openSession(sid);
  }

  // -------------------------------------------------- prompts/thinking

  /** 当前会话的 prompts 模板（slash 弹窗与技能合并展示） */
  async function listPrompts(sessionId?: string): Promise<Array<{ name: string; description: string; argument_hint: string }>> {
    const sid = sessionId ?? activeId.value;
    if (!sid) return [];
    try {
      const r = await bus.request("config.prompts.list", { session_id: sid });
      return r?.prompts ?? [];
    } catch {
      return [];
    }
  }

  /** 拉取当前会话模型的思考能力（选择器过滤用） */
  async function fetchThinkingInfo(sessionId?: string): Promise<ThinkingInfoUi | null> {
    const sid = sessionId ?? activeId.value;
    if (!sid) return null;
    try {
      const r = await bus.request("session.thinking_info", { session_id: sid });
      const info = r as ThinkingInfoUi;
      thinkingBySession.value = { ...thinkingBySession.value, [sid]: info };
      return info;
    } catch {
      return null;
    }
  }

  return reactive({
    // state
    sessions,
    activeId,
    items,
    agentState,
    lastError,
    loadingHistory,
    homeDir,
    permissionModes,
    thinkingLevels,
    pendingApproval,
    pendingAsk,
    pendingBySession,
    turnStartedAt,
    turnEndedAt,
    turnFileChanges,
    allModels,
    todosBySession,
    contextBySession,
    queueBySession,
    thinkingBySession,
    autoRetryBySession,
    compactingBySession,
    compactionBySession,
    // actions
    start,
    refreshSessions,
    refreshModels,
    upsertSessionSummary,
    openSession,
    newSession,
    forkSession,
    fetchPending,
    send,
    abort,
    setPermissionMode,
    renameSession,
    removeSessions,
    listSkills,
    listExperts,
    getExpert,
    setModelOverride,
    listWorkspaceFiles,
    readWorkspaceFile,
    resolveApproval,
    resolveAsk,
    setThinkingLevel,
    setModel,
    fetchFileChanges,
    fileDiff,
    revertFiles,
    fetchContextUsage,
    compactSession,
    clearQueue,
    setAutoCompaction,
    setAutoRetry,
    exportSession,
    listTools,
    setActiveTools,
    fetchSessionTree,
    navigateTree,
    listPrompts,
    fetchThinkingInfo,
  });
}

export type AgentStore = ReturnType<typeof createAgentStore>;
