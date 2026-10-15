<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { TodoItem } from "@pidock/protocol";
import type { AgentStore, ExpertInfo, SubagentRunUi } from "../store.js";
import Icon from "./Icon.vue";
import SubagentDetail from "./SubagentDetail.vue";

/**
 * 任务进度第三栏（对话区右侧停靠，压缩对话流宽度而非遮挡）：
 * 专家段（绑定的专家头像/描述/角色提示词摘要/资源标签）+
 * 项目段（名称/路径/模型/权限）+ 进度段（TodoWrite 任务清单）+
 * 子代理段（task 工具派生的运行条目；点行进入工作流详情视图）。
 * TodoWrite 首次推送或会话绑定专家或子代理派生时自动展开；
 * 收起后缩成悬浮小圆标，点击恢复；收起状态按会话记忆（localStorage）。
 */

const props = defineProps<{ store: AgentStore; expert?: ExpertInfo | null }>();

const todos = computed<TodoItem[]>(() =>
  props.store.activeId ? (props.store.todosBySession[props.store.activeId] ?? []) : [],
);
const summary = computed(() => props.store.sessions.find((s) => s.session_id === props.store.activeId));
const done = computed(() => todos.value.filter((t) => t.status === "completed").length);

// ---- 子代理运行段 ----
const runs = computed<SubagentRunUi[]>(() => {
  const pid = props.store.activeId;
  if (!pid) return [];
  return Object.values(props.store.subagentRuns).filter((r) => r.parent_session_id === pid);
});
const runningCount = computed(() => runs.value.filter((r) => !["idle", "waiting_approval", "waiting_ask"].includes(r.state)).length);
const waitingCount = computed(() => runs.value.filter((r) => ["waiting_approval", "waiting_ask"].includes(r.state)).length);
/** 详情导航：选中的子代理（会话结束后条目消失时自动回列表） */
const selectedChild = ref<string | null>(null);
const detailRun = computed(() => (selectedChild.value ? runs.value.find((r) => r.session_id === selectedChild.value) ?? null : null));
watch(runs, (list) => {
  if (selectedChild.value && !list.some((r) => r.session_id === selectedChild.value)) selectedChild.value = null;
});
// 首个子代理派生时自动停靠展开（用户手动收起后不反复打扰）
watch(
  () => runs.value.map((r) => r.session_id).join(","),
  (nowIds, prevIds) => {
    if (nowIds && !prevIds) expand();
  },
);

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/** 行内终止子代理（与详情视图的中止同一 RPC；挂起审批由 host 按拒绝结算） */
async function abortRun(r: SubagentRunUi): Promise<void> {
  try {
    await props.store.rpc("agent.abort", { session_id: r.session_id });
  } catch {
    // 已结束/已关闭的子代理中止报错可忽略
  }
}

