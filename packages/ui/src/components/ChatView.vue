<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { NSplit } from "naive-ui";
import type { AgentStore, ExpertInfo, UiItem } from "../store.js";
import { formatSpan, greeting } from "../utils/time.js";
import FileIcon from "./FileIcon.vue";
import FilePreview from "./FilePreview.vue";
import FilesPanel from "./FilesPanel.vue";
import MdContent from "./MdContent.vue";
import ReviewPanel from "./ReviewPanel.vue";
import Composer from "./Composer.vue";
import Icon from "./Icon.vue";
import ProgressCard from "./ProgressCard.vue";
import SessionSettings from "./SessionSettings.vue";
import { appConfirm } from "../confirm.js";
import AskCard from "./AskCard.vue";

const props = withDefaults(
  defineProps<{
    store: AgentStore;
    disabled?: boolean;
    disabledHint?: string;
    model?: string;
    /** 新建任务模式：强制显示默认对话页，发送首条消息时自动创建会话 */
    newTask?: boolean;
    /** 新建任务预选的项目目录（来自侧栏项目分组点击），透传给首页输入卡 */
    newTaskCwd?: { cwd: string; seq: number } | null;
    /** 新任务预雇佣的专家（专家页「雇佣」按钮进入）；雇佣后随首条消息绑定到新会话 */
    newTaskExpert?: { id: string; name: string } | null;
    /** 项目文件浏览面板（侧栏项目右键「查看项目文件」）；seq 支持同项目重复触发刷新 */
    filesCwd?: { cwd: string; seq: number } | null;
  }>(),
  { disabled: false },
);

const emit = defineEmits<{
  "open-settings": [tab?: string];
  "open-providers": [];
  /** 关闭项目文件浏览面板 */
  "close-files": [];
}>();

const scroller = ref<HTMLElement | null>(null);
const preset = ref("");
const home = computed(() => props.newTask === true || !props.store.activeId);

/** 当前会话的权限模式 / 思考级别（缺省与 host 一致：新会话默认计划模式） */
const permissionMode = computed(
  () => (props.store.activeId ? props.store.permissionModes[props.store.activeId] : undefined) ?? "plan",
);
const thinkingLevel = computed(
  () => (props.store.activeId ? props.store.thinkingLevels[props.store.activeId] : undefined) ?? "medium",
);
/** 首页选中的模型（暂存，新建会话时生效） */
const homeModel = ref<string | null>(null);
/** 首页暂存的权限模式 / 思考级别（新建会话后下发给 host） */
const homePermissionMode = ref<string | null>(null);
const homeThinkingLevel = ref<string | null>(null);
/** 首页雇佣的专家（绑定到将创建的会话；专家页「雇佣」进入时预置） */
const hiredExpert = ref<{ id: string; name: string } | null>(null);
watch(
  () => props.newTaskExpert,
  (v) => {
    if (v) hiredExpert.value = v;
  },
  { immediate: true },
);
watch(home, (h) => {
  if (!h) hiredExpert.value = null;
});
/** 模型下拉可选项：全部已配置供应商的模型（provider/id），兜底合并历史会话里出现过的模型 */
const modelOptions = computed(() => {
  const set = new Set<string>();
  for (const m of props.store.allModels) {
    if (m.provider && m.id) set.add(`${m.provider}/${m.id}`);
  }
  for (const s of props.store.sessions) if (s.model) set.add(s.model);
  if (homeModel.value) set.add(homeModel.value);
  return [...set].sort();
});
/** 首页输入卡显示：暂存模型优先，其次沿用当前会话模型 */
const homeComposerModel = computed(() => homeModel.value ?? props.model);
/** 输入卡显示值：首页用暂存值，会话内用 store 值 */
const composerPermissionMode = computed(() => homePermissionMode.value ?? permissionMode.value);
const composerThinkingLevel = computed(() => homeThinkingLevel.value ?? thinkingLevel.value);
function setModel(m: string): void {
  if (home.value) homeModel.value = m;
  else void props.store.setModel(m);
}
function setPermissionMode(m: string): void {
  if (home.value) homePermissionMode.value = m;
  else void props.store.setPermissionMode(m);
}
function setThinkingLevel(l: string): void {
  if (home.value) homeThinkingLevel.value = l;
  else void props.store.setThinkingLevel(l);
}

/** @ 文件提及：基准目录（首页用主目录兜底，会话内用会话 cwd）与加载器 */
const activeSession = computed(() => props.store.sessions.find((s) => s.session_id === props.store.activeId));

