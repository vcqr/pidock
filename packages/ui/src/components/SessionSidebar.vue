<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import type { SessionSummaryUi } from "../store.js";
import { basename, relTime } from "../utils/time.js";
import Icon from "./Icon.vue";

const props = defineProps<{
  sessions: SessionSummaryUi[];
  activeId: string | null;
  /** show the 插件/技能/MCP tool nav (desktop only) */
  showToolNav?: boolean;
  /** web has no settings center — hide the entry */
  showSettingsBtn?: boolean;
  /** currently open main-area tool view, for nav highlight */
  activeTool?: string;
}>();
const emit = defineEmits<{
  select: [sessionId: string];
  "new-task": [];
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

const I = {
  arrowL: ["M19 12H5m7-7-7 7 7 7"],
  arrowR: ["M5 12h14m-7-7 7 7-7 7"],
  plusCircle: ["M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z", "M12 8v8M8 12h8"],
  search: ["M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14z", "m20 20-4-4"],
  clock: ["M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z", "M12 9v4l2.5 2.5"],
  grid: [
    "M3 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5z",
    "M13 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2a2 2 0 0 1-2 2h-2a2 2 0 0 1-2-2V5z",
    "M3 15a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-2z",
    "M13 15a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2a2 2 0 0 1-2 2h-2a2 2 0 0 1-2-2v-2z",
  ],
  zap: ["M13 2 4 14h6l-1 8 9-12h-6l1-8z"],
  layers: ["M12 2 2 7l10 5 10-5-10-5z", "M2 12l10 5 10-5", "M2 17l10 5 10-5"],
  mcp: ["M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0V8z", "M12 17v4"],
  folder: ["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"],
  chevD: ["m6 9 6 6 6-6"],
  gear: [
    "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z",
    "M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  ],
};

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

interface Group {
  project: string;
  cwd: string;
  sessions: SessionSummaryUi[];
}
const groups = computed<Group[]>(() => {
  const map = new Map<string, Group>();
  for (const s of filtered.value) {
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
  <aside class="sidebar">
    <div class="sb-head">
      <div class="logo">π</div>
      <button class="icon-btn" disabled title="后退"><Icon :paths="I.arrowL" :size="16" /></button>
      <button class="icon-btn" disabled title="前进"><Icon :paths="I.arrowR" :size="16" /></button>
      <span class="flex-sp"></span>
      <slot name="actions" />
    </div>

    <nav class="sb-nav">
      <div class="nav-item" @click="emit('new-task')">
        <Icon :paths="I.plusCircle" :size="16" />新建任务<span class="kbd">Ctrl+N</span>
      </div>
      <div class="nav-item" @click="openSearch">
        <Icon :paths="I.search" :size="16" />搜索<span class="kbd">Ctrl+K</span>
      </div>
      <div class="nav-item disabled" title="开发中">
        <Icon :paths="I.clock" :size="16" />自动化
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'plugins' }" @click="emit('open-tools', 'plugins')">
        <Icon :paths="I.grid" :size="16" />插件
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'skills' }" @click="emit('open-tools', 'skills')">
        <Icon :paths="I.zap" :size="16" />技能
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'providers' }" @click="emit('open-tools', 'providers')">
        <Icon :paths="I.layers" :size="16" />模型供应商
      </div>
      <div v-if="showToolNav" class="nav-item" :class="{ active: activeTool === 'mcp' }" @click="emit('open-tools', 'mcp')">
        <Icon :paths="I.mcp" :size="16" />MCP
      </div>
    </nav>

    <div v-if="searchOpen || search" class="search-row">
      <input ref="searchInput" v-model="search" placeholder="搜索会话…" @keydown.esc="searchOpen = false" />
    </div>

    <div class="sb-tools">
      <button class="seg-btn" :class="{ active: !grouped }" @click="grouped = false"># 分组</button>
      <button class="seg-btn" :class="{ active: grouped }" @click="grouped = true">
        <Icon :paths="I.folder" :size="13" />项目
      </button>
      <span class="flex-sp"></span>
    </div>

    <div class="sb-scroll">
      <!-- 项目分组视图 -->
      <template v-if="grouped">
        <div class="sb-title">项目</div>
        <template v-for="g in groups" :key="g.cwd + g.project">
          <div class="folder-row" :title="g.cwd" @click="toggleGroup(g.project)">
            <span class="chev" :class="{ fold: collapsed.has(g.project) }">
              <Icon :paths="I.chevD" :size="12" :stroke="2" />
            </span>
            <Icon :paths="I.folder" :size="15" />
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

      <!-- 按时间平铺视图 -->
      <template v-else>
        <div class="sb-title">任务</div>
        <div
          v-for="s in filtered"
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
        <div v-if="!filtered.length" class="empty">暂无任务</div>
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
      ><Icon :paths="I.gear" :size="15" /></button>
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
