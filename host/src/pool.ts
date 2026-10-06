import { randomUUID } from "node:crypto";
import { closeSync, createReadStream, existsSync, mkdirSync, openSync, readdirSync, readFileSync, readSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { createInterface } from "node:readline";
import {
  createAgentSession,
  DefaultResourceLoader,
  getAgentDir,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  VERSION as PI_VERSION,
  type AgentSession,
  type ExtensionAPI,
} from "@earendil-works/pi-coding-agent";
import { emitEvent } from "./emit.js";
import { entryToPayload, metaPayloadFromSession } from "./map.js";
import { parseAttachments, type IncomingAttachment } from "./attachments.js";
import { createTodoWriteTool, sanitizeTodos } from "./todo.js";
import { createAskUserQuestionTool, type AskAnswer } from "./ask.js";
import { buildExpertPromptBlock, ExpertsService, matchExpertCommand, type ExpertSessionConfig } from "./experts.js";
import { basename } from "node:path";
import {
  ATTACHMENT_THRESHOLD_BYTES,
  Event,
  INLINE_PREVIEW_BYTES,
  type AgentState,
  type AskQuestion,
  type Block,
  type TodoItem,
} from "@pidock/protocol";

export const HOST_VERSION = "0.1.0";

/** 工具调用权限模式（输入卡片左下角下拉） */
export type PermissionMode = "plan" | "confirm" | "edit-auto" | "full";
const PERMISSION_MODES: PermissionMode[] = ["plan", "confirm", "edit-auto", "full"];
const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high"];

type LoaderOptions = ConstructorParameters<typeof DefaultResourceLoader>[0];

export class RpcError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

/** sum helper for JSONL usage counters that may be absent/non-numeric */
function num(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

interface TrackedSession {
  session: AgentSession;
  cwd: string;
  provider?: string;
  model?: string;
  /**
   * Number of JSONL entries already drained (seeded from the entries present
   * at open). Persisted events get seq = ++lastSeq, i.e. the 1-based entry
   * index — identical mapping for live events and replay.
   */
  lastSeq: number;
  /** streaming identity for the in-flight assistant message (deltas/snapshots) */
  turnCounter: number;
  pendingTurnId: string | null;
  unsubscribe: () => void;
  drain: () => void;
  /** LLM 最近一次 TodoWrite 提交的任务清单（UI 进度卡数据源；重开时从 JSONL 恢复） */
  todos: TodoItem[];
  snapshot: {
    messageId: string | null;
    text: string;
    thinking: string;
    dirty: boolean;
    lastEmit: number;
  };
}

interface RegistryEntry {
  session_id: string;
  file: string;
  cwd: string;
  provider?: string;
  model?: string;
  name?: string;
  /** 创建时雇佣的专家（重开会话据此重放同样的 loader 覆盖） */
  expert_id?: string;
  created_at: string;
}

export interface SessionSummary {
  session_id: string;
  file: string;
  cwd: string;
  provider?: string;
  model?: string;
  name?: string;
  expert_id?: string;
  expert_name?: string;
  created_at: string;
  open: boolean;
  state: AgentState;
}

const SNAPSHOT_INTERVAL_MS = 2000;

export class SessionPool {
  private sessions = new Map<string, TrackedSession>();
  private modelRuntimePromise: Promise<ModelRuntime> | null = null;
  /** 专家档案服务（无状态，读 experts.json） */
  private experts = new ExpertsService();
  /** 每会话权限模式（内置权限扩展实时读取） */
  private permissionStates = new Map<string, { mode: PermissionMode }>();
  /** 每会话回合内被修改文件的快照（撤销/文件变更卡片的数据源） */
  /** 回合快照：turnId（该轮用户消息 session 条目官方 id）→ { path → 快照 }；按回合累积，供按轮审查/撤销 */
  private fileSnapshots = new Map<
    string,
    Map<string, Map<string, { existed: boolean; content: Buffer }>>
  >();
  /** bash 工具执行前的目录元数据扫描（执行后 diff 出新建文件补快照） */
  private bashPreScans = new Map<string, Map<string, { mtime: number; size: number }>>();
  /** 等待用户审批的工具调用 */
  private pendingApprovals = new Map<
    string,
    { sessionId: string; resolve: (approved: boolean) => void }
  >();
  /** 等待用户回答的 AskUserQuestion 提问（"timeout" 哨兵 = 等待超时） */
  private pendingAsks = new Map<
    string,
    { sessionId: string; resolve: (answer: AskAnswer | "timeout" | null) => void }
  >();

  constructor() {
    // 启动时清理 registry 里已不存在会话的快照落盘文件
    this.pruneSnapshots();
  }

  // ---------------------------------------------------------------- model

  /** shared ModelRuntime (also used by the config service) */
  modelRuntime(): Promise<ModelRuntime> {
    this.modelRuntimePromise ??= ModelRuntime.create();
    return this.modelRuntimePromise;
  }

  private async resolveModel(ref?: string): Promise<object | undefined> {
    if (!ref) return undefined;
    const slash = ref.indexOf("/");
    if (slash <= 0) {
      throw new RpcError("bad_model_ref", `model must be "provider/model-id", got "${ref}"`);
    }
    const provider = ref.slice(0, slash);
    const id = ref.slice(slash + 1);
    const mr = await this.modelRuntime();
    const model = mr.getModel(provider, id);
    if (!model) {
      throw new RpcError("model_not_found", `model "${ref}" not found in registry (check models.json / auth)`);
    }
    return model;
  }

  // ------------------------------------------------------------- registry

  private registryPath(): string {
    return join(getAgentDir(), "pidock", "registry.json");
  }

  private readRegistry(): RegistryEntry[] {
    try {
      const raw = readFileSync(this.registryPath(), "utf8");
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed?.sessions) ? parsed.sessions : [];
    } catch {
      return [];
    }
  }

  private writeRegistryEntry(entry: RegistryEntry): void {
    const path = this.registryPath();
    mkdirSync(dirname(path), { recursive: true });
    const all = this.readRegistry().filter((e) => e.session_id !== entry.session_id);
    all.push(entry);
    writeFileSync(path, JSON.stringify({ sessions: all }, null, 2));
  }

  private removeRegistryEntry(sessionId: string): void {
    const path = this.registryPath();
    const all = this.readRegistry().filter((e) => e.session_id !== sessionId);
    writeFileSync(path, JSON.stringify({ sessions: all }, null, 2));
  }

  /**
   * 从 UI 列表移除会话（只删注册表条目；磁盘 JSONL 保留）。
   * 仍在内存中打开的会话先关闭，未知的 id 幂等跳过。
   */
  removeSessions(params: { session_ids: string[] }): { removed: number } {
    const ids = new Set(
      Array.isArray(params.session_ids) ? params.session_ids.filter((x) => typeof x === "string") : [],
    );
    if (!ids.size) return { removed: 0 };
    const all = this.readRegistry();
    const kept = all.filter((e) => !ids.has(e.session_id));
    const removed = all.length - kept.length;
    if (!removed) return { removed: 0 };
    for (const e of all) {
      if (ids.has(e.session_id)) this.closeSession({ session_id: e.session_id });
    }
    writeFileSync(this.registryPath(), JSON.stringify({ sessions: kept }, null, 2));
    return { removed };
  }

  // ---------------------------------------------------------------- state

  private setState(tracked: TrackedSession, sessionId: string, state: AgentState): void {
    emitEvent(sessionId, Event.AGENT_STATE_CHANGED, { state });
  }

  // ------------------------------------------------------------- creation

  /**
   * 专家档案 → DefaultResourceLoader 覆盖项。角色提示词与知识库清单走
   * appendSystemPrompt（追加在 pi 默认系统提示词之后，保留编码底座）；
   * 私有技能/插件目录走 additional*Paths（与全局资源合并后加载）；
   * skillsOverride 过滤的是合并结果（0.80.10 reload 顺序已核实），所以
   * 白名单同时约束全局技能与专家私有技能；extensionsOverride 必须保留
   * `<inline:` 前缀条目——host 的权限拦截扩展是 inline factory。
   */
  private expertLoaderOptions(cfg: ExpertSessionConfig | null): Partial<LoaderOptions> {
    if (!cfg) return {};
    const options: Partial<LoaderOptions> = {
      ...(cfg.appendSystemPrompt.length ? { appendSystemPrompt: cfg.appendSystemPrompt } : {}),
      ...(cfg.additionalSkillPaths.length ? { additionalSkillPaths: cfg.additionalSkillPaths } : {}),
      ...(cfg.additionalExtensionPaths.length ? { additionalExtensionPaths: cfg.additionalExtensionPaths } : {}),
    };
    if (cfg.skillAllowlist) {
      const allow = cfg.skillAllowlist;
      options.skillsOverride = (base) => ({
        ...base,
        skills: base.skills.filter((s) => allow.includes(s.name)),
      });
    }
    if (cfg.extensionAllowlist) {
      const allow = cfg.extensionAllowlist;
      options.extensionsOverride = (base) => ({
        ...base,
        extensions: base.extensions.filter(
          (x) => x.path.startsWith("<inline:") || allow.includes(basename(x.path).replace(/\.(ts|js)$/i, "")),
        ),
      });
    }
    return options;
  }

  async createSession(params: {
    cwd?: string;
    model?: string;
    thinking_level?: string;
    /** 雇佣的专家 id（缺省 = 普通会话） */
    expert_id?: string;
  }): Promise<{ session_id: string; file: string; cwd: string; expert_id?: string; expert_name?: string }> {
    // 无头模式下没有"当前项目"概念：缺省落在用户主目录而不是 host 进程目录
    const cwd = params.cwd ?? homedir();
    const expertCfg = this.experts.resolveForSession(params.expert_id);
    // 显式传入的模型/思考级别优先于专家预设
    const model = await this.resolveModel(params.model ?? expertCfg?.expert.model);
    const sessionManager = SessionManager.create(cwd);
    // 新会话默认计划模式：修改类工具（bash/write/edit）先被拦下，用户可在输入卡切换；
    // 专家可带建议权限模式（如只读专家保持 plan、自动化场景的 full）
    const permission = { mode: expertCfg?.expert.permission_mode ?? "plan" };
    const sessionIdRef = { id: "" };
    const trackedRef: { current: TrackedSession | null } = { current: null };
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir: getAgentDir(),
      settingsManager: SettingsManager.create(cwd, getAgentDir()),
      ...this.expertLoaderOptions(expertCfg),
      extensionFactories: [
        {
          name: "pidock-permission",
          factory: (pi: ExtensionAPI) => {
            pi.on("tool_call", async (event) => {
              this.snapshotBeforeMutation(sessionIdRef.id, cwd, event.toolName, event.input);
              return this.enforcePermission(sessionIdRef.id, permission, event.toolName, event.input);
            });
          },
        },
      ],
    });
    await loader.reload();
    const thinkingLevel = params.thinking_level ?? expertCfg?.expert.thinking_level;
    const { session } = await createAgentSession({
      cwd,
      model: model as never,
      ...(thinkingLevel ? { thinkingLevel: thinkingLevel as never } : {}),
      // 专家可增减内置工具（如文档专家禁掉 bash/edit）
      ...(expertCfg?.tools ? { tools: expertCfg.tools } : {}),
      ...(expertCfg?.excludeTools?.length ? { excludeTools: expertCfg.excludeTools } : {}),
      modelRuntime: await this.modelRuntime(),
      sessionManager,
      resourceLoader: loader,
      // TodoWrite：LLM 更新任务清单 → 存进 TrackedSession 并推送 todo_updated
      // AskUserQuestion：向用户提问 → 发 ask_user_question 事件并等待 resolve_ask
      customTools: [
        createTodoWriteTool((todos) => {
          if (trackedRef.current) trackedRef.current.todos = todos;
          if (sessionIdRef.id) {
            emitEvent(sessionIdRef.id, Event.TODO_UPDATED, { todos }, { persist: false });
          }
        }),
        createAskUserQuestionTool((question) => this.askUser(sessionIdRef.id, question)),
      ],
    });
    const sessionId = session.sessionId;
    sessionIdRef.id = sessionId;
    this.permissionStates.set(sessionId, permission);
    const tracked: TrackedSession = {
      session,
      cwd,
      provider: undefined,
      model: params.model,
      lastSeq: sessionManager.getEntries().length,
      turnCounter: 0,
      pendingTurnId: null,
      unsubscribe: () => {},
      drain: () => {},
      todos: [],
      snapshot: { messageId: null, text: "", thinking: "", dirty: false, lastEmit: 0 },
    };
    trackedRef.current = tracked;
    tracked.unsubscribe = this.wire(sessionId, tracked);
    this.sessions.set(sessionId, tracked);

    const file = session.sessionFile ?? "";
    this.writeRegistryEntry({
      session_id: sessionId,
      file,
      cwd,
      model: params.model ?? expertCfg?.expert.model,
      ...(expertCfg ? { expert_id: expertCfg.expert.id } : {}),
      created_at: new Date().toISOString(),
    });

    emitEvent(sessionId, Event.SESSION_META, metaPayloadFromSession({ cwd, model: params.model ?? expertCfg?.expert.model }), {
      persist: false,
    });
    return {
      session_id: sessionId,
      file,
      cwd,
      ...(expertCfg ? { expert_id: expertCfg.expert.id, expert_name: expertCfg.expert.name } : {}),
    };
  }

  async openSession(params: { file: string }): Promise<{ session_id: string; file: string; cwd: string; expert_id?: string; expert_name?: string }> {
    const sessionManager = SessionManager.open(params.file);
    const cwd = sessionManager.getCwd() || process.cwd();
    // 重开会话按注册表里的 expert_id 重放专家配置（专家已被删除则回退普通会话）
    const expertCfg = this.experts.resolveForSession(
      this.readRegistry().find((e) => e.file === params.file)?.expert_id,
    );
    // 与 createSession 一致：权限模式不持久化，重开回到默认（专家会话回到专家预设）计划模式
    const permission = { mode: expertCfg?.expert.permission_mode ?? "plan" };
    const sessionIdRef = { id: "" };
    const trackedRef: { current: TrackedSession | null } = { current: null };
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir: getAgentDir(),
      settingsManager: SettingsManager.create(cwd, getAgentDir()),
      ...this.expertLoaderOptions(expertCfg),
      extensionFactories: [
        {
          name: "pidock-permission",
          factory: (pi: ExtensionAPI) => {
            pi.on("tool_call", async (event) => {
              this.snapshotBeforeMutation(sessionIdRef.id, cwd, event.toolName, event.input);
              return this.enforcePermission(sessionIdRef.id, permission, event.toolName, event.input);
            });
          },
        },
      ],
    });
    await loader.reload();
    const { session } = await createAgentSession({
      cwd,
      modelRuntime: await this.modelRuntime(),
      sessionManager,
      resourceLoader: loader,
      // 与 createSession 一致：专家会话重开同样应用工具增减
      ...(expertCfg?.tools ? { tools: expertCfg.tools } : {}),
      ...(expertCfg?.excludeTools?.length ? { excludeTools: expertCfg.excludeTools } : {}),
      // 与 createSession 一致：TodoWrite 推送任务清单（重开场景由 restoreTodos 补发历史状态）
      customTools: [
        createTodoWriteTool((todos) => {
          if (trackedRef.current) trackedRef.current.todos = todos;
          if (sessionIdRef.id) {
            emitEvent(sessionIdRef.id, Event.TODO_UPDATED, { todos }, { persist: false });
          }
        }),
        createAskUserQuestionTool((question) => this.askUser(sessionIdRef.id, question)),
      ],
    });
    const sessionId = session.sessionId;
    sessionIdRef.id = sessionId;
    this.permissionStates.set(sessionId, permission);
    const tracked: TrackedSession = {
      session,
      cwd,
      lastSeq: sessionManager.getEntries().length,
      turnCounter: 0,
      pendingTurnId: null,
      unsubscribe: () => {},
      drain: () => {},
      todos: this.restoreTodos(sessionManager),
      snapshot: { messageId: null, text: "", thinking: "", dirty: false, lastEmit: 0 },
    };
    trackedRef.current = tracked;
    tracked.unsubscribe = this.wire(sessionId, tracked);
    this.sessions.set(sessionId, tracked);

    // 重开会话后立即把恢复出的任务清单推给 UI（web 端经 cloud mirror 同样收到）
    if (tracked.todos.length) {
      emitEvent(sessionId, Event.TODO_UPDATED, { todos: tracked.todos }, { persist: false });
    }

    emitEvent(sessionId, Event.SESSION_META, metaPayloadFromSession({ cwd }), { persist: false });
    return {
      session_id: sessionId,
      file: params.file,
      cwd,
      ...(expertCfg ? { expert_id: expertCfg.expert.id, expert_name: expertCfg.expert.name } : {}),
    };
  }

  closeSession(params: { session_id: string }): { closed: boolean } {
    const tracked = this.sessions.get(params.session_id);
    if (!tracked) return { closed: false };
    this.denyApprovals(params.session_id);
    this.denyAsks(params.session_id);
    // 只清内存；落盘快照保留，重新打开会话后文件变更/审查/撤销仍可用
    this.fileSnapshots.delete(params.session_id);
    this.bashPreScans.delete(params.session_id);
    tracked.unsubscribe();
    tracked.session.dispose();
    this.sessions.delete(params.session_id);
    return { closed: true };
  }

  /**
   * 扫描会话 JSONL 条目里最后一个 TodoWrite 工具调用，恢复任务清单。
   * 自定义工具调用随 assistant 消息块持久化，无需额外落盘；压缩重写后
   * 旧条目消失 → 清单为空，可接受（仅重开时扫描，live 状态在内存里）。
   */
  private restoreTodos(sessionManager: SessionManager): TodoItem[] {
    let todos: TodoItem[] = [];
    for (const entry of sessionManager.getEntries() as unknown as Record<string, any>[]) {
      if (entry?.type !== "message") continue;
      const msg = entry.message;
      if (msg?.role !== "assistant" || !Array.isArray(msg.content)) continue;
      for (const part of msg.content) {
        if (part?.type === "toolCall" && part?.name === "TodoWrite") {
          todos = sanitizeTodos(part.arguments?.todos);
        }
      }
    }
    return todos;
  }

  // ------------------------------------------------------------- querying

  listSessions(): { sessions: SessionSummary[]; home: string } {
    const openIds = new Set(this.sessions.keys());
    const expertNames = new Map(this.experts.list().experts.map((e) => [e.id, e.name]));
    const summaries: SessionSummary[] = this.readRegistry().map((entry) => ({
      session_id: entry.session_id,
      file: entry.file,
      cwd: entry.cwd,
      ...(entry.provider ? { provider: entry.provider } : {}),
      ...(entry.model ? { model: entry.model } : {}),
      ...(entry.name ? { name: entry.name } : {}),
      ...(entry.expert_id
        ? { expert_id: entry.expert_id, ...(expertNames.get(entry.expert_id) ? { expert_name: expertNames.get(entry.expert_id) } : {}) }
        : {}),
      created_at: entry.created_at,
      open: openIds.has(entry.session_id),
      state: this.stateOf(entry.session_id),
    }));
    // home 用于 UI 区分「不在项目中」的会话（cwd = 主目录）与项目会话
    return { sessions: summaries, home: homedir() };
  }

  private stateOf(sessionId: string): AgentState {
    const tracked = this.sessions.get(sessionId);
    if (!tracked) return "idle";
    const s = tracked.session;
    if (s.isCompacting) return "compacting";
    if (s.retryAttempt > 0) return "retrying";
    if (s.isStreaming) return "responding";
    return s.isIdle ? "idle" : "thinking";
  }

  // ------------------------------------------------------------ usage stats

  /**
   * Aggregate usage statistics by scanning persisted session JSONL files
   * (assistant message entries carry per-call token usage). Scanning is
   * async line-streamed so live sessions are never blocked by big histories.
   */
  async usageStats(params: { max_sessions?: number } = {}): Promise<{
    scanned_sessions: number;
    messages: { user: number; assistant: number };
    tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number };
    cost: number;
    by_model: Array<{ key: string; sessions: number; messages: number; tokens: number }>;
    by_day: Array<{ day: string; messages: number; tokens: number }>;
    top_sessions: Array<{
      session_id: string;
      name?: string;
      cwd: string;
      model?: string;
      messages: number;
      tokens: number;
      created_at: string;
    }>;
  }> {
    const MAX_SESSIONS = Math.min(Math.max(params.max_sessions ?? 500, 1), 2000);
    const MAX_LINE_CHARS = 2 * 1024 * 1024; // skip giant lines (embedded images etc.)

    const entries = this.readRegistry()
      .sort((a, b) => (a.created_at < b.created_at ? 1 : -1))
      .slice(0, MAX_SESSIONS);

    const messages = { user: 0, assistant: 0 };
    const tokens = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };
    let cost = 0;
    const perModel = new Map<string, { sessions: Set<string>; messages: number; tokens: number }>();
    const perDay = new Map<string, { messages: number; tokens: number }>();
    const perSession = new Map<string, { messages: number; tokens: number }>();
    let scanned = 0;

    for (const entry of entries) {
      if (!existsSync(entry.file)) continue;
      scanned++;
      const sid = entry.session_id;
      const sess = { messages: 0, tokens: 0 };
      const rl = createInterface({
        input: createReadStream(entry.file, { encoding: "utf8" }),
        crlfDelay: Infinity,
      });
      for await (const line of rl) {
        if (!line || line.length > MAX_LINE_CHARS) continue;
        let e: any;
        try {
          e = JSON.parse(line);
        } catch {
          continue;
        }
        if (e?.type !== "message") continue;
        const msg = e.message ?? {};
        const ts: string = typeof e.timestamp === "string" ? e.timestamp : "";
        if (msg.role === "user") {
          messages.user++;
          sess.messages++;
        } else if (msg.role === "assistant") {
          messages.assistant++;
          sess.messages++;
          const u = msg.usage ?? {};
          const t = typeof u.totalTokens === "number" ? u.totalTokens : 0;
          tokens.input += num(u.input);
          tokens.output += num(u.output);
          tokens.cacheRead += num(u.cacheRead);
          tokens.cacheWrite += num(u.cacheWrite);
          tokens.total += t;
          cost += num(u.cost?.total);
          sess.tokens += t;
          const key = `${msg.provider ?? entry.provider ?? "?"}/${msg.model ?? entry.model ?? "?"}`;
          const m = perModel.get(key) ?? { sessions: new Set<string>(), messages: 0, tokens: 0 };
          m.sessions.add(sid);
          m.messages++;
          m.tokens += t;
          perModel.set(key, m);
          if (ts) {
            const day = ts.slice(0, 10);
            const d = perDay.get(day) ?? { messages: 0, tokens: 0 };
            d.messages++;
            d.tokens += t;
            perDay.set(day, d);
          }
        }
      }
      perSession.set(sid, sess);
    }

    const byModel = [...perModel.entries()]
      .map(([key, m]) => ({ key, sessions: m.sessions.size, messages: m.messages, tokens: m.tokens }))
      .sort((a, b) => b.tokens - a.tokens);
    const byDay = [...perDay.entries()]
      .map(([day, d]) => ({ day, messages: d.messages, tokens: d.tokens }))
      .sort((a, b) => (a.day < b.day ? -1 : 1));
    const topSessions = entries
      .map((e) => ({
        session_id: e.session_id,
        ...(e.name ? { name: e.name } : {}),
        cwd: e.cwd,
        ...(e.model ? { model: e.model } : {}),
        messages: perSession.get(e.session_id)?.messages ?? 0,
        tokens: perSession.get(e.session_id)?.tokens ?? 0,
        created_at: e.created_at,
      }))
      .filter((s) => s.messages > 0)
      .sort((a, b) => b.tokens - a.tokens)
      .slice(0, 8);

    return { scanned_sessions: scanned, messages, tokens, cost, by_model: byModel, by_day: byDay, top_sessions: topSessions };
  }

  /**
   * Replay persisted events derived from session entries (live pool entry or
   * closed session file). Used by the desktop UI for history and by the sync
   * layer for backfill after reconnect.
   */
  async replayEvents(params: {
    session_id: string;
    after_seq?: number;
  }): Promise<{ events: Array<{ seq: number; ts: string; kind: string; payload: unknown }> }> {
    let entries: Record<string, any>[];
    const tracked = this.sessions.get(params.session_id);
    if (tracked) {
      entries = tracked.session.sessionManager.getEntries() as unknown as Record<string, any>[];
    } else {
      const file = this.readRegistry().find((e) => e.session_id === params.session_id)?.file;
      if (!file) throw new RpcError("session_not_found", `unknown session "${params.session_id}"`);
      const { parseSessionEntries } = await import("@earendil-works/pi-coding-agent");
      const raw = parseSessionEntries(readFileSync(file, "utf8")) as unknown as Record<string, any>[];
      entries = raw.filter((e) => e?.type !== "session");
    }
    const events: Array<{ seq: number; ts: string; kind: string; payload: unknown }> = [];
    entries.forEach((entry, index) => {
      const seq = index + 1;
      if (params.after_seq !== undefined && seq <= params.after_seq) return;
      const mapped = entryToPayload(entry);
      if (!mapped) return;
      events.push({
        seq,
        ts: typeof entry.timestamp === "string" ? entry.timestamp : new Date().toISOString(),
        kind: mapped.kind,
        payload: mapped.payload,
      });
    });
    return { events };
  }

  // ---------------------------------------------------------------- agent

  async prompt(params: {
    session_id: string;
    text: string;
    /** 图片附件（base64，不带 data: 前缀），随 prompt 一起发给模型 */
    images?: Array<{ data: string; mime_type: string }>;
    /** 文档附件（txt/pdf/office 等），host 侧提取文本后追加到 prompt；图片类路由进 images */
    attachments?: IncomingAttachment[];
  }): Promise<{ accepted: boolean }> {
    const tracked = this.require(params.session_id);
    this.autoTitle(params.session_id, params.text);
    const images = (params.images ?? [])
      .slice(0, 6)
      .filter((img) => img?.data)
      .map((img) => ({ type: "image" as const, data: img.data, mimeType: img.mime_type || "image/png" }));
    // /expert:name args → 以该专家身份发言：整条消息替换为专家块（与 pi 的
    // /skill: 展开同构，展开后的文本就是持久化的用户消息）；未知专家原样透传。
    // 这是会话中途"咨询"专家的通道——完整雇佣（技能/工具过滤）只在创建会话时生效。
    const expertCall = matchExpertCommand(params.text);
    let promptText = params.text;
    if (expertCall) {
      const found = this.experts.get({ name: expertCall.name }).expert;
      if (found) promptText = buildExpertPromptBlock(found, expertCall.args);
    }
    const parsed = await parseAttachments(params.attachments);
    const allImages = [...images, ...parsed.images].slice(0, 6);
    const fullText = parsed.blocks.length ? `${promptText}\n\n${parsed.blocks.join("\n\n")}` : promptText;
    tracked.session
      .prompt(fullText, allImages.length ? { images: allImages } : undefined)
      .catch((err) => emitEvent(params.session_id, Event.ERROR, { message: String(err) }))
      .finally(() => tracked.drain());
    return { accepted: true };
  }

  /**
   * 权限模式对修改类工具（bash/write/edit）的拦截与审批。
   * 作为内置扩展在会话创建时注入，tool_call 处理器可异步等待用户审批。
   */
  private async enforcePermission(
    sessionId: string,
    permission: { mode: PermissionMode },
    toolName: string,
    input: unknown,
  ): Promise<{ block: true; reason: string } | undefined> {
    const mode = permission.mode;
    if (mode === "full") return undefined;
    const mutating = toolName === "bash" || toolName === "write" || toolName === "edit";
    if (!mutating) return undefined;
    if (mode === "plan") {
      return { block: true, reason: "计划模式下不允许执行修改类工具（bash/write/edit）" };
    }
    if (mode === "edit-auto" && toolName !== "bash") return undefined;
    // confirm 模式（或 auto-edit 下的 bash）：请求用户审批并等待回复
    const approvalId = randomUUID();
    const pending = new Promise<boolean>((resolve) => {
      this.pendingApprovals.set(approvalId, { sessionId, resolve });
    });
    emitEvent(
      sessionId,
      Event.TOOL_APPROVAL,
      { approval_id: approvalId, tool_name: toolName, args: JSON.stringify(input ?? {}) },
      { persist: false },
    );
    emitEvent(sessionId, Event.AGENT_STATE_CHANGED, { state: "waiting_approval" }, { persist: false });
    const approved = await pending;
    emitEvent(sessionId, Event.AGENT_STATE_CHANGED, { state: "thinking" }, { persist: false });
    return approved ? undefined : { block: true, reason: "用户拒绝了本次工具调用" };
  }

  /**
   * AskUserQuestion 的等待端：发 ask_user_question 事件（UI 弹出问题卡）并挂起，
   * 直到 session.resolve_ask 回传答案；abort/关闭会话时由 denyAsks 兜底为取消，
   * 超时（pidock/settings.json ask.timeoutSec，默认 180 秒，0 = 不限时）按取消
   * 处理但回传 "timeout" 哨兵，模型据此自行决策继续而非视为拒绝。
   */
  /**
   * AskUserQuestion 的等待端：发 ask_user_question 事件（UI 弹出问题卡 + 倒计时）
   * 并挂起，直到 session.resolve_ask 回传答案；超时（pidock/settings.json
   * ask.timeoutSec，默认 180 秒，0 = 不限时）交由 onAskTimeout 处理——full 模式
   * 自动采用模型推荐项，其余模式按取消处理；abort/关闭会话由 denyAsks 兜底为取消。
   */
  private askUser(sessionId: string, question: AskQuestion): Promise<AskAnswer | "timeout" | null> {
    if (!sessionId || !this.sessions.has(sessionId)) return Promise.resolve(null);
    const askId = randomUUID();
    const pending = new Promise<AskAnswer | "timeout" | null>((resolve) => {
      this.pendingAsks.set(askId, { sessionId, resolve });
    });
    const timeoutMs = this.askTimeoutMs();
    emitEvent(
      sessionId,
      Event.ASK_USER_QUESTION,
      { ask_id: askId, question, timeout_sec: Math.round(timeoutMs / 1000) },
      { persist: false },
    );
    emitEvent(sessionId, Event.AGENT_STATE_CHANGED, { state: "waiting_ask" }, { persist: false });
    const timer =
      timeoutMs > 0
        ? setTimeout(() => this.onAskTimeout(sessionId, askId, question), timeoutMs)
        : null;
    return pending.finally(() => {
      if (timer) clearTimeout(timer);
      // 会话已关闭时不再推状态（socket 对端已消失，事件无人消费）
      if (this.sessions.has(sessionId)) {
        emitEvent(sessionId, Event.AGENT_STATE_CHANGED, { state: "thinking" }, { persist: false });
      }
    });
  }

  /** AskUserQuestion 等待超时（毫秒）：settings.json ask.timeoutSec，默认 180s，0/负 = 不限时，上限 1 小时防呆 */
  private askTimeoutMs(): number {
    const DEFAULT_MS = 3 * 60 * 1000;
    try {
      const raw = readFileSync(join(getAgentDir(), "pidock", "settings.json"), "utf8");
      const sec = Number(JSON.parse(raw)?.ask?.timeoutSec);
      if (!Number.isFinite(sec)) return DEFAULT_MS;
      if (sec <= 0) return 0;
      return Math.min(sec, 3600) * 1000;
    } catch {
      return DEFAULT_MS;
    }
  }

  setPermissionMode(params: { session_id: string; mode: PermissionMode }): { ok: true } {
    const state = this.permissionStates.get(params.session_id);
    if (!state) throw new RpcError("session_not_found", `unknown session "${params.session_id}"`);
    if (!PERMISSION_MODES.includes(params.mode)) {
      throw new RpcError("bad_request", `invalid permission mode "${params.mode}"`);
    }
    state.mode = params.mode;
    return { ok: true };
  }

  /** 重命名会话（写入注册表 name 字段，供侧栏展示；空名清除自定义名） */
  renameSession(params: { session_id: string; name: string }): { ok: true; name: string } {
    const entry = this.readRegistry().find((e) => e.session_id === params.session_id);
    if (!entry) throw new RpcError("session_not_found", `unknown session "${params.session_id}"`);
    const name = params.name.trim();
    const updated: RegistryEntry = { ...entry };
    if (name) updated.name = name;
    else delete updated.name;
    this.writeRegistryEntry(updated);
    return { ok: true, name };
  }

  /**
   * 列出工作区文件（输入框 @ 提及选择器）。
   * 受限遍历：最大深度/数量上限，跳过依赖与构建目录，隐藏点开头文件。
   */
  listWorkspaceFiles(params: { cwd: string }): {
    files: Array<{ path: string; name: string; dir: boolean }>;
  } {
    const root = params.cwd?.trim();
    if (!root || !existsSync(root)) return { files: [] };
    const IGNORE_DIRS = new Set([
      "node_modules", ".git", ".pi", "target", "dist", "build", "out",
      "__pycache__", ".venv", "venv", "vendor", ".next", ".nuxt", ".cache",
    ]);
    const MAX_FILES = 800;
    const MAX_DEPTH = 6;
    const files: Array<{ path: string; name: string; dir: boolean }> = [];
    const walk = (dir: string, rel: string, depth: number): void => {
      if (depth > MAX_DEPTH || files.length >= MAX_FILES) return;
      let entries;
      try {
        entries = readdirSync(dir, { withFileTypes: true });
      } catch {
        return; // 无权限等读取失败时跳过该目录
      }
      entries.sort((a, b) => a.name.localeCompare(b.name));
      for (const e of entries) {
        if (files.length >= MAX_FILES) return;
        if (e.name.startsWith(".")) continue;
        const relPath = rel ? `${rel}/${e.name}` : e.name;
        if (e.isDirectory()) {
          if (IGNORE_DIRS.has(e.name)) continue;
          files.push({ path: relPath, name: e.name, dir: true });
          walk(join(dir, e.name), relPath, depth + 1);
        } else if (e.isFile()) {
          files.push({ path: relPath, name: e.name, dir: false });
        }
      }
    };
    walk(resolve(root), "", 0);
    return { files };
  }

  /**
   * 读取工作区内的文本文件（文件浏览预览用）：路径解析后必须仍落在 cwd 内
   * （防目录穿越），只取前 256KB（超出标记 truncated），头部含 NUL 视为二进制。
   */
  readWorkspaceFile(params: { cwd: string; path: string }): {
    text: string;
    truncated: boolean;
    binary: boolean;
    size: number;
  } {
    const root = resolve(String(params.cwd ?? ""));
    if (!root || !existsSync(root)) throw new RpcError("bad_cwd", "workspace directory not found");
    const rel = String(params.path ?? "").replace(/\\/g, "/");
    if (!rel || rel.includes("\0")) throw new RpcError("bad_path", "file path required");
    const abs = resolve(root, rel);
    const relCheck = relative(root, abs);
    if (relCheck.startsWith("..") || isAbsolute(relCheck)) {
      throw new RpcError("bad_path", "path escapes workspace");
    }
    let st;
    try {
      st = statSync(abs);
    } catch {
      throw new RpcError("not_found", `no such file: ${rel}`);
    }
    if (!st.isFile()) throw new RpcError("not_found", `not a file: ${rel}`);
    const LIMIT = 256 * 1024;
    const buf = Buffer.alloc(Math.min(st.size, LIMIT));
    const fd = openSync(abs, "r");
    try {
      const got = readSync(fd, buf, 0, buf.length, 0);
      const head = (got === buf.length ? buf : buf.subarray(0, got));
      return {
        text: head.toString("utf8"),
        truncated: st.size > got,
        binary: head.subarray(0, 8192).includes(0),
        size: st.size,
      };
    } finally {
      closeSync(fd);
    }
  }

  resolveApproval(params: { approval_id: string; approved: boolean }): { ok: boolean } {
    const pending = this.pendingApprovals.get(params.approval_id);
    if (!pending) return { ok: false };
    this.pendingApprovals.delete(params.approval_id);
    pending.resolve(params.approved);
    return { ok: true };
  }

  /**
   * 回答挂起的 AskUserQuestion（labels=选中项 / text=自由输入 / interrupted=取消 /
   * timeout=等待超时）。空答案按取消处理（UI 不会提交，防御异常调用方）；unknown ask_id 幂等。
   */
  resolveAsk(params: {
    ask_id: string;
    labels?: string[];
    text?: string;
    interrupted?: boolean;
    timeout?: boolean;
  }): { ok: boolean } {
    if (params.interrupted) return { ok: this.settleAsk(params.ask_id, null) };
    if (params.timeout) return { ok: this.settleAsk(params.ask_id, "timeout") };
    const labels = (Array.isArray(params.labels) ? params.labels : []).filter(
      (s): s is string => typeof s === "string" && !!s.trim(),
    );
    const text =
      typeof params.text === "string" && params.text.trim()
        ? params.text.trim().slice(0, 2000)
        : undefined;
    if (!labels.length && !text) return { ok: this.settleAsk(params.ask_id, null) };
    return { ok: this.settleAsk(params.ask_id, { labels, ...(text ? { text } : {}) }) };
  }

  /** 兑现挂起提问（ask_id 不存在时幂等返回 false） */
  private settleAsk(askId: string, answer: AskAnswer | "timeout" | null): boolean {
    const pending = this.pendingAsks.get(askId);
    if (!pending) return false;
    this.pendingAsks.delete(askId);
    pending.resolve(answer);
    return true;
  }

  /**
   * 提问超时：full 模式下自动采用模型声明的推荐项作答（无人值守也不停摆，
   * 答案带 auto 标注回给模型）；无有效推荐项或非 full 模式按等待超时处理。
   */
  private onAskTimeout(sessionId: string, askId: string, question: AskQuestion): void {
    if (this.permissionStates.get(sessionId)?.mode === "full") {
      const rec = question.recommended;
      const label = typeof rec === "number" ? question.options[rec]?.label : undefined;
      if (label && this.settleAsk(askId, { labels: [label], auto: true })) return;
    }
    this.settleAsk(askId, "timeout");
  }

  private denyApprovals(sessionId: string): void {
    for (const [id, pending] of this.pendingApprovals) {
      if (pending.sessionId === sessionId) {
        this.pendingApprovals.delete(id);
        pending.resolve(false);
      }
    }
  }

  /** 会话中止/关闭时把挂起提问按用户取消处理，避免工具卡死在等待 */
  private denyAsks(sessionId: string): void {
    for (const [id, pending] of this.pendingAsks) {
      if (pending.sessionId === sessionId) {
        this.pendingAsks.delete(id);
        pending.resolve(null);
      }
    }
  }

  setThinkingLevel(params: { session_id: string; level: string }): { ok: true } {
    const tracked = this.require(params.session_id);
    if (!THINKING_LEVELS.includes(params.level)) {
      throw new RpcError("bad_request", `invalid thinking level "${params.level}"`);
    }
    tracked.session.setThinkingLevel(params.level as never);
    return { ok: true };
  }

  /** 按名称（id/name/provider/id）唯一匹配后切换会话模型 */
  async setModel(params: { session_id: string; model: string }): Promise<{ ok: true; model: string }> {
    const tracked = this.require(params.session_id);
    const mr = await this.modelRuntime();
    const target = params.model.trim().toLowerCase();
    const matches: Array<{ provider: string; id: string }> = [];
    for (const p of mr.getProviders()) {
      for (const m of mr.getModels(p.id)) {
        const id = String(m.id ?? "").toLowerCase();
        const name = String(m.name ?? "").toLowerCase();
        if (id === target || name === target || `${p.id}/${id}` === target) {
          matches.push({ provider: p.id, id: m.id });
        }
      }
    }
    const match = matches[0];
    if (!match) {
      throw new RpcError("model_not_found", `no model matching "${params.model}"`);
    }
    if (matches.length > 1) {
      throw new RpcError("ambiguous_model", `multiple models match "${params.model}"`);
    }
    const resolved = await this.resolveModel(`${match.provider}/${match.id}`);
    await tracked.session.setModel(resolved as never);
    const entry = this.readRegistry().find((e) => e.session_id === params.session_id);
    if (entry) {
      entry.model = `${match.provider}/${match.id}`;
      this.writeRegistryEntry(entry);
    }
    return { ok: true, model: `${match.provider}/${match.id}` };
  }

  // ---------------------------------------------------------- file changes

  /** 快照落盘目录：~/.pi/agent/pidock/snapshots/<session_id>.json（重启后审查/撤销仍可用） */
  private snapshotsDir(): string {
    return join(getAgentDir(), "pidock", "snapshots");
  }

  private snapshotPath(sessionId: string): string {
    return join(this.snapshotsDir(), `${sessionId}.json`);
  }

  /** 取会话的回合快照组；内存没有时从磁盘懒加载（重启恢复，兼容旧版单轮/纯数组格式） */
  private loadSnapshots(
    sessionId: string,
  ): Map<string, Map<string, { existed: boolean; content: Buffer }>> {
    const cached = this.fileSnapshots.get(sessionId);
    if (cached) return cached;
    const turns = new Map<string, Map<string, { existed: boolean; content: Buffer }>>();
    try {
      const raw: unknown = JSON.parse(readFileSync(this.snapshotPath(sessionId), "utf8"));
      // 新格式 { turns: [{ turn_id, files }] }；旧格式 { turn_id, files } / 纯数组归入 "" 轮
      let list: Array<Record<string, unknown>> = [];
      let legacyTurnId: string | undefined;
      if (Array.isArray(raw)) {
        legacyTurnId = "";
        list = (raw as Array<Record<string, unknown>>).map((f) => ({ turn_id: "", files: [f] }));
      } else {
        const obj = raw as Record<string, unknown>;
        if (Array.isArray(obj?.turns)) {
          list = obj.turns as Array<Record<string, unknown>>;
        } else if (Array.isArray(obj?.files)) {
          legacyTurnId = typeof obj.turn_id === "string" ? obj.turn_id : "";
          list = [{ turn_id: legacyTurnId, files: obj.files }];
        }
      }
      for (const t of list) {
        const files = new Map<string, { existed: boolean; content: Buffer }>();
        for (const e of (t?.files ?? []) as Array<Record<string, unknown>>) {
          if (typeof e?.path === "string" && typeof e?.content === "string") {
            files.set(e.path, {
              existed: Boolean(e.existed),
              content: Buffer.from(e.content, "base64"),
            });
          }
        }
        const turnId = typeof t?.turn_id === "string" ? t.turn_id : "";
        const existing = turns.get(turnId);
        if (existing) for (const [k, v] of files) existing.set(k, v);
        else turns.set(turnId, files);
      }
    } catch {
      // 无落盘文件或损坏 → 空表
    }
    this.fileSnapshots.set(sessionId, turns);
    return turns;
  }

  /** 把会话的内存快照写盘；空表时移除落盘文件 */
  private persistSnapshots(sessionId: string): void {
    const turns = this.fileSnapshots.get(sessionId);
    try {
      if (!turns || turns.size === 0) {
        rmSync(this.snapshotPath(sessionId), { force: true });
        return;
      }
      mkdirSync(this.snapshotsDir(), { recursive: true });
      const data = {
        turns: [...turns.entries()].map(([turn_id, files]) => ({
          turn_id,
          files: [...files.entries()].map(([path, snap]) => ({
            path,
            existed: snap.existed,
            content: snap.content.toString("base64"),
          })),
        })),
      };
      writeFileSync(this.snapshotPath(sessionId), JSON.stringify(data));
    } catch {
      // 落盘失败不影响内存快照
    }
  }

  /** 取「当前回合」的快照桶（按需创建）；超上限时淘汰最旧的回合 */
  private turnBucket(sessionId: string): Map<string, { existed: boolean; content: Buffer }> {
    const turns = this.loadSnapshots(sessionId);
    const tracked = this.sessions.get(sessionId);
    const turnId = tracked ? this.currentTurnId(tracked) : "";
    let files = turns.get(turnId);
    if (!files) {
      files = new Map();
      turns.set(turnId, files);
      if (turns.size > 30) {
        const oldest = turns.keys().next().value;
        if (oldest !== undefined) turns.delete(oldest);
      }
    }
    return files;
  }

  /** 启动时清理 registry 里已不存在会话的快照落盘文件 */
  private pruneSnapshots(): void {    try {
      const dir = this.snapshotsDir();
      if (!existsSync(dir)) return;
      const known = new Set(this.readRegistry().map((e) => e.session_id));
      for (const f of readdirSync(dir)) {
        if (f.endsWith(".json") && !known.has(f.slice(0, -5))) {
          rmSync(join(dir, f), { force: true });
        }
      }
    } catch {
      // 清理失败无碍运行
    }
  }

  /**
   * 目录元数据扫描（路径 → mtime+size），供 bash 前后 diff 出新建文件。
   * 只存元数据不存内容；跳过依赖/产物目录，限制条目数与深度防止大仓卡顿。
   */
  private scanTreeMeta(root: string): Map<string, { mtime: number; size: number }> {
    const SKIP_DIRS = new Set([
      "node_modules", ".git", "target", "dist", "build", ".next", "out",
      "__pycache__", ".venv", "venv", "coverage", ".idea", ".vscode", ".cache",
    ]);
    const MAX_FILES = 20_000;
    const MAX_DEPTH = 10;
    const out = new Map<string, { mtime: number; size: number }>();
    const walk = (dir: string, depth: number): void => {
      if (depth > MAX_DEPTH || out.size >= MAX_FILES) return;
      let entries;
      try {
        entries = readdirSync(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of entries) {
        if (out.size >= MAX_FILES) return;
        const abs = join(dir, e.name);
        if (e.isDirectory()) {
          if (!SKIP_DIRS.has(e.name)) walk(abs, depth + 1);
        } else if (e.isFile()) {
          try {
            const st = statSync(abs);
            out.set(abs, { mtime: st.mtimeMs, size: st.size });
          } catch {
            // 不可读的条目跳过
          }
        }
      }
    };
    walk(root, 0);
    return out;
  }

  /** bash 执行前：记录目录元数据基线 */
  private bashScanStart(sessionId: string, cwd: string): void {
    this.bashPreScans.set(sessionId, this.scanTreeMeta(cwd));
  }

  /**
   * bash 执行后：与基线 diff，把「新建文件」补入快照（ existed=false，
   * 撤销时删除、卡片显示全部新增行）。bash 修改已有文件的原始内容无法
   * 回取，不纳入快照 —— edit/write 路径的快照不受影响。
   */
  private bashScanEnd(sessionId: string, cwd: string): void {
    const pre = this.bashPreScans.get(sessionId);
    this.bashPreScans.delete(sessionId);
    if (!pre) return;
    const post = this.scanTreeMeta(cwd);
    const files = this.turnBucket(sessionId);
    let added = 0;
    for (const [abs, meta] of post) {
      const before = pre.get(abs);
      if (before) continue; // 已存在（无论内容是否变化）→ 无法回取原文，跳过
      if (files.has(abs)) continue; // edit/write 已有更早快照，保留最早版本
      if (added >= 200) break; // 单次 bash 命令的追踪上限
      try {
        const st = statSync(abs);
        if (!st.isFile() || st.size > 4 * 1024 * 1024) continue;
        files.set(abs, { existed: false, content: Buffer.alloc(0) });
        added++;
      } catch {
        // 竞态中被删除/不可读 → 跳过
      }
    }
    if (added > 0) this.persistSnapshots(sessionId);
  }

  /** 当前回合 id：最后一条用户消息条目的官方 id（以会话条目为准，无事件时序竞态） */
  private currentTurnId(tracked: TrackedSession): string {
    const entries = tracked.session.sessionManager.getEntries() as unknown as Array<
      Record<string, any>
    >;
    for (let i = entries.length - 1; i >= 0; i--) {
      const e = entries[i];
      if (e?.type === "message" && (e.message as any)?.role === "user" && typeof e.id === "string") {
        return e.id;
      }
    }
    return "";
  }

  /** 修改类工具执行前快照目标文件（每回合每路径只保留最早的版本，快照归入所属回合） */  private snapshotBeforeMutation(sessionId: string, cwd: string, toolName: string, input: unknown): void {
    if (toolName !== "edit" && toolName !== "write") return;
    const a = (input ?? {}) as Record<string, unknown>;
    const rel = String(a.path ?? "");
    if (!rel) return;
    const files = this.turnBucket(sessionId);
    const abs = resolve(cwd, rel);
    if (files.has(abs)) return;
    try {
      const st = statSync(abs);
      if (!st.isFile() || st.size > 4 * 1024 * 1024) return; // 过大不快照，不参与撤销
      files.set(abs, { existed: true, content: readFileSync(abs) });
    } catch {
      files.set(abs, { existed: false, content: Buffer.alloc(0) }); // 新建文件
    }
    this.persistSnapshots(sessionId);
  }

  /** 回合内被修改的文件列表：按回合分组返回（快照 vs 磁盘当前内容的行级统计） */
  fileChanges(params: { session_id: string }): {
    turns: Array<{
      turn_id: string;
      files: Array<{ path: string; added: number; removed: number; isNew: boolean }>;
    }>;
  } {
    const turns = this.loadSnapshots(params.session_id);
    const out: Array<{
      turn_id: string;
      files: Array<{ path: string; added: number; removed: number; isNew: boolean }>;
    }> = [];
    for (const [turnId, files] of turns) {
      const list = [...files.entries()].map(([abs, snapFile]) => {
        let current: string | null = null;
        try {
          if (existsSync(abs) && statSync(abs).isFile()) current = readFileSync(abs, "utf8");
        } catch {
          current = null;
        }
        const oldText = snapFile.existed ? snapFile.content.toString("utf8") : "";
        // 末尾换行不产生空行（split("\n") 会多出一个尾元素）
        const splitLines = (t: string): string[] => {
          if (!t) return [];
          const ls = t.split("\n");
          if (ls[ls.length - 1] === "") ls.pop();
          return ls;
        };
        const oldLines = splitLines(oldText);
        const newLines = splitLines(current ?? "");
        const oldSet = new Set(oldLines);
        const newSet = new Set(newLines);
        return {
          path: abs,
          added: newLines.filter((l) => !oldSet.has(l)).length,
          removed: oldLines.filter((l) => !newSet.has(l)).length,
          isNew: !snapFile.existed,
        };
      });
      // 净变更为零的文件（回合内改回原样）不进卡片；新建文件保留（撤销时需删除）
      const filtered = list.filter((f) => f.isNew || f.added > 0 || f.removed > 0);
      if (filtered.length) out.push({ turn_id: turnId, files: filtered });
    }
    return { turns: out };
  }

  /** 单个文件的快照与当前内容（供 UI 渲染 diff）；turn_id 缺省取最近回合 */
  fileDiff(params: { session_id: string; path: string; turn_id?: string }): {
    oldText: string;
    newText: string;
  } {
    const turns = this.loadSnapshots(params.session_id);
    const files = params.turn_id ? turns.get(params.turn_id) : [...turns.values()].at(-1);
    const abs = resolve(params.path ?? "");
    const snapFile = files?.get(abs);
    if (!snapFile) throw new RpcError("not_found", "no snapshot for this path");
    let newText = "";
    try {
      if (existsSync(abs) && statSync(abs).isFile()) newText = readFileSync(abs, "utf8");
    } catch {
      newText = "";
    }
    return { oldText: snapFile.existed ? snapFile.content.toString("utf8") : "", newText };
  }

  /** 撤销指定回合（缺省最近回合）的快照文件（新建的删除），返回恢复数量 */
  revertFiles(params: { session_id: string; turn_id?: string }): { reverted: number } {
    const turns = this.loadSnapshots(params.session_id);
    const turnId = params.turn_id ?? [...turns.keys()].at(-1);
    const files = turnId !== undefined ? turns.get(turnId) : undefined;
    if (!files || files.size === 0) return { reverted: 0 };
    let reverted = 0;
    for (const [abs, snapFile] of files) {
      try {
        if (snapFile.existed) writeFileSync(abs, snapFile.content);
        else if (existsSync(abs)) rmSync(abs);
        reverted++;
      } catch {
        // 无法恢复的文件跳过
      }
    }
    turns.delete(turnId!);
    this.persistSnapshots(params.session_id);
    return { reverted };
  }

  /**
   * 会话尚无标题时，用首条用户消息生成（registry 持久化 + SESSION_META 实时推送）。
   * pi 本体只在 TUI /name 命令里命名会话，无头 RPC 模式不会有名字。
   */
  private autoTitle(sessionId: string, text: string): void {
    const entry = this.readRegistry().find((e) => e.session_id === sessionId);
    if (!entry || entry.name) return;
    const title = text.replace(/\s+/g, " ").trim().slice(0, 40);
    if (!title) return;
    entry.name = title;
    this.writeRegistryEntry(entry);
    emitEvent(sessionId, Event.SESSION_META, { name: title });
  }

  steer(params: { session_id: string; text: string }): { accepted: boolean } {
    const tracked = this.require(params.session_id);
    tracked.session
      .steer(params.text)
      .catch((err) => emitEvent(params.session_id, Event.ERROR, { message: String(err) }));
    return { accepted: true };
  }

  followUp(params: { session_id: string; text: string }): { accepted: boolean } {
    const tracked = this.require(params.session_id);
    tracked.session
      .followUp(params.text)
      .catch((err) => emitEvent(params.session_id, Event.ERROR, { message: String(err) }));
    return { accepted: true };
  }

  async abort(params: { session_id: string }): Promise<{ aborted: boolean }> {
    const tracked = this.require(params.session_id);
    // 中断时挂起的工具审批全部按拒绝处理，避免工具卡在等待状态
    this.denyApprovals(params.session_id);
    // 挂起的提问按用户取消处理（工具返回 interrupted，模型自行收尾）
    this.denyAsks(params.session_id);
    await tracked.session.abort();
    return { aborted: true };
  }

  // ---------------------------------------------------------------- wiring

  private require(sessionId: string): TrackedSession {
    const tracked = this.sessions.get(sessionId);
    if (!tracked) throw new RpcError("session_not_found", `session "${sessionId}" is not open`);
    return tracked;
  }

  /** subscribe to pi session events and map them to pidock envelopes */
  private wire(sessionId: string, tracked: TrackedSession): () => void {
    const snap = tracked.snapshot;

    /**
     * Emit persistable events for entries not yet drained. The SessionManager
     * entry array is the sole source of truth: live pipeline and replay both
     * map entry index -> seq, so the two can never disagree.
     */
    const drainEntries = (): void => {
      const entries = tracked.session.sessionManager.getEntries() as unknown as Record<string, any>[];
      if (entries.length < tracked.lastSeq) {
        // compaction rewrote the session: history shrank, so the cloud copy
        // must be rebuilt — server deletes the old events on this marker
        emitEvent(sessionId, Event.SESSION_RESYNCED, { reason: "compaction" });
        tracked.lastSeq = 0;
      }
      while (tracked.lastSeq < entries.length) {
        tracked.lastSeq += 1;
        const entry = entries[tracked.lastSeq - 1];
        if (!entry) continue;
        let messageId: string | undefined;
        if (entry?.type === "message" && (entry.message as any)?.role === "assistant") {
          messageId = tracked.pendingTurnId ?? `${sessionId}:assistant#${tracked.lastSeq}`;
          tracked.pendingTurnId = null;
        }
        const mapped = entryToPayload(entry, messageId);
        if (mapped) {
          emitEvent(sessionId, mapped.kind, mapped.payload, { persist: true, seq: tracked.lastSeq });
        }
      }
    };
    // safety net for entry appends that coincide with no SDK event
    const drainTimer = setInterval(drainEntries, 1000);
    tracked.drain = drainEntries;

    /**
     * Snapshot throttle: inline, time-based. On each delta, if enough time has
     * passed since the last snapshot, push the accumulated partial content.
     * (A timer-based flush races with short turns; the inline check cannot.)
     */
    const maybeSnapshot = (): void => {
      if (!snap.dirty) return;
      const now = Date.now();
      if (now - snap.lastEmit < SNAPSHOT_INTERVAL_MS) return;
      snap.lastEmit = now;
      snap.dirty = false;
      emitEvent(sessionId, Event.MESSAGE_SNAPSHOT, {
        message_id: snap.messageId ?? `${sessionId}:assistant`,
        text: snap.text,
        thinking: snap.thinking,
      });
    };

    const resetSnap = (): void => {
      snap.messageId = null;
      snap.text = "";
      snap.thinking = "";
      snap.dirty = false;
    };

    const unsubscribe = tracked.session.subscribe((event) => {
      // entries may have been appended since the previous event
      drainEntries();
      switch (event.type) {
        // ---- delta channel (local UI streaming) ----
        case "message_update": {
          const ame = event.assistantMessageEvent as Record<string, any>;
          const messageId = snap.messageId ?? `${sessionId}:assistant`;
          if (ame?.type === "text_delta" && typeof ame.delta === "string") {
            if (snap.messageId !== messageId) {
              snap.messageId = messageId;
              snap.text = "";
              snap.thinking = "";
            }
            snap.text += ame.delta;
            snap.dirty = true;
            maybeSnapshot();
            emitEvent(sessionId, Event.MESSAGE_DELTA, {
              message_id: messageId,
              part: "text",
              delta: ame.delta,
            });
          } else if (ame?.type === "thinking_delta" && typeof ame.delta === "string") {
            snap.thinking += ame.delta;
            snap.dirty = true;
            maybeSnapshot();
            emitEvent(sessionId, Event.MESSAGE_DELTA, {
              message_id: messageId,
              part: "thinking",
              delta: ame.delta,
            });
          }
          break;
        }

        // ---- ephemeral state ----
        case "agent_start":
          this.setState(tracked, sessionId, "thinking");
          break;
        case "turn_start":
          this.setState(tracked, sessionId, "thinking");
          break;
        case "message_start": {
          if ((event.message as Record<string, any>)?.role === "assistant") {
            tracked.turnCounter += 1;
            tracked.pendingTurnId = `${sessionId}#${tracked.turnCounter}`;
            snap.messageId = tracked.pendingTurnId;
            this.setState(tracked, sessionId, "responding");
          }
          break;
        }
        case "tool_execution_start": {
          this.setState(tracked, sessionId, "executing_tool");
          // bash 建文件绕过 edit/write 快照：执行前记录目录基线，结束后 diff 补快照
          if (event.toolName === "bash") this.bashScanStart(sessionId, tracked.cwd);
          const args = JSON.stringify(event.args ?? {});
          emitEvent(sessionId, Event.TOOL_EXECUTION_START, {
            call_id: event.toolCallId,
            tool_name: event.toolName,
            args: args.length > INLINE_PREVIEW_BYTES ? args.slice(0, INLINE_PREVIEW_BYTES) : args,
          });
          break;
        }
        case "tool_execution_update": {
          const partial =
            typeof event.partialResult === "string"
              ? event.partialResult
              : JSON.stringify(event.partialResult ?? "");
          emitEvent(sessionId, Event.TOOL_EXECUTION_UPDATE, {
            call_id: event.toolCallId,
            tool_name: event.toolName,
            partial: partial.length > INLINE_PREVIEW_BYTES ? partial.slice(0, INLINE_PREVIEW_BYTES) : partial,
          });
          break;
        }
        case "tool_execution_end": {
          this.setState(tracked, sessionId, "thinking");
          if (event.toolName === "bash") this.bashScanEnd(sessionId, tracked.cwd);
          const result =
            typeof event.result === "string"
              ? event.result
              : JSON.stringify(event.result ?? "");
          emitEvent(sessionId, Event.TOOL_EXECUTION_END, {
            call_id: event.toolCallId,
            tool_name: event.toolName,
            is_error: event.isError,
            output: result.length > ATTACHMENT_THRESHOLD_BYTES ? result.slice(0, INLINE_PREVIEW_BYTES) : result,
            ...(result.length > ATTACHMENT_THRESHOLD_BYTES ? { truncated: true } : {}),
          });
          break;
        }
        case "agent_end":
          if (!event.willRetry) this.setState(tracked, sessionId, "idle");
          resetSnap();
          break;
        case "agent_settled":
          this.setState(tracked, sessionId, "idle");
          resetSnap();
          break;
        case "queue_update":
          emitEvent(sessionId, Event.QUEUE_CHANGED, {
            steering_count: event.steering.length,
            follow_up_count: event.followUp.length,
          });
          break;
        case "compaction_start":
          this.setState(tracked, sessionId, "compacting");
          emitEvent(sessionId, Event.COMPACTION_LIFECYCLE, { phase: "start", reason: event.reason });
          break;
        case "compaction_end":
          this.setState(tracked, sessionId, event.willRetry ? "thinking" : "idle");
          emitEvent(sessionId, Event.COMPACTION_LIFECYCLE, {
            phase: "end",
            reason: event.reason,
            aborted: event.aborted,
            ...(event.errorMessage ? { error: event.errorMessage } : {}),
          });
          break;
        case "auto_retry_start":
          this.setState(tracked, sessionId, "retrying");
          emitEvent(sessionId, Event.AUTO_RETRY, {
            phase: "start",
            attempt: event.attempt,
            max_attempts: event.maxAttempts,
            error: event.errorMessage,
          });
          break;
        case "auto_retry_end":
          this.setState(tracked, sessionId, event.success ? "idle" : "thinking");
          emitEvent(sessionId, Event.AUTO_RETRY, {
            phase: "end",
            attempt: event.attempt,
            success: event.success,
            ...(event.finalError ? { error: event.finalError } : {}),
          });
          break;
        case "session_info_changed":
          emitEvent(sessionId, Event.SESSION_META, {
            ...(event.name !== undefined ? { name: event.name } : {}),
          });
          break;

        default:
          break;
      }
    });
    return () => {
      clearInterval(drainTimer);
      unsubscribe();
    };
  }

  disposeAll(): void {
    for (const [id, tracked] of this.sessions) {
        tracked.unsubscribe();
      try {
        tracked.session.dispose();
      } catch {
        // already disposed
      }
      this.sessions.delete(id);
    }
  }
}

/** re-export for the dispatcher */
export { Event as Events, type Block };