// ---- 上下文用量仪表条 + 会话设置 ----
const showSessionSettings = ref(false);
const contextInfo = computed(() =>
  props.store.activeId ? props.store.contextBySession[props.store.activeId] : undefined,
);
/** 仪表条百分比（usage 未知时 null → 显示"待响应"） */
const contextPercent = computed(() => {
  const p = contextInfo.value?.percent;
  return typeof p === "number" && Number.isFinite(p) ? Math.min(100, Math.max(0, p)) : null;
});
/** token 数缩写：≥1M 显示 xM（整数），≥1K 显示 x.xK，其余原样 */
function fmtTokens(n: number): string {
  if (n >= 1024 * 1024) {
    const m = n / (1024 * 1024);
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (n >= 1024) return `${(n / 1024).toFixed(1)}K`;
  return String(n);
}
const contextTokensText = computed(() => {
  const t = contextInfo.value?.tokens;
  return typeof t === "number" && Number.isFinite(t) ? fmtTokens(t) : "";
});
const contextWindowText = computed(() => {
  const w = contextInfo.value?.context_window;
  return typeof w === "number" && Number.isFinite(w) ? fmtTokens(w) : "";
});
/** 仪表条颜色阈值：<70% 常态色，<90% 琥珀，≥90% 红 */
const contextMeterClass = computed(() => {
  const p = contextPercent.value;
  if (p === null) return "";
  if (p >= 90) return "hot";
  if (p >= 70) return "warm";
  return "";
});
/** 排队中的消息总数（steering + followUp） */
const queuedCount = computed(() => {
  const q = props.store.activeId ? props.store.queueBySession[props.store.activeId] : undefined;
  return (q?.steering ?? 0) + (q?.follow_up ?? 0);
});
/** 压缩进行中（compaction_lifecycle start/end） */
const compacting = computed(() => {
  const sid = props.store.activeId;
  return !!(sid && props.store.compactingBySession[sid]);
});
/** 自动重试状态（auto_retry 事件；非 null = 重试中） */
const retryInfo = computed(() => {
  const sid = props.store.activeId;
  return sid ? props.store.autoRetryBySession[sid] ?? null : null;
});
/** 最近一次压缩摘要（compaction_summary 落库事件，对话顶部折叠条） */
const compactionSummary = computed(() => {
  const sid = props.store.activeId;
  return sid ? props.store.compactionBySession[sid] ?? null : null;
});
const csOpen = ref(false);
/** 清空排队消息；被清的文本回填输入框（preset 是 Composer 的受控草稿通道） */
async function clearQueueToComposer(): Promise<void> {
  const texts = await props.store.clearQueue();
  if (texts.length) preset.value = texts.join("\n\n");
}
/** 手动压缩（确认后触发；压缩全程/结果由 compaction 事件与状态徽标呈现） */
async function compactConfirm(): Promise<void> {
  if (
    !(await appConfirm({
      title: "压缩上下文？",
      message: "把当前对话历史压缩成摘要以腾出上下文窗口。压缩期间无法继续对话，历史在磁盘保留。",
      confirmText: "开始压缩",
    }))
  )
    return;
  await props.store.compactSession();
}

/** 活动会话绑定的专家详情（第三栏信息卡）；专家已删除则为 null */
const activeExpert = ref<ExpertInfo | null>(null);
watch(
  () => activeSession.value?.expert_id,
  async (id) => {
    activeExpert.value = null;
    if (!id) return;
    activeExpert.value = await props.store.getExpert(id);
  },
  { immediate: true },
);
function mentionLoader(cwd: string): Promise<Array<{ path: string; name: string; dir: boolean }>> {
  return props.store.listWorkspaceFiles(cwd);
}
/** $ 技能列表加载器（会话内含项目级技能） */
function skillsLoader(): Promise<Array<{ name: string; description: string }>> {
  return props.store.listSkills(activeSession.value?.cwd);
}
/** / 弹层专家段加载器 */
function expertsLoader(): Promise<Array<{ id: string; name: string; description?: string; icon?: string }>> {
  return props.store.listExperts();
}
/** / 弹层命令模板段加载器（当前会话已加载的 prompts，未打开会话返回空） */
function promptsLoader(): Promise<Array<{ name: string; description: string; argument_hint: string }>> {
  return props.store.listPrompts(activeSession.value?.session_id);
}
/** 当前模型可用思考档位（thinking_info；首页/未知时 null = 全部展示） */
const thinkingAvailable = computed(() => {
  const info = props.store.activeId ? props.store.thinkingBySession[props.store.activeId] : undefined;
  return info ? info.levels : null;
});
/** 当前模型是否支持图片输入（未知 = undefined，不预警） */
const modelSupportsImages = computed<boolean | undefined>(() => {
  const cur = home.value ? (homeModel.value ?? props.model ?? "") : (props.model ?? "");
  if (!cur) return undefined;
  const hit = props.store.allModels.find((m) => `${m.provider}/${m.id}` === cur);
  if (!hit || !hit.input) return undefined;
  return hit.input.includes("image");
});

// distinct project dirs for the composer folder selector
const projects = computed(() => {
  const set = new Set<string>();
  for (const s of props.store.sessions) if (s.cwd) set.add(s.cwd);
  return [...set];
});

// ---- 活动流辅助 ----
const busy = computed(() => props.store.agentState !== "idle");
/** callId → 结果状态与输出（从 toolResult 消息汇总，合并进 toolCall 活动行） */
const resultsMap = computed(() => {
  const m: Record<string, { status: "done" | "error"; output: string }> = {};
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "toolResult") {
      for (const b of it.blocks ?? []) {
        if (b.callId) m[b.callId] = { status: b.isError ? "error" : "done", output: b.output ?? "" };
      }
    }
  }
  return m;
});
/** callId → 工具调用参数（assistant toolCall 块），供 toolResult 行回填展示 */
const argsMap = computed(() => {
  const m: Record<string, string> = {};
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "assistant") {
      for (const b of it.blocks ?? []) {
        if (b.type === "toolCall" && b.callId) m[b.callId] = b.args ?? "";
      }
    }
  }
  return m;
});
const assistantCallIds = computed(() => {
  const s = new Set<string>();
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "assistant") {
      for (const b of it.blocks ?? []) {
        if (b.type === "toolCall" && b.callId) s.add(b.callId);
      }
    }
  }
  return s;
});
function isMergedResult(item: UiItem): boolean {
  if (item.kind !== "message" || item.role !== "toolResult") return false;
  const blocks = item.blocks ?? [];
  return blocks.length > 0 && blocks.every((b) => b.callId !== undefined && assistantCallIds.value.has(b.callId));
}