// ---- dock 拖拽调宽（按对话区宽度的百分比记忆，窗口缩放保持同比例；窄屏浮层固定宽不拖拽） ----
// 实际渲染 width: clamp(260px, N%, min(1280px, 100% - 420px))——下限/聊天区保底/绝对上限
// 全部交给 CSS clamp 动态裁决，窗口变化无需 JS 重算。
const PCT_KEY = "pidock.dockWidthPct";
/** 旧版 px 记忆键（迁移用） */
const LEGACY_PX_KEY = "pidock.dockWidth";
const PCT_DEFAULT = 24;
function readDockPct(): number {
  try {
    const v = Number(localStorage.getItem(PCT_KEY));
    if (Number.isFinite(v) && v >= 8 && v <= 92) return v;
  } catch {
    // localStorage 不可用时用缺省
  }
  return PCT_DEFAULT;
}
const dockEl = ref<HTMLElement | null>(null);
const dockPct = ref(readDockPct());
let dragState: { startX: number; startPct: number; rowWidth: number } | null = null;
function onResizeStart(e: MouseEvent): void {
  const row = dockEl.value?.parentElement;
  dragState = {
    startX: e.clientX,
    startPct: dockPct.value,
    rowWidth: row?.clientWidth || window.innerWidth,
  };
  window.addEventListener("mousemove", onResizeMove);
  window.addEventListener("mouseup", onResizeEnd);
  document.body.style.cursor = "col-resize";
  document.body.style.userSelect = "none";
  e.preventDefault();
}
function onResizeMove(e: MouseEvent): void {
  if (!dragState) return;
  // 手柄在 dock 左缘：向左拖 = 变宽；实时换算成对话区百分比（越界由 CSS clamp 收敛）
  const deltaPx = dragState.startX - e.clientX;
  const pct = dragState.startPct + (deltaPx / dragState.rowWidth) * 100;
  dockPct.value = Math.min(95, Math.max(5, pct));
}
function onResizeEnd(): void {
  dragState = null;
  window.removeEventListener("mousemove", onResizeMove);
  window.removeEventListener("mouseup", onResizeEnd);
  document.body.style.cursor = "";
  document.body.style.userSelect = "";
  try {
    localStorage.setItem(PCT_KEY, String(Math.round(dockPct.value * 10) / 10));
  } catch {
    // localStorage 不可用时忽略
  }
}
onMounted(() => {
  // 旧版 px 记忆迁移成百分比（读一次即弃）
  if (dockPct.value === PCT_DEFAULT) {
    try {
      const px = Number(localStorage.getItem(LEGACY_PX_KEY));
      const row = dockEl.value?.parentElement;
      if (Number.isFinite(px) && px >= 260 && row?.clientWidth) {
        dockPct.value = Math.min(92, Math.max(8, (px / row.clientWidth) * 100));
      }
      localStorage.removeItem(LEGACY_PX_KEY);
    } catch {
      // 忽略迁移失败
    }
  }
});
const dockStyle = computed(() =>
  isNarrow.value
    ? undefined
    : { width: `clamp(260px, ${dockPct.value}%, min(1280px, calc(100% - 420px)))` },
);

const isHome = computed(() => !!summary.value && !!props.store.homeDir && summary.value.cwd === props.store.homeDir);
const projectName = computed(() => {
  if (!summary.value) return "";
  if (isHome.value) return "本地任务";
  const norm = summary.value.cwd.replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1) || summary.value.cwd;
});
const modelShort = computed(() => {
  const m = summary.value?.model;
  if (!m) return "";
  const slash = m.indexOf("/");
  return slash > 0 ? m.slice(slash + 1) : m;
});
const PERM_LABELS: Record<string, string> = {
  plan: "计划模式",
  confirm: "变更前确认",
  "edit-auto": "自动编辑",
  full: "完全访问",
};
const permLabel = computed(() => {
  const sid = props.store.activeId;
  return PERM_LABELS[sid ? (props.store.permissionModes[sid] ?? "") : ""] ?? "计划模式";
});

