<script setup lang="ts">
import { computed, ref, watch } from "vue";
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
    (todos.value.length > 0 || !!props.expert),
);
function collapse(): void {
  if (!props.store.activeId) return;
  chips.value = new Set(chips.value).add(props.store.activeId);
}
function expand(): void {
  if (!props.store.activeId) return;
  const next = new Set(chips.value);
  next.delete(props.store.activeId);
  chips.value = next;
}

function copyPath(): void {
  const cwd = summary.value?.cwd;
  if (cwd) void navigator.clipboard?.writeText(cwd).catch(() => {});
}
</script>

<template>
  <!-- 停靠态：第三栏，占布局不遮挡内容 -->
  <aside v-if="expanded" class="pcol">
    <header class="head">
      <template v-if="expert">
        <span
          class="head-avatar"
          :style="!expert.avatar && expert.avatar_color ? { background: expert.avatar_color, color: '#fff' } : undefined"
        >
          <img v-if="expert.avatar" :src="expert.avatar" alt="" />
          <Icon v-else :name="expert.icon || 'user-star-line'" :size="14" />
        </span>
        <span class="title">{{ expert.name }}</span>
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

    <!-- 专家段：绑定专家的角色与资源配置摘要 -->
    <div v-if="expert" class="expert">
      <div class="e-name" :title="expert.description">{{ expert.name }}</div>
      <div v-if="expert.description" class="e-desc">{{ expert.description }}</div>
      <p class="e-prompt" :title="expert.prompt">{{ expert.prompt || "（未填写角色提示词）" }}</p>
      <div class="e-tags">
        <span class="tag" :title="expert.skills.join('、')">技能 {{ expert.skills.length || "不限" }}</span>
        <span class="tag" :title="expert.extensions.join('、')">MCP {{ expert.extensions.length || "不限" }}</span>
        <span v-if="expert.exclude_tools?.length" class="tag">禁 {{ expert.exclude_tools.join("/") }}</span>
        <span v-if="expert.knowledge_dirs.length" class="tag">知识库 {{ expert.knowledge_dirs.length }}</span>
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
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--pd-text-2);
  font-size: 12.5px;
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

.head-avatar {
  width: 20px;
  height: 20px;
  border-radius: 50%;
  overflow: hidden;
  display: grid;
  place-items: center;
  background: var(--pd-bg-hover);
  color: var(--pd-accent);
  flex: none;
}
.head-avatar img { width: 100%; height: 100%; object-fit: cover; }

/* 专家段：与项目段同款卡片，头像行 + 描述 + 提示词摘要 + 资源标签 */
.expert {
  margin-top: 10px;
  padding: 9px 10px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 5px;
  flex: none;
}
.e-name { font-size: 13px; font-weight: 600; color: var(--pd-accent); }
.e-desc {
  font-size: 11.5px;
  color: var(--pd-text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.e-prompt {
  margin: 0;
  font-size: 11.5px;
  color: var(--pd-text-3);
  line-height: 1.55;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.e-tags { display: flex; gap: 5px; flex-wrap: wrap; }

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
  font-size: 12.5px;
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
  font-size: 11px;
  color: var(--pd-text-4);
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.p-path:hover { color: var(--pd-accent); }
.p-chips { display: flex; gap: 5px; flex-wrap: wrap; }
.tag {
  font-size: 10.5px;
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
  font-size: 12.5px;
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
  font-size: 12px;
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