/** 无任何可见内容的助手消息（回合尾部空回复）：不渲染，避免出现空白段；带错误信息的保留展示 */
function isBlankMessage(item: UiItem): boolean {
  if (item.kind !== "message" || item.role !== "assistant") return false;
  if (item.streaming) return false; // 流式占位保留光标
  if (item.errorMessage) return false;
  const blockHasContent = (item.blocks ?? []).some(
    (b) =>
      b.type === "toolCall" ||
      b.type === "image" ||
      (b.type === "text" && (b.text ?? "").trim() !== "") ||
      (b.type === "thinking" && (b.thinking ?? "").trim() !== ""),
  );
  return !blockHasContent && !(item.text ?? "").trim() && !(item.thinking ?? "").trim();
}
/** 工作中分隔线：插在最后一条用户消息之后，时长每秒跳动 */
const lastUserIndex = computed(() => {
  const items = props.store.items;
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it && it.kind === "message" && it.role === "user") return i;
  }
  return -1;
});
/** 回合分组：每条用户消息开启一个回合，结束后折叠为「已工作 · 耗时」（重启后依然折叠） */
interface TurnGroup {
  key: string;
  userIndex: number;
  endIndex: number;
  finished: boolean;
  duration: string | null;
  hasActivity: boolean;
}
const expandedTurns = ref(new Set<string>());
const turnGroups = computed(() => {
  const items = props.store.items;
  const userIdx: number[] = [];
  items.forEach((it, i) => {
    if (it.kind === "message" && it.role === "user") userIdx.push(i);
  });
  const groups: TurnGroup[] = [];
  userIdx.forEach((u, gi) => {
    const endIndex = gi + 1 < userIdx.length ? userIdx[gi + 1]! - 1 : items.length - 1;
    const isLast = gi === userIdx.length - 1;
    const finished = !isLast || !busy.value;
    const uIt = items[u];
    const lastIt = items[endIndex];
    let duration: string | null = null;
    if (isLast && props.store.turnStartedAt && props.store.turnEndedAt) {
      duration = formatSpan(props.store.turnEndedAt - props.store.turnStartedAt);
    } else if (uIt && uIt.kind === "message" && uIt.ts && lastIt && lastIt.kind === "message" && lastIt.ts) {
      const ms = new Date(lastIt.ts).getTime() - new Date(uIt.ts).getTime();
      if (ms > 0) duration = formatSpan(ms);
    }
    groups.push({
      // 分组键优先用官方回合 id（该轮用户消息的 session 条目 id），旧数据回退到消息 key
      key: uIt && uIt.kind === "message" ? (uIt.turnId ?? uIt.key) : `turn-${u}`,
      userIndex: u,
      endIndex,
      finished,
      duration,
      hasActivity: endIndex > u,
    });
  });
  return groups;
});
const groupByIndex = computed(() => {
  const m = new Map<number, TurnGroup>();
  for (const g of turnGroups.value) {
    for (let i = g.userIndex; i <= g.endIndex; i++) m.set(i, g);
  }
  return m;
});
function groupAt(index: number): TurnGroup | undefined {
  return groupByIndex.value.get(index);
}
function isTurnCollapsed(g: TurnGroup): boolean {
  return g.finished && !expandedTurns.value.has(g.key);
}
function finalTextIndexOf(g: TurnGroup): number {
  const items = props.store.items;
  for (let i = g.endIndex; i > g.userIndex; i--) {
    const it = items[i];
    if (it && it.kind === "message" && it.role === "assistant" && (it.text || (it.blocks ?? []).some((b) => b.type === "text"))) return i;
  }
  // 无文本回复时回退到带错误信息的助手消息（折叠态也要可见，避免整回合空白）
  for (let i = g.endIndex; i > g.userIndex; i--) {
    const it = items[i];
    if (it && it.kind === "message" && it.role === "assistant" && it.errorMessage) return i;
  }
  return -1;
}
function showHeader(index: number): boolean {
  const g = groupAt(index);
  return !!g && index === g.userIndex + 1 && g.finished && g.hasActivity;
}
function headerDuration(index: number): string {
  return groupAt(index)?.duration ?? "";
}
function turnExpanded(index: number): boolean {
  const g = groupAt(index);
  return !!g && expandedTurns.value.has(g.key);
}
function toggleHeader(index: number): void {
  const g = groupAt(index);
  if (!g) return;
  const next = new Set(expandedTurns.value);
  next.has(g.key) ? next.delete(g.key) : next.add(g.key);
  expandedTurns.value = next;
}
function showItem(index: number): boolean {
  const g = groupAt(index);
  if (!g) return true;
  if (index <= g.userIndex) return true; // 用户气泡
  if (!g.finished) return true; // 进行中的回合
  if (!isTurnCollapsed(g)) return true; // 已展开的卡片
  return index === finalTextIndexOf(g); // 折叠时只显示总结正文
}
function textOnlyFor(index: number): boolean {
  const g = groupAt(index);
  return !!g && isTurnCollapsed(g) && index === finalTextIndexOf(g);
}
// ---- 文件变更卡片（按回合，挂回合尾部；回合结束后拉取，新回合不清掉历史轮卡片） ----
const filesCollapsed = ref(new Set<string>());
function toggleFilesCard(key: string): void {
  const next = new Set(filesCollapsed.value);
  next.has(key) ? next.delete(key) : next.add(key);
  filesCollapsed.value = next;
}
function tfAdd(files: Array<{ added: number }>): number {
  return files.reduce((n, f) => n + f.added, 0);
}
function tfDel(files: Array<{ removed: number }>): number {
  return files.reduce((n, f) => n + f.removed, 0);
}
/** 该回合的变更组：index 为回合最后一个条目时返回（按回合 id 匹配） */
function turnFilesAt(
  index: number,
): { turnId: string; files: Array<{ path: string; added: number; removed: number; isNew: boolean }> } | null {
  const g = groupAt(index);
  if (!g || index !== g.endIndex) return null;
  return props.store.turnFileChanges.find((t) => t.turnId === g.key) ?? null;
}
/** 右侧审查面板：多标签，每项为该文件在该回合的快照 diff（ReviewPanel 用 CodeMirror 渲染） */
const reviewTabs = ref<Array<{ path: string; oldText: string; newText: string; added: number; removed: number }>>([]);
const reviewActive = ref<string | null>(null);
async function toggleReview(path: string, turnId: string): Promise<void> {
  if (reviewActive.value === path) {
    closeReviewTab(path);
    return;
  }
  // 已有标签也重新拉取（新回合后 diff 可能变化），然后激活
  let added = 0;
  let removed = 0;
  for (const t of props.store.turnFileChanges) {
    const f = t.files.find((x) => x.path === path);
    if (f && t.turnId === turnId) {
      added = f.added;
      removed = f.removed;
      break;
    }
  }
  try {
    const r = await props.store.fileDiff(path, turnId);
    const data = { path, oldText: r.oldText, newText: r.newText, added, removed };
    const i = reviewTabs.value.findIndex((t) => t.path === path);
    if (i >= 0) reviewTabs.value[i] = data;
    else reviewTabs.value.push(data);
    reviewActive.value = path;
  } catch (err) {
    // 拉快照失败（命令超时/桌面忙）别静默：面板出不来又无提示，像点了没反应
    props.store.lastError = `审查快照拉取失败：${err instanceof Error ? err.message : String(err)}`;
  }
}
function closeReviewTab(path: string): void {
  const i = reviewTabs.value.findIndex((t) => t.path === path);
  if (i < 0) return;
  reviewTabs.value.splice(i, 1);
  if (reviewActive.value === path) {
    reviewActive.value = reviewTabs.value[Math.min(i, reviewTabs.value.length - 1)]?.path ?? null;
  }
}
function closeReviewAll(): void {
  reviewTabs.value = [];
  reviewActive.value = null;
}