// ---- 收起为小圆标（按会话记忆，localStorage 持久化） ----
const CHIP_KEY = "pidock.progressChips";
function readChips(): Set<string> {
  try {
    const v = JSON.parse(localStorage.getItem(CHIP_KEY) ?? "[]");
    return new Set(Array.isArray(v) ? v.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}
const chips = ref<Set<string>>(readChips());
watch(
  chips,
  (v) => {
    try {
      localStorage.setItem(CHIP_KEY, JSON.stringify([...v]));
    } catch {
      // localStorage 不可用时忽略
    }
  },
  { deep: true },
);

const expanded = computed(
  () =>
    !!props.store.activeId &&
    !chips.value.has(props.store.activeId) &&
    (todos.value.length > 0 || !!props.expert || runs.value.length > 0) &&
    // 窄屏：停靠第三栏会挤没聊天区，默认收起为小圆标，点圆标以浮层展开
    (!isNarrow.value || userExpanded.value),
);
function collapse(): void {
  if (!props.store.activeId) return;
  chips.value = new Set(chips.value).add(props.store.activeId);
  userExpanded.value = false;
}
function expand(): void {
  if (!props.store.activeId) return;
  const next = new Set(chips.value);
  next.delete(props.store.activeId);
  chips.value = next;
  userExpanded.value = true;
}

/** 窄屏断点与 ProgressCard 样式的浮层断点保持一致 */
const isNarrow = ref(typeof matchMedia === "function" && matchMedia("(max-width: 768px)").matches);
if (typeof matchMedia === "function") {
  const mq = matchMedia("(max-width: 768px)");
  const onChange = (e: MediaQueryListEvent): void => {
    isNarrow.value = e.matches;
    if (e.matches) userExpanded.value = false;
  };
  mq.addEventListener("change", onChange);
  onBeforeUnmount(() => mq.removeEventListener("change", onChange));
}
/** 窄屏下用户点小圆标的主动展开（宽屏恒为停靠态，不参与判断） */
const userExpanded = ref(false);

function copyPath(): void {
  const cwd = summary.value?.cwd;
  if (cwd) void navigator.clipboard?.writeText(cwd).catch(() => {});
}
</script>

<template>
  <!-- 停靠态：第三栏，占布局不遮挡内容（窄屏为右侧浮层，遮罩点击收起） -->
  <div v-if="expanded && isNarrow" class="pcol-scrim" @click="collapse" />
  <aside v-if="expanded" ref="dockEl" class="pcol" :style="dockStyle">
    <!-- 左缘拖拽手柄：调 dock 宽度（260–560，localStorage 记忆；窄屏浮层不显示） -->
    <span v-if="!isNarrow" class="rz-grip" title="拖动调整宽度" @mousedown="onResizeStart"></span>
    <header class="head">
      <template v-if="detailRun">
        <Icon name="git-branch-line" :size="14" />
        <span class="title">子代理</span>
      </template>
      <template v-else-if="expert">
        <Icon name="user-star-line" :size="14" />
        <span class="title">专家</span>
      </template>
      <template v-else>
        <Icon name="check-double-line" :size="14" />
        <span class="title">任务清单</span>
      </template>
      <span v-if="todos.length && !detailRun" class="count" :class="{ all: done === todos.length }">{{ done }}/{{ todos.length }}</span>
      <span class="flex-sp"></span>
      <button class="hbtn" title="收起为小圆标" @click="collapse">
        <Icon name="arrow-right-line" :size="13" />
      </button>
    </header>

    <!-- 子代理工作流详情（第二层：列表行点击进入） -->
    <SubagentDetail v-if="detailRun" :store="store" :run="detailRun" @back="selectedChild = null" />

    <template v-else>
    <!-- 专家档案卡：人物介绍式排版（居中头像 + 名称/简介 + 角色提示词 + 资源标签） -->
    <div v-if="expert" class="expert-card">
      <div class="e-hero">
        <span
          class="e-avatar"
          :style="!expert.avatar && expert.avatar_color ? { background: expert.avatar_color, color: '#fff', borderColor: expert.avatar_color } : undefined"
        >
          <img v-if="expert.avatar" :src="expert.avatar" alt="" />
          <Icon v-else :name="expert.icon || 'user-star-line'" :size="28" />
        </span>
        <div class="e-name">{{ expert.name }}</div>
        <div class="e-desc">{{ expert.description || "专家智能体" }}</div>
      </div>
      <div class="e-sec">
        <div class="e-label">角色定位</div>
        <p class="e-prompt" :title="expert.prompt">{{ expert.prompt || "（未填写角色提示词）" }}</p>
      </div>
      <div class="e-sec">
        <div class="e-label">资源配置</div>
        <div class="e-tags">
          <span class="tag" :title="expert.skills.join('、')">技能 {{ expert.skills.length || "不限" }}</span>
          <span class="tag" :title="expert.extensions.join('、')">MCP {{ expert.extensions.length || "不限" }}</span>
          <span v-if="expert.exclude_tools?.length" class="tag warn">禁 {{ expert.exclude_tools.join("/") }}</span>
          <span v-if="expert.knowledge_dirs.length" class="tag">知识库 {{ expert.knowledge_dirs.length }}</span>
        </div>
      </div>
    </div>

    <div class="proj">
      <div class="p-name" :title="summary?.cwd">
        <Icon name="folder-line" :size="13" />
        <span>{{ projectName }}</span>
      </div>
      <button v-if="!isHome" class="p-path" :title="`${summary?.cwd}（点击复制）`" @click="copyPath">
        {{ summary?.cwd }}
      </button>
      <div class="p-chips">
        <span v-if="modelShort" class="tag">{{ modelShort }}</span>
        <span class="tag">{{ permLabel }}</span>
      </div>
    </div>

    <div v-if="todos.length" class="bar"><i :style="{ width: `${todos.length ? (done / todos.length) * 100 : 0}%` }"></i></div>

    <ol v-if="todos.length" class="list">
      <li v-for="(t, i) in todos" :key="i" :class="t.status">
        <span class="st">
          <Icon v-if="t.status === 'completed'" name="checkbox-circle-fill" :size="14" />
          <span v-else-if="t.status === 'in_progress'" class="spin"></span>
          <Icon v-else name="circle-line" :size="14" />
        </span>
        <span class="txt">{{ t.status === 'in_progress' ? (t.active_form ?? t.content) : t.content }}</span>
      </li>
    </ol>

    <!-- 子代理运行段：task 工具派生的会话条目，点击进入工作流详情 -->
    <div v-if="runs.length" class="sa-sec">
      <div class="sa-head">
        <Icon name="git-branch-line" :size="12" />
        <span>子代理</span>
        <span v-if="runningCount" class="sa-live">{{ runningCount }} 运行中</span>
        <span v-if="waitingCount" class="sa-wait">{{ waitingCount }} 待处理</span>
      </div>
      <div
        v-for="r in runs"
        :key="r.session_id"
        class="sa-row"
        role="button"
        tabindex="0"
        :title="r.activity || r.name"
        @click="selectedChild = r.session_id"
        @keydown.enter="selectedChild = r.session_id"
      >
        <span class="st">
          <Icon v-if="r.state === 'idle'" name="checkbox-circle-fill" :size="13" />
          <span v-else-if="['waiting_approval', 'waiting_ask'].includes(r.state)" class="warn-dot"></span>
          <span v-else class="spin"></span>
        </span>
        <span class="sa-col">
          <span class="sa-name">{{ r.expert_name || r.name }}</span>
          <span v-if="r.activity && r.state !== 'idle'" class="sa-activity">{{ r.activity }}</span>
        </span>
        <span v-if="r.tokens" class="sa-tok">{{ fmtTokens(r.tokens) }}</span>
        <!-- 行内终止：未完成的子代理可直接中止（审批/提问等待中同样可终止） -->
        <button
          v-if="r.state !== 'idle'"
          class="sa-stop"
          title="终止子代理"
          @click.stop="abortRun(r)"
        >
          <Icon name="close-circle-line" :size="13" />
        </button>
      </div>
    </div>
    </template>
  </aside>
  <!-- 收起态：小圆标悬浮在对话区右上（绝对定位，不占布局）；专家会话显示专家头像 -->
  <div v-else-if="(todos.length || expert || runs.length) && store.activeId" class="chip-float">
    <button
      v-if="runs.length"
      class="chip sub-chip"
      :class="{ wait: waitingCount > 0 }"
      :title="runningCount ? `${runningCount} 个子代理运行中` : `${runs.length} 个子代理已结束`"
      @click="expand"
    >
      <Icon name="git-branch-line" :size="13" />
      <span v-if="waitingCount">{{ waitingCount }} 待</span>
      <span v-else>{{ runningCount || runs.length }}</span>
    </button>
    <button v-if="expert" class="chip expert-chip" :title="`专家 · ${expert.name}`" @click="expand">
      <span
        class="chip-avatar"
        :style="!expert.avatar && expert.avatar_color ? { background: expert.avatar_color, color: '#fff' } : undefined"
      >
        <img v-if="expert.avatar" :src="expert.avatar" alt="" />
        <Icon v-else :name="expert.icon || 'user-star-line'" :size="12" />
      </span>
      <span>{{ expert.name }}</span>
      <span v-if="todos.length" class="chip-count">{{ done }}/{{ todos.length }}</span>
    </button>
    <button v-else-if="todos.length" class="chip" :title="`${projectName} · ${done}/${todos.length} 已完成`" @click="expand">
      <Icon name="checkbox-circle-fill" :size="13" />
      <span>{{ done }}/{{ todos.length }}</span>
    </button>
  </div>
</template>

<style scoped>
/* 停靠第三栏：作为 .conv-row 的 flex 子项（宽度 = clamp(260px, N%, min(1280px, 100%-420px))，
 * 由拖拽以对话区百分比控制；此处的 320px 仅为内联回退） */
.pcol {
  width: 320px;
  flex: none;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-panel);
  border-left: 1px solid var(--pd-border);
  padding: 12px 14px 16px;
  position: relative;
}
/* 左缘拖拽手柄：热区落在 dock 左内缘（滚动容器负偏移会被裁剪），悬停亮起 */
.rz-grip {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  width: 5px;
  cursor: col-resize;
  z-index: 5;
}
.rz-grip:hover,
.rz-grip:active {
  background: color-mix(in srgb, var(--pd-accent) 45%, transparent);
}
/* 窄屏：第三栏改浮层（.conv-row 为定位上下文），不挤占聊天区宽度；
 * 点头部按钮或遮罩收起为悬浮小圆标 */
.pcol-scrim { display: none; }
@media (max-width: 768px) {
  .pcol-scrim {
    display: block;
    position: absolute;
    inset: 0;
    z-index: 44;
    background: rgba(0, 0, 0, 0.45);
  }
  .pcol {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 45;
    width: min(84vw, 300px);
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.4);
  }
  .rz-grip { display: none; }
}
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  flex: none;
}
.title { color: var(--pd-text); }
.count { color: var(--pd-green); font-weight: 500; }
.count.all { color: var(--pd-text-3); }
.flex-sp { flex: 1; }
.hbtn {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 0;
}
.hbtn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }

/* 专家档案卡：人物介绍式排版 */
.expert-card {
  margin-top: 10px;
  padding: 18px 12px 12px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 10px;
  display: flex;
  flex-direction: column;
  flex: none;
}
.e-hero { display: flex; flex-direction: column; align-items: center; gap: 6px; }
.e-avatar {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  overflow: hidden;
  display: grid;
  place-items: center;
  background: var(--pd-bg-hover);
  color: var(--pd-accent);
  border: 2px solid var(--pd-border);
  flex: none;
}
.e-avatar img { width: 100%; height: 100%; object-fit: cover; }
.e-name { font-size: calc(14px * var(--pd-font-scale)); font-weight: 700; color: var(--pd-text); text-align: center; }
.e-desc {
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  text-align: center;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 100%;
}
.e-sec { border-top: 1px dashed var(--pd-border-soft); margin-top: 14px; padding-top: 10px; }
.e-label {
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
  letter-spacing: 1px;
  margin-bottom: 5px;
}
.e-prompt {
  margin: 0;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-2);
  line-height: 1.65;
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.e-tags { display: flex; flex-wrap: wrap; gap: 5px; }
.tag.warn { color: var(--pd-red); border-color: color-mix(in srgb, var(--pd-red) 35%, transparent); }

.proj {
  margin-top: 10px;
  padding: 8px 9px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: none;
}
.p-name {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text);
  font-weight: 500;
  min-width: 0;
}
.p-name span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.p-path {
  text-align: left;
  border: none;
  background: none;
  padding: 0;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.p-path:hover { color: var(--pd-accent); }
.p-chips { display: flex; gap: 5px; flex-wrap: wrap; }
.tag {
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  border: 1px solid var(--pd-border-soft);
  border-radius: 4px;
  padding: 1px 6px;
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bar {
  margin-top: 10px;
  height: 4px;
  border-radius: 2px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  overflow: hidden;
  flex: none;
}
.bar i {
  display: block;
  height: 100%;
  background: var(--pd-green);
  border-radius: 2px;
  transition: width 0.3s;
}

.list {
  list-style: none;
  margin: 8px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.list li {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  padding: 3px 2px;
  font-size: calc(12.5px * var(--pd-font-scale));
  line-height: 1.45;
  color: var(--pd-text-2);
  border-radius: 6px;
}
.list li:hover { background: var(--pd-bg-hover); }
.st { flex: none; display: grid; place-items: center; margin-top: 2px; color: var(--pd-text-4); }
li.completed .st { color: var(--pd-green); }
li.completed .txt {
  color: var(--pd-text-4);
  text-decoration: line-through;
  text-decoration-color: var(--pd-text-4);
}
li.in_progress { color: var(--pd-text); }
li.in_progress .st { color: var(--pd-accent); }
.spin {
  width: 11px;
  height: 11px;
  border: 2px solid var(--pd-accent);
  border-top-color: transparent;
  border-radius: 50%;
  animation: pc-spin 0.8s linear infinite;
}
@keyframes pc-spin {
  to { transform: rotate(360deg); }
}
.txt { min-width: 0; word-break: break-word; }

/* 收起态小圆标：悬浮于对话区右上角（.conv-row 上）；多枚时纵向堆叠 */
.chip-float {
  position: absolute;
  top: 12px;
  right: 16px;
  z-index: 40;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 6px;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--pd-border);
  border-radius: 13px;
  background: var(--pd-bg-panel);
  color: var(--pd-green);
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}
.chip:hover { border-color: var(--pd-accent); }
.chip.expert-chip { color: var(--pd-text-2); }
.chip.sub-chip { color: var(--pd-accent); }
.chip.sub-chip.wait { color: var(--pd-orange, #e08a3c); }

/* 子代理运行段（dock 内列表） */
.sa-sec {
  margin-top: 12px;
  border-top: 1px dashed var(--pd-border-soft);
  padding-top: 10px;
}
.sa-head {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
  letter-spacing: 1px;
  margin-bottom: 6px;
}
.sa-live { color: var(--pd-accent); letter-spacing: 0; }
.sa-wait { color: var(--pd-orange, #e08a3c); letter-spacing: 0; }
.sa-row {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  border: none;
  background: none;
  text-align: left;
  padding: 5px 6px;
  border-radius: 6px;
  cursor: pointer;
}
.sa-row:hover { background: var(--pd-bg-hover); }
.sa-row .st { flex: none; display: grid; place-items: center; color: var(--pd-text-4); }
.sa-row .st :deep(svg) { color: var(--pd-green); }
.sa-col { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
.sa-name {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sa-activity {
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sa-tok {
  flex: none;
  font-size: calc(10px * var(--pd-font-scale));
  color: var(--pd-text-4);
  font-variant-numeric: tabular-nums;
}
.sa-stop {
  flex: none;
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 5px;
  background: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 0;
}
.sa-stop:hover {
  background: color-mix(in srgb, var(--pd-red) 12%, transparent);
  color: var(--pd-red);
}
.warn-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: var(--pd-orange, #e08a3c);
  animation: sa-pulse 1.2s ease-in-out infinite;
}
@keyframes sa-pulse {
  50% { opacity: 0.35; }
}
.chip-avatar {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  overflow: hidden;
  display: grid;
  place-items: center;
  background: var(--pd-bg-hover);
  color: var(--pd-accent);
  flex: none;
}
.chip-avatar img { width: 100%; height: 100%; object-fit: cover; }
.chip-count { color: var(--pd-green); }
</style>
