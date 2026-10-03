<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { NSplit } from "naive-ui";
import type { AgentStore, UiItem } from "../store.js";
import { formatSpan, greeting } from "../utils/time.js";
import FileIcon from "./FileIcon.vue";
import MdContent from "./MdContent.vue";
import ReviewPanel from "./ReviewPanel.vue";
import Composer from "./Composer.vue";
import Icon from "./Icon.vue";

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
  }>(),
  { disabled: false },
);

const emit = defineEmits<{ "open-settings": [tab?: string]; "open-providers": [] }>();

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
function mentionLoader(cwd: string): Promise<Array<{ path: string; name: string; dir: boolean }>> {
  return props.store.listWorkspaceFiles(cwd);
}

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
// ---- 文件变更卡片（回合结束后） ----
const fileChangesOpen = ref(true);
const totalAdd = computed(() => props.store.fileChanges.reduce((n, f) => n + f.added, 0));
const totalDel = computed(() => props.store.fileChanges.reduce((n, f) => n + f.removed, 0));
/** 右侧审查面板：多标签，每项为该文件的快照 diff（ReviewPanel 用 CodeMirror 渲染） */
const reviewTabs = ref<Array<{ path: string; oldText: string; newText: string; added: number; removed: number }>>([]);
const reviewActive = ref<string | null>(null);
async function toggleReview(path: string): Promise<void> {
  if (reviewActive.value === path) {
    closeReviewTab(path);
    return;
  }
  // 已有标签也重新拉取（新回合后 diff 可能变化），然后激活
  const f = props.store.fileChanges.find((x) => x.path === path);
  const r = await props.store.fileDiff(path);
  const data = { path, oldText: r.oldText, newText: r.newText, added: f?.added ?? 0, removed: f?.removed ?? 0 };
  const i = reviewTabs.value.findIndex((t) => t.path === path);
  if (i >= 0) reviewTabs.value[i] = data;
  else reviewTabs.value.push(data);
  reviewActive.value = path;
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
/** pane1（对话区）flex-basis：未开审查时占满；打开时 = 容器 − 触发条 − 面板宽 */
const reviewPane1Size = computed(() =>
  reviewTabs.value.length
    ? `calc(100% - ${SPLIT_TRIGGER}px - ${(reviewWidth.value ?? Math.round(chatW.value * 0.42))}px)`
    : "100%",
);
const reviewPane1Max = computed(() => `${Math.max(360, chatW.value - SPLIT_TRIGGER - REVIEW_MIN)}px`);
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

function baseOf(p: string): string {
  const norm = p.replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1);
}
function dirOfPath(p: string): string {
  const segs = p.replace(/\\/g, "/").split("/").filter(Boolean);
  segs.pop();
  return segs.length ? segs.slice(-2).join("/") + "/" : "";
}
async function revertAll(): Promise<void> {
  if (!props.store.fileChanges.length) return;
  if (!window.confirm(`撤销本轮全部 ${props.store.fileChanges.length} 个文件的更改？`)) return;
  await props.store.revertFiles();
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
  if (b) props.store.fileChanges = [];
  else void props.store.fetchFileChanges();
});
watch(
  () => props.store.activeId,
  () => {
    props.store.fileChanges = [];
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

async function sendFromHome(text: string, cwd?: string | null): Promise<void> {
  // cwd 为 null = 「不在项目中工作」（host 落到主目录）；
  // 未指定时用最近会话的目录兜底；首页选中的模型随新会话生效
  const last = props.store.sessions[0];
  const dir = cwd === null ? undefined : cwd || last?.cwd || ".";
  await props.store.newSession(dir, homeModel.value ?? undefined);
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
  await props.store.send(text);
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
    <!-- home: watermark + greeting + composer -->
    <div v-if="home" class="home-wrap">
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
          centered
          :projects="projects"
          :preset-cwd="newTaskCwd"
          placeholder="描述你的任务，Enter 发送"
          :preset="preset"
          @send="sendFromHome"
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

    <!-- conversation -->
    <template v-else>
      <n-split
        direction="horizontal"
        class="chat-split"
        :size="reviewPane1Size"
        min="360px"
        :max="reviewPane1Max"
        :resize-trigger-size="6"
        :disabled="!reviewTabs.length"
        :pane1-style="{ display: 'flex' }"
        :pane2-style="{ flex: '1 1 0', minWidth: '0', overflow: 'hidden' }"
        @update:size="onReviewSplitSize"
        @drag-end="saveReview"
      >
        <template #1>
      <div class="chat-main">
      <div ref="scroller" class="scroll">
        <div v-if="store.loadingHistory" class="hint">加载历史中…</div>
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
              v-else-if="!isMergedResult(item)"
              :item="item"
              :dimmed="busy"
              :results="resultsMap"
              :args-map="argsMap"
              :text-only="textOnlyFor(index)"
            />
          </template>
          <div v-if="index === lastUserIndex && busy" class="turn-divider">
            <span class="turn-label">工作中 · {{ formatSpan(Math.max(1000, nowTick - (store.turnStartedAt ?? nowTick))) }}</span>
            <span class="turn-line"></span>
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

        <!-- 文件变更卡片 -->
        <div v-if="store.fileChanges.length" class="files-card">
          <button class="files-head" @click="fileChangesOpen = !fileChangesOpen">
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: !fileChangesOpen }" />
            <span>{{ store.fileChanges.length }} 个文件已更改</span>
            <span v-if="totalAdd" class="t-add">+{{ totalAdd }}</span>
            <span v-if="totalDel" class="t-del">−{{ totalDel }}</span>
            <span class="flex-sp"></span>
            <span class="revert" @click.stop="revertAll">
              <Icon name="history-line" :size="13" />撤销
            </span>
          </button>
          <div v-if="fileChangesOpen" class="files-list">
            <template v-for="f in store.fileChanges" :key="f.path">
              <div class="file-row">
                <span class="tile"><FileIcon :path="f.path" :size="15" /></span>
                <span class="f-name" :title="f.path">{{ baseOf(f.path) }}</span>
                <span class="f-dir">{{ dirOfPath(f.path) }}</span>
                <span class="t-add">+{{ f.added }}</span>
                <span class="t-del">−{{ f.removed }}</span>
                <button
                  class="review-btn"
                  :class="{ on: reviewTabs.some((t) => t.path === f.path) }"
                  @click="toggleReview(f.path)"
                >
                  {{ reviewActive === f.path ? "收起" : "审查" }}
                </button>
              </div>
            </template>
          </div>
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
      <div class="dock">
        <Composer
          :busy="store.agentState !== 'idle'"
          :model="model"
          :permission-mode="composerPermissionMode"
          :thinking-level="composerThinkingLevel"
          :mention-cwd="activeSession?.cwd"
          :mention-loader="mentionLoader"
          :models="modelOptions"
          @send="(t: string) => store.send(t)"
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
    </template>
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
.chat-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 360px;
  min-height: 0;
}
.chat.home { background: var(--pd-bg); }
.home-wrap {
  flex: 1;
  position: relative;
  display: grid;
  place-items: center;
  padding: 24px 40px 60px;
  overflow: hidden;
}
.watermark {
  position: absolute;
  top: 2%;
  left: 50%;
  transform: translateX(-50%) skewX(-10deg);
  font-size: 220px;
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
  font-size: 26px;
  font-weight: 600;
  color: var(--pd-text);
  margin: 0 0 34px;
  letter-spacing: 0.02em;
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
  font-size: 13px;
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
  font-size: 13px;
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
  font-size: 13px;
}
.dock { padding: 10px 16px 14px; }

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
  font-size: 12px;
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
  font-size: 13px;
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
  font-size: 13px;
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
  font-size: 12.5px;
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
  font-size: 13px;
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
  font-size: 12px;
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
  font-size: 12px;
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
.approval-info b { display: block; font-size: 13px; color: var(--pd-text); }
.approval-args {
  display: block;
  font-size: 11.5px;
  color: var(--pd-text-3);
  font-family: Consolas, monospace;
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
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}
.approval-btn.ok { background: var(--pd-accent); color: #1a1a1a; }
.approval-btn.ok:hover { background: var(--pd-accent-hover); }
.approval-btn.no { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.approval-btn.no:hover { background: var(--pd-bg-active); color: var(--pd-text); }
</style>