// ---- 审查面板拖拽调宽（NSplit）：size 指对话区（pane1），持久化审查面板宽度 ----
const REVIEW_KEY = "pidock.reviewWidth";
const REVIEW_MIN = 320;
const SPLIT_TRIGGER = 6;
const chatEl = ref<HTMLElement | null>(null);
const chatW = ref(1000);
const reviewWidth = ref<number | null>(readReviewWidth());
function readReviewWidth(): number | null {
  const v = Number(localStorage.getItem(REVIEW_KEY));
  return Number.isFinite(v) && v >= REVIEW_MIN && v <= 1200 ? Math.round(v) : null;
}
/** 窄屏：审查面板改全屏浮层（分栏会把对话区挤到无法阅读） */
const isNarrow = ref(typeof matchMedia !== "undefined" && matchMedia("(max-width: 768px)").matches);
if (typeof matchMedia !== "undefined") {
  const mq = matchMedia("(max-width: 768px)");
  const onNarrowChange = (e: MediaQueryListEvent): void => {
    isNarrow.value = e.matches;
  };
  mq.addEventListener("change", onNarrowChange);
  onBeforeUnmount(() => mq.removeEventListener("change", onNarrowChange));
}
/** pane1（对话区）flex-basis：未开审查时占满；打开时 = 容器 − 触发条 − 面板宽；窄屏恒占满（面板浮层） */
const reviewPane1Size = computed(() =>
  !reviewTabs.value.length || isNarrow.value
    ? "100%"
    : `calc(100% - ${SPLIT_TRIGGER}px - ${(reviewWidth.value ?? Math.round(chatW.value * 0.42))}px)`,
);
const reviewPane1Max = computed(() => (isNarrow.value ? "100%" : `${Math.max(360, chatW.value - SPLIT_TRIGGER - REVIEW_MIN)}px`));
const reviewSplitDisabled = computed(() => isNarrow.value || !reviewTabs.value.length);
const reviewPane2Style = computed(() =>
  isNarrow.value
    ? {
        position: "absolute" as const,
        inset: "0",
        zIndex: 45,
        width: "100%",
        boxShadow: "0 10px 32px rgba(0, 0, 0, 0.4)",
        // 无审查 tab 时空浮层不得拦截对话区的点击
        pointerEvents: reviewTabs.value.length ? ("auto" as const) : ("none" as const),
      }
    : { flex: "1 1 0", minWidth: "0", overflow: "hidden" },
);
function onReviewSplitSize(s: string | number): void {
  const usable = Math.max(0, chatW.value - SPLIT_TRIGGER);
  const px = typeof s === "string" ? parseFloat(s) : s * usable;
  if (!Number.isFinite(px)) return;
  reviewWidth.value = Math.round(Math.min(usable - 360, Math.max(REVIEW_MIN, usable - px)));
}
function saveReview(): void {
  try {
    if (reviewWidth.value != null) localStorage.setItem(REVIEW_KEY, String(reviewWidth.value));
  } catch {
    // localStorage 不可用时忽略
  }
}

// ---- 主区文件预览（临时浮层，覆盖在对话上方）：文件树点击 → 打开/激活标签 ----
const previewTabs = ref<Array<{ path: string; text?: string; truncated?: boolean; binary?: boolean; size?: number; error?: string }>>([]);
const activePreviewPath = ref<string | null>(null);
const previewProjectName = computed(
  () => props.filesCwd?.cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "",
);
// 切换浏览的项目时清空预览标签（标签内容属于该项目）
watch(
  () => props.filesCwd?.cwd,
  () => {
    previewTabs.value = [];
    activePreviewPath.value = null;
  },
);
const PREVIEW_MAX_TABS = 12;
async function openPreviewFile(path: string): Promise<void> {
  const cwd = props.filesCwd?.cwd;
  if (!cwd) return;
  if (!previewTabs.value.some((t) => t.path === path)) {
    try {
      const r = await props.store.readWorkspaceFile(cwd, path);
      previewTabs.value.push({ ...r, path });
      if (previewTabs.value.length > PREVIEW_MAX_TABS) previewTabs.value.shift();
    } catch (err) {
      previewTabs.value.push({ path, error: err instanceof Error ? err.message : String(err) });
    }
  }
  activePreviewPath.value = path;
}
function closePreviewTab(path: string): void {
  const i = previewTabs.value.findIndex((t) => t.path === path);
  if (i < 0) return;
  previewTabs.value.splice(i, 1);
  if (activePreviewPath.value === path) {
    activePreviewPath.value = previewTabs.value[Math.min(i, previewTabs.value.length - 1)]?.path ?? null;
  }
}
function closePreviewAll(): void {
  previewTabs.value = [];
  activePreviewPath.value = null;
}

// ---- 文件树面板宽度（chat-body 的 NSplit #2 = 文件树），记忆 px ----
const FILES_KEY = "pidock.filesWidth";
const FILES_MIN = 240;
const filesWidth = ref<number>(readFilesWidth() ?? 300);
function readFilesWidth(): number | null {
  const v = Number(localStorage.getItem(FILES_KEY));
  return Number.isFinite(v) && v >= FILES_MIN && v <= 1200 ? Math.round(v) : null;
}
/** pane1（主内容）flex-basis：未开文件树时占满；打开时 = 容器 − 触发条 − 面板宽 */
const bodyPane1Size = computed(() =>
  props.filesCwd ? `calc(100% - ${SPLIT_TRIGGER}px - ${filesWidth.value}px)` : "100%",
);
const bodyPane1Max = computed(() => `${Math.max(360, chatW.value - SPLIT_TRIGGER - FILES_MIN)}px`);
function onBodySplitSize(s: string | number): void {
  const usable = Math.max(0, chatW.value - SPLIT_TRIGGER);
  const px = typeof s === "string" ? parseFloat(s) : s * usable;
  if (!Number.isFinite(px)) return;
  filesWidth.value = Math.round(Math.min(usable - 360, Math.max(FILES_MIN, usable - px)));
}
function saveFilesWidth(): void {
  try {
    localStorage.setItem(FILES_KEY, String(filesWidth.value));
  } catch {
    // localStorage 不可用时忽略
  }
}

function baseOf(p: string): string {
  const norm = p.replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1);
}
function dirOfPath(p: string): string {
  const segs = p.replace(/\\/g, "/").split("/").filter(Boolean);
  segs.pop();
  return segs.length ? segs.slice(-2).join("/") + "/" : "";
}
async function revertAll(turnId: string, count: number): Promise<void> {
  if (!count) return;
  if (!(await appConfirm({ title: `撤销该回合全部 ${count} 个文件的更改？`, message: "文件将恢复为该回合开始前的内容。", danger: true }))) return;
  await props.store.revertFiles(turnId);
  closeReviewAll();
}

