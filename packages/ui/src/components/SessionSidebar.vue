<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import type { SessionSummaryUi } from "../store.js";
import { basename, relTime } from "../utils/time.js";
import Icon from "./Icon.vue";

const props = defineProps<{
  sessions: SessionSummaryUi[];
  activeId: string | null;
  /** host 主目录（session.list 下发）；cwd 等于它的会话视为「不在项目中」的任务 */
  homeDir?: string | null;
  /** show the 插件/技能/MCP tool nav (desktop only) */
  showToolNav?: boolean;
  /** web has no settings center — hide the entry */
  showSettingsBtn?: boolean;
  /** currently open main-area tool view, for nav highlight */
  activeTool?: string;
  /** 面板宽度（px）；缺省用 CSS 默认 260。拖拽分隔条实时改写 */
  width?: number;
}>();
const emit = defineEmits<{
  select: [sessionId: string];
  "new-task": [];
  /** 点击项目分组目录行：右侧打开默认输入页并预选该目录 */
  "open-project": [cwd: string];
  "open-settings": [tab?: string];
  "open-tools": [tool: "plugins" | "skills" | "providers" | "mcp"];
}>();

const search = ref("");
const searchOpen = ref(false);
const searchInput = ref<HTMLInputElement | null>(null);
/** true = 按项目分组（「项目」页签）；false = 按时间平铺（「# 分组」页签） */
const grouped = ref(true);
const collapsed = ref(new Set<string>());
const expanded = ref(new Set<string>());
const PREVIEW = 5;


const filtered = computed(() => {
  const q = search.value.trim().toLowerCase();
  const list = [...props.sessions].sort(
    (a, b) => ((b.updated_at ?? b.created_at) > (a.updated_at ?? a.created_at) ? 1 : -1),
  );
  if (!q) return list;
  return list.filter(
    (s) =>
      (s.name ?? "").toLowerCase().includes(q) ||
      s.cwd.toLowerCase().includes(q) ||
      (s.model ?? "").toLowerCase().includes(q),
  );
});

/** 会话是否「不在项目中」（cwd 为主目录或为空）；主目录未知（web 快照）时不做拆分 */
function isLooseTask(s: SessionSummaryUi): boolean {
  if (!props.homeDir) return false;
  return !s.cwd || s.cwd === props.homeDir;
}

/** 任务页签：不在项目中的会话；主目录未知时退回显示全部（与原行为一致） */
const taskList = computed(() =>
  props.homeDir ? filtered.value.filter((s) => isLooseTask(s)) : filtered.value,
);

interface Group {
  project: string;
  cwd: string;
  sessions: SessionSummaryUi[];
}
const groups = computed<Group[]>(() => {
  const map = new Map<string, Group>();
  for (const s of filtered.value) {
    if (isLooseTask(s)) continue; // 不在项目中的任务只进任务页签
    const project = basename(s.cwd) || "未分类";
    if (!map.has(project)) map.set(project, { project, cwd: s.cwd, sessions: [] });
    map.get(project)!.sessions.push(s);
  }
  return [...map.values()].sort((a, b) => {
    const la = a.sessions[0], lb = b.sessions[0];
    return ((lb?.updated_at ?? lb?.created_at ?? "") > (la?.updated_at ?? la?.created_at ?? "") ? 1 : -1);
  });
});

function visibleIn(g: Group): SessionSummaryUi[] {
  return expanded.value.has(g.project) ? g.sessions : g.sessions.slice(0, PREVIEW);
}
function toggleGroup(project: string): void {
  const next = new Set(collapsed.value);
  next.has(project) ? next.delete(project) : next.add(project);
  collapsed.value = next;
}
function toggleMore(project: string): void {
  const next = new Set(expanded.value);
  next.has(project) ? next.delete(project) : next.add(project);
  expanded.value = next;
}

const busyStates = new Set([
  "responding", "thinking", "executing_tool", "compacting", "retrying", "waiting_approval", "running",
]);
const stateLabel: Record<string, string> = {
  idle: "空闲", thinking: "思考中", responding: "回复中", executing_tool: "执行工具",
  compacting: "压缩上下文", retrying: "重试中", running: "运行中",
  waiting_approval: "等待审批", error: "出错", done: "完成",
};
function dotClass(s: SessionSummaryUi): string {
  if (s.state === "error") return "err";
  if (busyStates.has(s.state)) return "busy";
  return s.open ? "on" : "off";
}
function rowTitle(s: SessionSummaryUi): string {
  const st = s.open ? (stateLabel[s.state] ?? s.state) : "离线";
  return [s.name || basename(s.cwd), s.model, st].filter(Boolean).join(" · ");
}

function openSearch(): void {
  searchOpen.value = true;
  requestAnimationFrame(() => searchInput.value?.focus());
}

