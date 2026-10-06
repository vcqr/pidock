import { reactive, ref, watch } from "vue";
import type { DataBus } from "./databus.js";
import type { AskQuestion, TodoItem } from "@pidock/protocol";

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

  let liveTools = new Map<string, UiToolItem>();
  let streaming: UiMessageItem | null = null;

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
      if (streaming && p.message_id && streaming.key === p.message_id) {
        streaming.blocks = asBlocks(p);
        streaming.ts = itemTs;
        streaming.streaming = false;
        streaming.errorMessage = p.errorMessage || undefined;
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
          pendingApproval.value = {
            approvalId: e.payload?.approval_id ?? "",
            toolName: e.payload?.tool_name ?? "",
            args: e.payload?.args ?? "",
          };
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
        // host 用首条用户消息自动生成标题后实时推送
        const name = e.payload?.name;
        if (e.session_id && typeof name === "string" && name) {
          const s = sessions.value.find((x) => x.session_id === e.session_id);
          if (s) s.name = name;
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
    }
  }

  async function openSession(sessionId: string): Promise<void> {
    const summary = sessions.value.find((s) => s.session_id === sessionId);
    if (summary && !summary.open && summary.file) {
      await bus.request("session.open", { file: summary.file });
      await refreshSessions();
    }
    activeId.value = sessionId;
    items.value = [];
    liveTools = new Map();
    streaming = null;
    agentState.value = "idle";
    lastError.value = null;
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
  }

  async function newSession(cwd?: string, model?: string, expertId?: string): Promise<void> {
    // cwd 省略 = 「不在项目中工作」，host 会落到用户主目录
    const created = await bus.request("session.create", {
      ...(cwd ? { cwd } : {}),
      ...(model ? { model } : {}),
      ...(expertId ? { expert_id: expertId } : {}),
    });
    await refreshSessions();
    await openSession(created.session_id);
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

  /** 回复工具审批请求 */
  async function resolveApproval(approved: boolean): Promise<void> {
    const p = pendingApproval.value;
    if (!p) return;
    pendingApproval.value = null;
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
    turnStartedAt,
    turnEndedAt,
    turnFileChanges,
    allModels,
    todosBySession,
    // actions
    start,
    refreshSessions,
    refreshModels,
    upsertSessionSummary,
    openSession,
    newSession,
    send,
    abort,
    setPermissionMode,
    renameSession,
    listSkills,
    listExperts,
    getExpert,
    setModelOverride,
    listWorkspaceFiles,
    resolveApproval,
    resolveAsk,
    setThinkingLevel,
    setModel,
    fetchFileChanges,
    fileDiff,
    revertFiles,
  });
}

export type AgentStore = ReturnType<typeof createAgentStore>;