const nowTick = ref(Date.now());
let dividerTimer: ReturnType<typeof setInterval> | undefined;
watch(
  busy,
  (b) => {
    if (b) dividerTimer = setInterval(() => (nowTick.value = Date.now()), 1000);
    else if (dividerTimer) {
      clearInterval(dividerTimer);
      dividerTimer = undefined;
    }
  },
  { immediate: true },
);
watch(busy, (b) => {
  // 回合结束后拉取；回合开始不清空 —— 历史回合的变更卡片保留在各自回合尾部
  if (!b) void props.store.fetchFileChanges();
});
watch(
  () => props.store.activeId,
  () => {
    props.store.turnFileChanges = [];
    closeReviewAll();
  },
);
let chatRO: ResizeObserver | undefined;
onBeforeUnmount(() => {
  if (dividerTimer) clearInterval(dividerTimer);
  chatRO?.disconnect();
  chatRO = undefined;
});
// 每次进入对话页刷新一次全量模型（从供应商页改完配置返回后立即可见）
onMounted(() => {
  void props.store.refreshModels();
  // 跟踪对话区宽度：NSplit 的 min/max 与面板宽度换算依赖它
  if (chatEl.value && typeof ResizeObserver !== "undefined") {
    chatRO = new ResizeObserver((entries) => {
      chatW.value = entries[0]?.contentRect.width ?? 1000;
    });
    chatRO.observe(chatEl.value);
  }
});

const chips: { name: string; text: string }[] = [
  {
    name: "folder-line",
    text: "帮我看看这个项目的结构",
  },
  {
    name: "bug-line",
    text: "修复当前项目里的报错",
  },
  {
    name: "file-add-line",
    text: "给项目写一份 README",
  },
  {
    name: "code-line",
    text: "代码评审最近的改动",
  },
];


function fillChip(text: string): void {
  preset.value = "";
  requestAnimationFrame(() => (preset.value = text));
}

/**
 * 会话分叉：从用户消息条目分叉出新会话（host 保留此前历史），原文回填输入框。
 * 原会话保持不变；回填后清掉 preset，避免首页输入卡被旧文本预填。
 */
async function forkFromMessage(item: UiItem): Promise<void> {
  if (item.kind !== "message" || item.role !== "user" || !item.entryId) return;
  if (busy.value) return;
  const ok = await appConfirm({
    title: "从此条消息分叉新会话？",
    message: "将创建一个包含此前历史的新会话，原会话保持不变；这条消息的内容会填入输入框，可修改后重新发送。",
  });
  if (!ok) return;
  try {
    const r = await props.store.forkSession(item.entryId);
    if (r.selected_text) {
      fillChip(r.selected_text);
      setTimeout(() => {
        if (preset.value === r.selected_text) preset.value = "";
      }, 500);
    }
  } catch (err) {
    props.store.lastError = err instanceof Error ? err.message : String(err);
  }
}

async function sendFromHome(
  text: string,
  cwd?: string | null,
  images?: Array<{ data: string; mime_type: string }>,
  files?: Array<{ name: string; mime_type: string; size: number; data: string }>,
): Promise<void> {
  // cwd 为 null = 「不在项目中工作」（host 落到主目录）；
  // 未指定时用最近会话的目录兜底；首页选中的模型随新会话生效
  const last = props.store.sessions[0];
  const dir = cwd === null ? undefined : cwd || last?.cwd || ".";
  await props.store.newSession(dir, homeModel.value ?? undefined, hiredExpert.value?.id);
  hiredExpert.value = null;
  // 首页暂存的权限模式 / 思考级别随新会话下发；失败时不发送，避免以错误权限执行任务
  try {
    if (homePermissionMode.value) await props.store.setPermissionMode(homePermissionMode.value);
    if (homeThinkingLevel.value) await props.store.setThinkingLevel(homeThinkingLevel.value);
  } catch (err) {
    props.store.lastError = err instanceof Error ? err.message : String(err);
    return;
  } finally {
    homePermissionMode.value = null;
    homeThinkingLevel.value = null;
  }
  await props.store.send(text, images, files);
}

function dismissError(): void {
  props.store.lastError = null;
}

const scrollTarget = () => {
  const el = scroller.value;
  if (el) el.scrollTop = el.scrollHeight;
};

watch(
  () => props.store.items.length,
  async () => {
    await nextTick();
    scrollTarget();
  },
);

let lastLen = 0;
watch(
  () => props.store.items.reduce((n, it) => n + ("text" in it ? it.text.length : 0), 0),
  async (len) => {
    if (len - lastLen > 0 && len - lastLen < 500) {
      await nextTick();
      scrollTarget();
    }
    lastLen = len;
  },
);
</script>