function onKey(e: KeyboardEvent): void {
  if (!(e.ctrlKey || e.metaKey)) return;
  const k = e.key.toLowerCase();
  if (k === "n") {
    e.preventDefault();
    emit("new-task");
  } else if (k === "k") {
    e.preventDefault();
    openSearch();
  }
}

onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <aside class="sidebar" :style="width ? { width: width + 'px', minWidth: width + 'px' } : undefined">
    <div class="sb-head">
      <div class="logo">π</div>
      <button class="icon-btn" disabled title="后退"><Icon name="arrow-left-line" :size="16" /></button>
      <button class="icon-btn" disabled title="前进"><Icon name="arrow-right-line" :size="16" /></button>
      <span class="flex-sp"></span>
      <slot name="actions" />
    </div>

    <nav class="sb-nav">
      <div class="nav-item" @click="emit('new-task')">
        <Icon name="add-circle-line" :size="16" />新建任务<span class="kbd">Ctrl+N</span>
      </div>
      <div class="nav-item" @click="openSearch">
        <Icon name="search-line" :size="16" />搜索<span class="kbd">Ctrl+K</span>
      </div>
      <div class="nav-item disabled" title="开发中">
        <Icon name="time-line" :size="16" />自动化
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'plugins' }" @click="emit('open-tools', 'plugins')">
        <Icon name="puzzle-2-line" :size="16" />插件
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'skills' }" @click="emit('open-tools', 'skills')">
        <Icon name="magic-line" :size="16" />技能
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'providers' }" @click="emit('open-tools', 'providers')">
        <Icon name="stack-line" :size="16" />模型供应商
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'mcp' }" @click="emit('open-tools', 'mcp')">
        <Icon name="plug-line" :size="16" />MCP
      </div>
    </nav>

    <div v-if="searchOpen || search" class="search-row">
      <input ref="searchInput" v-model="search" placeholder="搜索会话…" @keydown.esc="searchOpen = false" />
    </div>

    <div class="sb-tools">
      <button class="seg-btn" :class="{ active: !grouped }" @click="grouped = false">任务</button>
      <button class="seg-btn" :class="{ active: grouped }" @click="grouped = true">
        <Icon name="folder-line" :size="13" />项目
      </button>
      <span class="flex-sp"></span>
    </div>

    <div class="sb-scroll">
      <!-- 项目分组视图 -->
      <template v-if="grouped">
        <div class="sb-title">项目</div>
        <template v-for="g in groups" :key="g.cwd + g.project">
          <div class="folder-row" :title="g.cwd" @click="emit('open-project', g.cwd)">
            <span
              class="chev"
              :class="{ fold: collapsed.has(g.project) }"
              title="展开 / 折叠"
              @click.stop="toggleGroup(g.project)"
            >
              <Icon name="arrow-down-s-line" :size="12" />
            </span>
            <Icon name="folder-line" :size="15" />
            <span class="fname">{{ g.project }}</span>
            <span class="g-count">{{ g.sessions.length }}</span>
          </div>
          <template v-if="!collapsed.has(g.project)">
            <div
              v-for="s in visibleIn(g)"
              :key="s.session_id"
              class="task-row"
              :class="{ active: s.session_id === activeId }"
              :title="rowTitle(s)"
              @click="emit('select', s.session_id)"
            >
              <span class="dot" :class="dotClass(s)"></span>
              <span class="txt">{{ s.name || basename(s.cwd) }}</span>
              <span class="time">{{ relTime(s.updated_at ?? s.created_at) }}</span>
            </div>
            <div
              v-if="g.sessions.length > PREVIEW"
              class="show-more"
              @click="toggleMore(g.project)"
            >{{ expanded.has(g.project) ? "收起" : "显示更多" }}</div>
          </template>
        </template>
        <div v-if="!groups.length" class="empty">
          {{ search ? "没有匹配的会话" : "暂无会话，点上方「新建任务」" }}
        </div>
      </template>

      <!-- 任务视图：不在项目中的会话（按时间平铺） -->
      <template v-else>
        <div class="sb-title">任务</div>
        <div
          v-for="s in taskList"
          :key="s.session_id"
          class="task-row flat"
          :class="{ active: s.session_id === activeId }"
          :title="rowTitle(s)"
          @click="emit('select', s.session_id)"
        >
          <span class="dot" :class="dotClass(s)"></span>
          <span class="txt">{{ s.name || basename(s.cwd) }}</span>
          <span class="time">{{ relTime(s.updated_at ?? s.created_at) }}</span>
        </div>
        <div v-if="!taskList.length" class="empty">
          {{ search ? "没有匹配的会话" : "暂无任务，未选项目创建的会话会出现在这里" }}
        </div>
      </template>
    </div>

    <div class="sb-foot">
      <div class="avatar">π</div>
      <span class="uname">PiDock</span>
      <span class="flex-sp"></span>
      <slot name="bottom" />
      <button
        v-if="showSettingsBtn !== false"
        class="icon-btn"
        title="设置"
        @click="emit('open-settings')"
      ><Icon name="settings-3-line" :size="15" /></button>
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  width: 260px;
  min-width: 260px;
  background: var(--pd-bg-panel);
  border-right: 1px solid var(--pd-border-soft);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.flex-sp { flex: 1; }

