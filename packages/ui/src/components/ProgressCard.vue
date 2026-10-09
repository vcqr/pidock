<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { TodoItem } from "@pidock/protocol";
import type { AgentStore, ExpertInfo } from "../store.js";
import Icon from "./Icon.vue";

/**
 * 任务进度第三栏（对话区右侧停靠，压缩对话流宽度而非遮挡）：
 * 专家段（绑定的专家头像/描述/角色提示词摘要/资源标签）+
 * 项目段（名称/路径/模型/权限）+ 进度段（TodoWrite 任务清单）。
 * TodoWrite 首次推送或会话绑定专家时自动展开；收起后缩成悬浮小圆标（专家会话显示头像），点击恢复；
 * 收起状态按会话记忆（localStorage）。
 */

const props = defineProps<{ store: AgentStore; expert?: ExpertInfo | null }>();

const todos = computed<TodoItem[]>(() =>
  props.store.activeId ? (props.store.todosBySession[props.store.activeId] ?? []) : [],
);
const summary = computed(() => props.store.sessions.find((s) => s.session_id === props.store.activeId));
const done = computed(() => todos.value.filter((t) => t.status === "completed").length);

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
    (todos.value.length > 0 || !!props.expert) &&
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
  <aside v-if="expanded" class="pcol">
    <header class="head">
      <template v-if="expert">
        <Icon name="user-star-line" :size="14" />
        <span class="title">专家</span>
      </template>
      <template v-else>
        <Icon name="check-double-line" :size="14" />
        <span class="title">任务清单</span>
      </template>
      <span v-if="todos.length" class="count" :class="{ all: done === todos.length }">{{ done }}/{{ todos.length }}</span>
      <span class="flex-sp"></span>
      <button class="hbtn" title="收起为小圆标" @click="collapse">
        <Icon name="arrow-right-line" :size="13" />
      </button>
    </header>

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
  </aside>
  <!-- 收起态：小圆标悬浮在对话区右上（绝对定位，不占布局）；专家会话显示专家头像 -->
  <div v-else-if="(todos.length || expert) && store.activeId" class="chip-float">
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
    <button v-else class="chip" :title="`${projectName} · ${done}/${todos.length} 已完成`" @click="expand">
      <Icon name="checkbox-circle-fill" :size="13" />
      <span>{{ done }}/{{ todos.length }}</span>
    </button>
  </div>
</template>

<style scoped>
/* 停靠第三栏：作为 .conv-row 的 flex 子项 */
.pcol {
  width: 300px;
  flex: none;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-panel);
  border-left: 1px solid var(--pd-border);
  padding: 12px 14px 16px;
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

/* 收起态小圆标：悬浮于对话区右上角（.conv-row 上） */
.chip-float {
  position: absolute;
  top: 12px;
  right: 16px;
  z-index: 40;
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