<template>
  <div ref="chatEl" class="chat" :class="{ home }">
    <!-- 文件树宽度可拖（NSplit）：#1 主内容区，#2 文件树面板 -->
    <n-split
      direction="horizontal"
      class="body-split"
      :size="bodyPane1Size"
      min="360px"
      :max="bodyPane1Max"
      :resize-trigger-size="6"
      :disabled="!filesCwd"
      :pane1-style="{ display: 'flex', minWidth: '0', overflow: 'hidden' }"
      :pane2-style="{ flex: '1 1 0', minWidth: '0', overflow: 'hidden' }"
      @update:size="onBodySplitSize"
      @drag-end="saveFilesWidth"
    >
    <template #1>
    <div class="main-area">
    <!-- home: watermark + greeting + composer -->
    <div v-if="home" class="home-row">
    <div class="home-wrap">
      <div class="watermark">π</div>
      <div class="home-inner">
        <h1 class="greeting">{{ greeting() }}，接下来交给我吧</h1>
        <div v-if="store.lastError" class="errbar">
          <Icon name="error-warning-line" :size="15" />
          <span class="err-text" :title="store.lastError">{{ store.lastError }}</span>
          <button class="err-close" title="忽略" @click="dismissError">
            <Icon name="close-line" :size="13" />
          </button>
        </div>
        <Composer
          :busy="false"
          :disabled="disabled"
          :disabled-hint="disabledHint"
          :model="homeComposerModel"
          :models="modelOptions"
          :permission-mode="composerPermissionMode"
          :thinking-level="composerThinkingLevel"
          :mention-cwd="store.homeDir ?? undefined"
          :mention-loader="mentionLoader"
          :skills-loader="skillsLoader"
          :experts-loader="expertsLoader"
          :prompts-loader="promptsLoader"
          :hired-expert="hiredExpert"
          :model-images-ok="modelSupportsImages"
          centered
          :projects="projects"
          :preset-cwd="newTaskCwd"
          placeholder="描述你的任务，Enter 发送"
          :preset="preset"
          @send="sendFromHome"
          @hire="(e) => (hiredExpert = e)"
          @unhire="hiredExpert = null"
          @set-permission-mode="setPermissionMode"
          @set-thinking-level="setThinkingLevel"
          @set-model="setModel"
          @open-providers="emit('open-providers')"
        />
        <div class="chips">
          <button v-for="c in chips" :key="c.text" class="chip" @click="fillChip(c.text)">
            <Icon :name="c.name" :size="14" />{{ c.text }}
          </button>
        </div>
      </div>
    </div>
    </div>

    <!-- conversation -->
    <div v-else class="conv-row">
      <n-split
        direction="horizontal"
        class="chat-split"
        :size="reviewPane1Size"
        :min="isNarrow ? '0px' : '360px'"
        :max="reviewPane1Max"
        :resize-trigger-size="6"
        :disabled="reviewSplitDisabled"
        :pane1-style="{ display: 'flex' }"
        :pane2-style="reviewPane2Style"
        @update:size="onReviewSplitSize"
        @drag-end="saveReview"
      >
        <template #1>
      <div class="chat-main">
      <div ref="scroller" class="scroll">
        <div v-if="store.loadingHistory" class="hint">加载历史中…</div>
        <div v-if="compactionSummary" class="compaction-strip">
          <button class="cs-toggle" @click="csOpen = !csOpen">
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: !csOpen }" />
            已压缩上下文<template v-if="compactionSummary.tokens_before"> · 压缩前 {{ compactionSummary.tokens_before }} tokens</template>
          </button>
          <pre v-if="csOpen" class="cs-body">{{ compactionSummary.summary }}</pre>
        </div>
        <template v-for="(item, index) in store.items" :key="item.key">
          <button v-if="showHeader(index)" class="turn-header" @click="toggleHeader(index)">
            <span>已工作<template v-if="headerDuration(index)"> · {{ headerDuration(index) }}</template></span>
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: !turnExpanded(index) }" />
          </button>
          <template v-if="showItem(index)">
            <ToolCard
              v-if="item.kind === 'tool'"
              :tool-name="item.toolName"
              :status="item.status"
              :args="item.args"
              :partial="item.partial"
              :output="item.output"
              :dimmed="busy"
            />
            <MessageItem
              v-else-if="!isMergedResult(item) && !isBlankMessage(item)"
              :item="item"
              :dimmed="busy"
              :results="resultsMap"
              :args-map="argsMap"
              :text-only="textOnlyFor(index)"
              @fork="forkFromMessage(item)"
            />
          </template>
          <div v-if="index === lastUserIndex && busy" class="turn-divider">
            <span class="turn-label">工作中 · {{ formatSpan(Math.max(1000, nowTick - (store.turnStartedAt ?? nowTick))) }}</span>
            <span class="turn-line"></span>
          </div>
          <!-- 该回合的文件变更卡片（挂在回合尾部；撤销/审查针对该回合） -->
          <div v-if="turnFilesAt(index)" class="files-card">
            <button class="files-head" @click="toggleFilesCard(groupAt(index)!.key)">
              <Icon name="arrow-down-s-line" :size="13" :class="{ fold: filesCollapsed.has(groupAt(index)!.key) }" />
              <span>{{ turnFilesAt(index)!.files.length }} 个文件已更改</span>
              <span v-if="tfAdd(turnFilesAt(index)!.files)" class="t-add">+{{ tfAdd(turnFilesAt(index)!.files) }}</span>
              <span v-if="tfDel(turnFilesAt(index)!.files)" class="t-del">−{{ tfDel(turnFilesAt(index)!.files) }}</span>
              <span class="flex-sp"></span>
              <span class="revert" @click.stop="revertAll(groupAt(index)!.key, turnFilesAt(index)!.files.length)">
                <Icon name="history-line" :size="14" />撤销
              </span>
            </button>
            <div v-if="!filesCollapsed.has(groupAt(index)!.key)" class="files-list">
              <template v-for="f in turnFilesAt(index)!.files" :key="f.path">
                <div class="file-row">
                  <span class="tile"><FileIcon :path="f.path" :size="15" /></span>
                  <span class="f-name" :title="f.path">{{ baseOf(f.path) }}</span>
                  <span class="f-dir">{{ dirOfPath(f.path) }}</span>
                  <span class="t-add">+{{ f.added }}</span>
                  <span class="t-del">−{{ f.removed }}</span>
                  <button
                    class="review-btn"
                    :class="{ on: reviewTabs.some((t) => t.path === f.path) }"
                    @click="toggleReview(f.path, groupAt(index)!.key)"
                  >
                    {{ reviewActive === f.path ? "收起" : "审查" }}
                  </button>
                </div>
              </template>
            </div>
          </div>
        </template>
        <!-- 工作中尾部的旋转 loading（消息/工具流到来前后的活动指示） -->
        <div v-if="busy" class="working-load">
          <Icon name="loader-2-line" :size="18" />
        </div>
        <div v-if="busy && lastUserIndex === -1" class="turn-divider">
          <span class="turn-label">工作中 · {{ formatSpan(Math.max(1000, nowTick - (store.turnStartedAt ?? nowTick))) }}</span>
          <span class="turn-line"></span>
        </div>
      </div>
      <div v-if="store.pendingApproval" class="approval">
        <Icon name="shield-flash-line" :size="15" />
        <div class="approval-info">
          <b>请求执行：{{ store.pendingApproval.toolName }}</b>
          <span class="approval-args" :title="store.pendingApproval.args">{{ store.pendingApproval.args }}</span>
        </div>
        <button class="approval-btn ok" @click="store.resolveApproval(true)">批准</button>
        <button class="approval-btn no" @click="store.resolveApproval(false)">拒绝</button>
      </div>
      <AskCard
        v-if="store.pendingAsk"
        :ask="store.pendingAsk"
        @resolve="(a) => store.resolveAsk(a)"
      />
      <div class="dock">
        <!-- 上下文用量仪表条：压缩入口 / 排队消息清空 / 会话设置 -->
        <div class="ctx-bar">
          <button
            class="ctx-meter"
            :class="contextMeterClass"
            :disabled="store.agentState !== 'idle'"
            title="压缩上下文"
            @click="compactConfirm"
          >
            <span class="meter"><span class="fill" :style="{ width: `${contextPercent ?? 0}%` }"></span></span>
            <span class="ctx-text">
              <template v-if="contextPercent !== null">上下文 {{ Math.round(contextPercent) }}%{{ contextTokensText ? ` · ${contextTokensText}${contextWindowText ? `/${contextWindowText}` : ""}` : "" }}</template>
              <template v-else>上下文 待响应</template>
            </span>
          </button>
          <button v-if="queuedCount" class="queue-chip" title="清空排队中的消息（原文回填输入框）" @click="clearQueueToComposer">
            <Icon name="time-line" :size="13" />排队 {{ queuedCount }} · 清空
          </button>
          <span v-if="compacting" class="retry-chip" title="正在压缩上下文">
            <Icon name="loader-2-line" :size="13" class="spin" />压缩中…
          </span>
          <span v-if="retryInfo" class="retry-chip" :title="retryInfo.error || '请求失败后自动重试'">
            <Icon name="loader-2-line" :size="13" class="spin" />自动重试 {{ retryInfo.attempt ?? "?" }}<template v-if="retryInfo.max_attempts">/{{ retryInfo.max_attempts }}</template>
          </span>
          <span class="flex-sp"></span>
          <button class="ctx-gear" title="会话设置" @click="showSessionSettings = true">
            <Icon name="settings-3-line" :size="15" />
          </button>
        </div>
        <Composer
          :busy="store.agentState !== 'idle'"
          :model="model"
          :permission-mode="composerPermissionMode"
          :thinking-level="composerThinkingLevel"
          :thinking-available="thinkingAvailable"
          :mention-cwd="activeSession?.cwd"
          :mention-loader="mentionLoader"
          :skills-loader="skillsLoader"
          :experts-loader="expertsLoader"
          :prompts-loader="promptsLoader"
          :model-images-ok="modelSupportsImages"
          :models="modelOptions"
          :preset="preset"
          @send="(t: string, _cwd: unknown, imgs?: Array<{ data: string; mime_type: string }>, files?: Array<{ name: string; mime_type: string; size: number; data: string }>) => store.send(t, imgs, files)"
          @abort="store.abort()"
          @set-permission-mode="setPermissionMode"
          @set-thinking-level="setThinkingLevel"
          @set-model="setModel"
          @open-providers="emit('open-providers')"
        />
      </div>
      </div>
      </template>
        <template #2>
          <ReviewPanel
            v-if="reviewTabs.length"
            :tabs="reviewTabs"
            :active="reviewActive ?? reviewTabs[0]!.path"
            @select="(p: string) => (reviewActive = p)"
            @close-tab="closeReviewTab"
            @close="closeReviewAll"
          />
        </template>
        <template #resize-trigger><div class="rz-line" /></template>
      </n-split>
      <!-- 任务进度第三栏（TodoWrite 推送或雇佣专家时停靠展开；收起为悬浮小圆标） -->
      <ProgressCard :store="store" :expert="activeExpert" />
      </div>
    <!-- 会话设置弹窗：开关/工具子集/导出/分支树导航 -->
    <SessionSettings :store="store" :open="showSessionSettings" @close="showSessionSettings = false" />
    <!-- 临时文件预览浮层：覆盖在主对话区上方（home 与会话模式共用），关闭后恢复对话 -->
    <FilePreview
      v-if="previewTabs.length"
      class="preview-overlay"
      :tabs="previewTabs"
      :active="activePreviewPath"
      :project-name="previewProjectName"
      @activate="(p: string) => (activePreviewPath = p)"
      @close="closePreviewTab"
      @close-all="closePreviewAll"
    />
    </div>
    </template>
    <template #2>
      <!-- 项目文件浏览面板：宽度可拖；点击文件在主区打开预览浮层 -->
      <FilesPanel
        v-if="filesCwd"
        :key="filesCwd.seq"
        :cwd="filesCwd.cwd"
        :open-paths="previewTabs.map((t) => t.path)"
        :active-path="activePreviewPath"
        :load-files="store.listWorkspaceFiles"
        @open-file="openPreviewFile"
        @close="emit('close-files')"
      />
    </template>
    <template #resize-trigger><div class="rz-line" /></template>
    </n-split>
  </div>