.sb-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 14px 12px 10px;
}
.logo {
  width: 28px;
  height: 28px;
  border-radius: 7px;
  background: var(--pd-text);
  color: var(--pd-bg);
  font-weight: 800;
  font-size: 15px;
  display: grid;
  place-items: center;
  margin-right: 4px;
}
.icon-btn {
  width: 28px;
  height: 28px;
  border-radius: 7px;
  display: grid;
  place-items: center;
  color: var(--pd-text-3);
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
}
.icon-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.icon-btn:disabled { opacity: 0.45; pointer-events: none; }

.sb-nav { padding: 2px 8px; }
.nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  color: var(--pd-text-2);
  cursor: pointer;
  margin-bottom: 2px;
  font-size: 13px;
  user-select: none;
}
.nav-item:hover { background: var(--pd-bg-hover); }
.nav-item.active { background: var(--pd-bg-active); color: var(--pd-text); }
.nav-item.disabled { opacity: 0.45; pointer-events: none; }
.nav-item .kbd {
  margin-left: auto;
  font-size: 11px;
  color: var(--pd-text-4);
}

.search-row { padding: 2px 12px 8px; }
.search-row input {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  color: var(--pd-text);
  border-radius: 8px;
  padding: 6px 10px;
  font-size: 12px;
}
.search-row input:focus { outline: none; border-color: var(--pd-accent); }

.sb-tools {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 12px;
}
.seg-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  border-radius: 999px;
  font-size: 12px;
  color: var(--pd-text-3);
  border: 1px solid transparent;
  background: none;
  cursor: pointer;
}
.seg-btn:hover { color: var(--pd-text-2); }
.seg-btn.active {
  background: var(--pd-bg);
  border-color: var(--pd-border);
  color: var(--pd-text);
}

.sb-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 2px 8px 8px;
  min-height: 80px;
}
.sb-scroll::-webkit-scrollbar { width: 8px; }
.sb-scroll::-webkit-scrollbar-thumb {
  background: var(--pd-scrollbar);
  border-radius: 4px;
}
.sb-title {
  font-size: 12px;
  color: var(--pd-text-3);
  padding: 12px 10px 6px;
}
.folder-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 8px;
  color: var(--pd-text-2);
  cursor: pointer;
  font-size: 13px;
  user-select: none;
}
.folder-row:hover { background: var(--pd-bg-hover); }
.folder-row svg { color: var(--pd-text-3); }
.chev {
  display: grid;
  place-items: center;
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.chev.fold { transform: rotate(-90deg); }
.fname {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.g-count {
  margin-left: auto;
  font-size: 10.5px;
  color: var(--pd-text-4);
}
.task-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px 6px 30px;
  border-radius: 8px;
  color: var(--pd-text-2);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  font-size: 13px;
}
.task-row.flat { padding-left: 10px; }
.task-row:hover { background: var(--pd-bg-hover); }
.task-row.active { background: var(--pd-bg-active); color: var(--pd-text); }
.task-row .txt {
  overflow: hidden;
  text-overflow: ellipsis;
}
.task-row .time {
  margin-left: auto;
  font-size: 11px;
  color: var(--pd-text-4);
  flex: none;
}
.dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex: none;
  background: var(--pd-text-4);
}
.dot.busy { background: var(--pd-yellow); animation: pulse 1.6s ease-in-out infinite; }
.dot.err { background: var(--pd-red); }
.dot.on { background: var(--pd-green); }
.dot.off { background: transparent; box-shadow: inset 0 0 0 1.5px var(--pd-text-4); }
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
.show-more {
  padding: 6px 10px 6px 30px;
  color: var(--pd-text-4);
  font-size: 12px;
  cursor: pointer;
  user-select: none;
}
.show-more:hover { color: var(--pd-text-3); }
.empty {
  padding: 12px 10px;
  color: var(--pd-text-4);
  font-size: 12px;
  line-height: 1.6;
}

.sb-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  border-top: 1px solid var(--pd-border-soft);
}
.avatar {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  background: linear-gradient(135deg, var(--pd-text), var(--pd-accent));
  color: var(--pd-bg);
  display: grid;
  place-items: center;
  font-size: 13px;
  font-weight: 700;
}
.uname { font-size: 13px; color: var(--pd-text-2); }
</style>