</template>

<script lang="ts">
import ToolCard from "./ToolCard.vue";
import MessageItem from "./MessageItem.vue";
export default { components: { ToolCard, MessageItem } };
</script>

<style scoped>
.chat {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}
.chat-split {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  /* 窄屏审查浮层（pane2 absolute）的定位上下文 */
  position: relative;
}
/* NSplit 触发条内容：6px 命中区 + 悬停主题色线 */
.rz-line {
  width: 100%;
  height: 100%;
  position: relative;
  cursor: col-resize;
}
.rz-line::after {
  content: "";
  position: absolute;
  inset: 0 2px;
  background: transparent;
  transition: background-color 0.15s;
}
.rz-line:hover::after {
  background: var(--pd-accent);
  opacity: 0.55;
}
/* 对话行：聊天/审查 n-split 与任务进度第三栏的水平容器（收起态小圆标相对它定位） */
.conv-row {
  flex: 1;
  display: flex;
  min-width: 0;
  min-height: 0;
  position: relative;
}
.chat-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 360px;
  min-height: 0;
}
.chat.home { background: var(--pd-bg); }
/* 文件树宽度可拖的分栏容器（NSplit）：#1 主内容，#2 文件树面板 */
.body-split {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
/* 主内容区（split #1）：文件预览浮层的定位参考 */
.main-area {
  position: relative;
  flex: 1;
  display: flex;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
/* 首页行：居中任务区铺满主内容区（预览浮层覆盖其上） */
.home-row {
  flex: 1;
  display: flex;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
/* 临时文件预览浮层：覆盖在主对话区上方（home 与会话模式共用） */
.preview-overlay {
  position: absolute;
  inset: 0;
  z-index: 30;
  border-left: 1px solid var(--pd-border);
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.18);
}
.home-wrap {
  flex: 1;
  position: relative;
  display: grid;
  /* minmax(0,1fr)：轨道不被 composer 工具行的 min-content 撑破窄容器 */
  grid-template-columns: minmax(0, 1fr);
  place-items: center;
  padding: 24px 40px 60px;
  overflow: hidden;
}
.watermark {
  position: absolute;
  top: 2%;
  left: 50%;
  transform: translateX(-50%) skewX(-10deg);
  font-size: calc(220px * var(--pd-font-scale));
  font-weight: 800;
  font-style: italic;
  line-height: 1;
  letter-spacing: -12px;
  color: transparent;
  -webkit-text-stroke: 1.5px var(--pd-watermark-line);
  user-select: none;
  pointer-events: none;
}
.home-inner {
  width: min(760px, 100%);
  display: flex;
  flex-direction: column;
  position: relative;
}
.greeting {
  text-align: center;
  font-size: calc(26px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
  margin: 0 0 34px;
  letter-spacing: 0.02em;
}
/* 手机：收窄留白与问候字号，避免换行后观感松散 */
@media (max-width: 480px) {
  .home-wrap { padding: 20px 16px 48px; }
  .greeting { font-size: calc(21px * var(--pd-font-scale)); margin-bottom: 26px; }
}
.errbar {
  display: flex;
  align-items: center;
  gap: 10px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 9px 14px;
  margin-bottom: 16px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
}
.errbar svg { color: var(--pd-text-3); flex: none; }
.err-text {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.err-close {
  background: none;
  border: none;
  color: var(--pd-text-3);
  cursor: pointer;
  border-radius: 6px;
  padding: 4px;
  display: grid;
  place-items: center;
  flex: none;
}
.err-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.chips {
  display: flex;
  gap: 12px;
  margin-top: 30px;
  flex-wrap: wrap;
  justify-content: center;
}
.chip {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  border-radius: 10px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg-card);
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s;
}
.chip:hover { border-color: #4a4a4a; color: var(--pd-text); }
.scroll {
  flex: 1;
  overflow-y: auto;
  padding: 18px 22px;
}
.hint {
  color: var(--pd-text-4);
  text-align: center;
  margin-top: 40vh;
  font-size: calc(13px * var(--pd-font-scale));
}
.dock { padding: 10px 16px 14px; }

/* ---- 上下文用量仪表条 ---- */
.ctx-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
  min-height: 22px;
}
.ctx-meter {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 2px 8px;
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  background: none;
  color: var(--pd-text-3);
  font-size: calc(11px * var(--pd-font-scale, 1));
  cursor: pointer;
}
.ctx-meter:hover:not(:disabled) { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.ctx-meter:disabled { cursor: default; opacity: 0.7; }
.ctx-meter .meter {
  width: 56px;
  height: 4px;
  border-radius: 2px;
  background: var(--pd-bg-hover);
  overflow: hidden;
  flex: none;
}
.ctx-meter .fill {
  display: block;
  height: 100%;
  border-radius: 2px;
  background: var(--pd-accent);
  transition: width 0.4s ease;
}
.ctx-meter.warm .fill { background: var(--pd-amber, #e6a23c); }
.ctx-meter.warm { color: var(--pd-amber, #e6a23c); }
.ctx-meter.hot .fill { background: var(--pd-red, #e5484d); }
.ctx-meter.hot { color: var(--pd-red, #e5484d); }
.ctx-text { white-space: nowrap; }
.queue-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px;
  border-radius: 999px;
  border: 1px solid var(--pd-amber, #e6a23c);
  background: none;
  color: var(--pd-amber, #e6a23c);
  font-size: calc(11px * var(--pd-font-scale, 1));
  cursor: pointer;
  white-space: nowrap;
}
.queue-chip:hover { filter: brightness(1.1); }
.retry-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px;
  border-radius: 999px;
  border: 1px solid var(--pd-amber, #e6a23c);
  color: var(--pd-amber, #e6a23c);
  font-size: calc(11px * var(--pd-font-scale, 1));
  white-space: nowrap;
}
.retry-chip .spin { animation: pd-spin 1s linear infinite; }
@keyframes pd-spin {
  to { transform: rotate(360deg); }
}
.compaction-strip {
  margin: 0 0 10px;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  background: var(--pd-bg-panel);
  overflow: hidden;
}
.cs-toggle {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  color: var(--pd-text-3);
  font-size: calc(12px * var(--pd-font-scale, 1));
  cursor: pointer;
  text-align: left;
}
.cs-toggle:hover { color: var(--pd-text-2); }
.cs-toggle .fold { transform: rotate(-90deg); }
.cs-body {
  margin: 0;
  padding: 8px 12px 10px 28px;
  border-top: 1px dashed var(--pd-border);
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale, 1));
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 240px;
  overflow-y: auto;
}
.ctx-gear {
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-3);
  cursor: pointer;
  flex: none;
}
.ctx-gear:hover { background: var(--pd-bg-hover); color: var(--pd-text); }

/* ---- 回合分隔线（工作中 · 耗时） ---- */
.turn-divider {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 14px 0 6px;
  user-select: none;
}
.working-load {
  padding: 8px 0 10px;
  color: var(--pd-text-4);
  user-select: none;
}
.working-load svg {
  display: block;
  animation: work-spin 0.9s linear infinite;
}
@keyframes work-spin {
  to { transform: rotate(360deg); }
}
.turn-label {
  flex: none;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
}
.turn-line {
  flex: 1;
  height: 1px;
  background: var(--pd-border-soft);
}

/* ---- 回合结束折叠行（已工作 · 耗时） ---- */
.turn-header {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 8px 0 4px;
  background: none;
  border: none;
  color: var(--pd-text-3);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  padding: 4px 0;
  user-select: none;
}
.turn-header:hover { color: var(--pd-text-2); }
.turn-header svg {
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.turn-header svg.fold { transform: rotate(-90deg); }

/* ---- 文件变更卡片 ---- */
.files-card {
  margin-top: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 4px 14px 10px;
}
.files-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  background: none;
  border: none;
  padding: 10px 2px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.files-head:hover { color: var(--pd-text); }
.files-head svg { color: var(--pd-text-4); transition: transform 0.12s; }
.files-head svg.fold { transform: rotate(-90deg); }
.files-head .revert {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--pd-text-3);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 4px 8px;
  border-radius: 7px;
}
.files-head .revert:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.files-list { border-top: 1px solid var(--pd-border-soft); }
.file-row {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 2px;
  border-bottom: 1px solid var(--pd-border-soft);
  font-size: calc(13px * var(--pd-font-scale));
}
.file-row:last-child { border-bottom: none; }
.tile {
  width: 20px;
  height: 20px;
  flex: none;
  display: grid;
  place-items: center;
}
.f-name {
  color: var(--pd-text);
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.f-dir {
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.review-btn {
  flex: none;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 7px;
  padding: 4px 10px;
  color: var(--pd-text-3);
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
}
.review-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.review-btn.on {
  color: var(--pd-text);
  background: var(--pd-bg-hover);
  border-color: var(--pd-accent);
}

/* ---- 工具审批横幅 ---- */
.approval {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0 16px 8px;
  padding: 10px 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-accent);
  border-radius: 12px;
  color: var(--pd-text-2);
}
.approval svg { color: var(--pd-accent); flex: none; }
.approval-info { flex: 1; min-width: 0; }
.approval-info b { display: block; font-size: calc(13px * var(--pd-font-scale)); color: var(--pd-text); }
.approval-args {
  display: block;
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  font-family: var(--pd-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-top: 2px;
}
.approval-btn {
  flex: none;
  border: none;
  border-radius: 8px;
  padding: 7px 14px;
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
}
.approval-btn.ok { background: var(--pd-accent); color: #1a1a1a; }
.approval-btn.ok:hover { background: var(--pd-accent-hover); }
.approval-btn.no { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.approval-btn.no:hover { background: var(--pd-bg-active); color: var(--pd-text); }
</style>
