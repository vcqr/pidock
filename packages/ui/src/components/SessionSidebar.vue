<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { SessionSummaryUi } from "../store.js";
import { basename, relTime } from "../utils/time.js";
import { REVEAL_PATH } from "../databus.js";
import Icon from "./Icon.vue";
import { appConfirm } from "../confirm.js";

const props = defineProps<{
  sessions: SessionSummaryUi[];
  activeId: string | null;
  /** host 主目录（session.list 下发）；cwd 等于它的会话视为「不在项目中」的任务 */
  homeDir?: string | null;
  /** show the 插件/技能/MCP tool nav (desktop only) */
  showToolNav?: boolean;
  /** 启用「自动化」入口（桌面端有 Rust 调度器；web 无后端保持禁用占位） */
  showAutomation?: boolean;
  /** 启用「专家」入口（桌面端；web 暂缓） */
  showExperts?: boolean;
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
  /** 项目右键「查看项目文件」：右侧停靠文件浏览面板 */
  "browse-project": [cwd: string];
  /** 项目右键「移除项目」：移除该项目全部会话的列表记录（已确认过） */
  "remove-project": [cwd: string];
  /** 会话右键「删除会话」：移除该会话的列表记录（已确认过） */
  "remove-session": [sessionId: string];
  /** 重命名会话（host 写注册表 name） */
  rename: [sessionId: string, name: string];
  "open-settings": [tab?: string];
  "open-tools": [tool: "plugins" | "skills" | "providers" | "mcp"];
  /** 打开自动化（定时任务）全屏页（桌面端） */
  "open-automation": [];
  /** 打开专家管理页（桌面端） */
  "open-experts": [];
}>();

const search = ref("");
const searchOpen = ref(false);
const searchInput = ref<HTMLInputElement | null>(null);
/** true = 按项目分组（「项目」页签）；false = 按时间平铺（「# 分组」页签） */
const grouped = ref(true);
const collapsed = ref(new Set<string>());
const expanded = ref(new Set<string>());
const PREVIEW = 5;

// ---- 右键菜单（会话行 / 项目行共用关闭机制与样式） ----
const revealPath = inject(REVEAL_PATH, null);
const ctxMenu = ref<{ x: number; y: number; s: SessionSummaryUi } | null>(null);
function openCtxMenu(e: MouseEvent, s: SessionSummaryUi): void {
  // 菜单尺寸约 224×370，贴边时向内收
  ctxMenu.value = {
    x: Math.min(e.clientX, window.innerWidth - 232),
    y: Math.min(e.clientY, window.innerHeight - 378),
    s,
  };
}
const projMenu = ref<{ x: number; y: number; g: Group } | null>(null);
function openProjMenu(e: MouseEvent, g: Group): void {
  // 菜单尺寸约 208×240，贴边时向内收
  projMenu.value = {
    x: Math.min(e.clientX, window.innerWidth - 216),
    y: Math.min(e.clientY, window.innerHeight - 250),
    g,
  };
}
function closeCtxMenu(): void {
  ctxMenu.value = null;
  projMenu.value = null;
}

// ---- 移除项目 / 删除会话（host 只删注册表条目，磁盘上的会话文件保留） ----
async function confirmRemoveProject(g: Group): Promise<void> {
  projMenu.value = null;
  const ok = await appConfirm({
    title: `移除项目「${g.project}」？`,
    message: `将把它的 ${g.sessions.length} 个会话从列表移除（不删除项目目录；磁盘上的会话记录文件保留，但界面中将无法再打开）。`,
    danger: true,
  });
  if (ok) emit("remove-project", g.cwd);
}
async function confirmRemoveSession(s: SessionSummaryUi): Promise<void> {
  ctxMenu.value = null;
  const ok = await appConfirm({
    title: `删除会话「${s.name || basename(s.cwd)}」？`,
    message: "将从列表移除该会话记录（磁盘上的会话文件保留）。",
    danger: true,
  });
  if (ok) emit("remove-session", s.session_id);
}

// ---- 置顶（UI 本地偏好，localStorage 持久化） ----
const PIN_KEY = "pidock.pinnedSessions";
const pinned = ref<Set<string>>(readPinned());
function readPinned(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(PIN_KEY) ?? "[]");
    return new Set(Array.isArray(raw) ? raw.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}
function isPinned(id: string): boolean {
  return pinned.value.has(id);
}
function togglePin(id: string): void {
  const next = new Set(pinned.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  pinned.value = next;
  try {
    localStorage.setItem(PIN_KEY, JSON.stringify([...next]));
  } catch {
    // localStorage 不可用时仅本次会话内生效
  }
  closeCtxMenu();
}
/** 置顶会话排在其所在列表的最前（保持组内时间序） */
function pinSort(list: SessionSummaryUi[]): SessionSummaryUi[] {
  return [
    ...list.filter((s) => pinned.value.has(s.session_id)),
    ...list.filter((s) => !pinned.value.has(s.session_id)),
  ];
}

// ---- 行内重命名 ----
const renaming = ref<string | null>(null);
const renameValue = ref("");
function startRename(s: SessionSummaryUi): void {
  renameValue.value = s.name ?? "";
  renaming.value = s.session_id;
  closeCtxMenu();
  requestAnimationFrame(() => {
    const el = document.querySelector<HTMLInputElement>("input.rename-input");
    el?.focus();
    el?.select();
  });
}
function commitRename(s: SessionSummaryUi): void {
  if (renaming.value !== s.session_id) return;
  renaming.value = null;
  const name = renameValue.value.trim();
  if (name && name !== (s.name ?? "")) emit("rename", s.session_id, name);
}
function cancelRename(): void {
  renaming.value = null;
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // WebView 剪贴板不可用时退回 execCommand
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
  closeCtxMenu();
}


const filtered = computed(() => {
  const q = search.value.trim().toLowerCase();
  const list = [...props.sessions].sort(
    (a, b) => ((b.updated_at ?? b.created_at) > (a.updated_at ?? a.created_at) ? 1 : -1),
  );
  if (!q) return pinSort(list);
  return pinSort(
    list.filter(
      (s) =>
        (s.name ?? "").toLowerCase().includes(q) ||
        s.cwd.toLowerCase().includes(q) ||
        (s.model ?? "").toLowerCase().includes(q),
    ),
  );
});

/** 会话是否「不在项目中」（cwd 为主目录或为空）；主目录未知（web 快照）时不做拆分 */
function isLooseTask(s: SessionSummaryUi): boolean {
  if (!props.homeDir) return false;
  return !s.cwd || s.cwd === props.homeDir;
}

/** 项目视图里「任务」分组：不在项目中的会话 */
const looseTasks = computed(() => filtered.value.filter((s) => isLooseTask(s)));
const tasksCollapsed = ref(false);
const projectsCollapsed = ref(false);

interface Group {
  project: string;
  cwd: string;
  sessions: SessionSummaryUi[];
}
const groups = computed<Group[]>(() => {
  const map = new Map<string, Group>();
  for (const s of filtered.value) {
    if (isLooseTask(s)) continue; // 不在项目中的会话挂到「任务」分组
    const project = basename(s.cwd) || "未分类";
    if (!map.has(project)) map.set(project, { project, cwd: s.cwd, sessions: [] });
    map.get(project)!.sessions.push(s);
  }
  return [...map.values()].sort((a, b) => {
    const la = a.sessions[0], lb = b.sessions[0];
    return ((lb?.updated_at ?? lb?.created_at ?? "") > (la?.updated_at ?? la?.created_at ?? "") ? 1 : -1);
  });
});
/** 项目区块会话总数（标题右侧计数） */
const projectCount = computed(() => groups.value.reduce((n, g) => n + g.sessions.length, 0));

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
  "responding", "thinking", "executing_tool", "compacting", "retrying", "waiting_approval", "waiting_ask", "running",
]);
const stateLabel: Record<string, string> = {
  idle: "空闲", thinking: "思考中", responding: "回复中", executing_tool: "执行工具",
  compacting: "压缩上下文", retrying: "重试中", running: "运行中",
  waiting_approval: "等待审批", waiting_ask: "等待回答", error: "出错", done: "完成",
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
  if (e.key === "Escape") {
    if (ctxMenu.value) {
      e.preventDefault();
      closeCtxMenu();
    }
    return;
  }
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

function onDocClick(): void {
  closeCtxMenu();
}

onMounted(() => {
  window.addEventListener("keydown", onKey);
  document.addEventListener("click", onDocClick);
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  document.removeEventListener("click", onDocClick);
});
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
      <div
        class="nav-item"
        :class="{ disabled: !showAutomation, active: activeTool === 'automation' }"
        :title="showAutomation ? undefined : '开发中'"
        @click="showAutomation && emit('open-automation')"
      >
        <Icon name="time-line" :size="16" />自动化
      </div>
      <div
        class="nav-item"
        :class="{ disabled: !showExperts, active: activeTool === 'experts' }"
        :title="showExperts ? undefined : '开发中'"
        @click="showExperts && emit('open-experts')"
      >
        <Icon name="user-star-line" :size="16" />专家
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
      <button class="seg-btn" :class="{ active: !grouped }" @click="grouped = false"># 分组</button>
      <button class="seg-btn" :class="{ active: grouped }" @click="grouped = true">
        <Icon name="folder-line" :size="14" />项目
      </button>
      <span class="flex-sp"></span>
    </div>

    <div class="sb-scroll">
      <!-- 项目分组视图：「项目」与「任务」两个并列区块 -->
      <template v-if="grouped">
        <template v-if="groups.length">
          <button class="section-head" @click="projectsCollapsed = !projectsCollapsed">
            <span>项目</span>
            <span class="g-count">{{ projectCount }}</span>
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: projectsCollapsed }" />
          </button>
          <template v-if="!projectsCollapsed">
            <template v-for="g in groups" :key="g.cwd + g.project">
              <div
                class="folder-row"
                :title="g.cwd"
                @click="emit('open-project', g.cwd)"
                @contextmenu.prevent="openProjMenu($event, g)"
              >
                <span
                  class="chev"
                  :class="{ fold: collapsed.has(g.project) }"
                  title="展开 / 折叠"
                  @click.stop="toggleGroup(g.project)"
                >
                  <Icon name="arrow-down-s-line" :size="12" />
                </span>
                <Icon :name="collapsed.has(g.project) ? 'folder-line' : 'folder-open-line'" :size="15" />
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
                @contextmenu.prevent="openCtxMenu($event, s)"
              >
                <span class="dot" :class="dotClass(s)"></span>
                <input
                  v-if="renaming === s.session_id"
                  v-model="renameValue"
                  class="rename-input"
                  @click.stop
                  @keydown.enter.prevent="commitRename(s)"
                  @keydown.esc.prevent="cancelRename"
                  @blur="commitRename(s)"
                />
                <span v-else class="txt">{{ s.name || basename(s.cwd) }}</span>
                <Icon
                  v-if="isPinned(s.session_id) && renaming !== s.session_id"
                  class="pin-badge"
                  name="pushpin-2-fill"
                  :size="12"
                />
                <span class="time">{{ relTime(s.updated_at ?? s.created_at) }}</span>
              </div>
                <div
                  v-if="g.sessions.length > PREVIEW"
                  class="show-more"
                  @click="toggleMore(g.project)"
                >{{ expanded.has(g.project) ? "收起" : "显示更多" }}</div>
              </template>
            </template>
          </template>
        </template>
        <template v-if="looseTasks.length">
          <button class="section-head" @click="tasksCollapsed = !tasksCollapsed">
            <span>任务</span>
            <span class="g-count">{{ looseTasks.length }}</span>
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: tasksCollapsed }" />
          </button>
          <template v-if="!tasksCollapsed">
            <div
              v-for="s in looseTasks"
              :key="s.session_id"
              class="task-row flat"
              :class="{ active: s.session_id === activeId }"
              :title="rowTitle(s)"
              @click="emit('select', s.session_id)"
              @contextmenu.prevent="openCtxMenu($event, s)"
            >
              <span class="dot" :class="dotClass(s)"></span>
              <input
                v-if="renaming === s.session_id"
                v-model="renameValue"
                class="rename-input"
                @click.stop
                @keydown.enter.prevent="commitRename(s)"
                @keydown.esc.prevent="cancelRename"
                @blur="commitRename(s)"
              />
              <span v-else class="txt">{{ s.name || basename(s.cwd) }}</span>
              <Icon
                v-if="isPinned(s.session_id) && renaming !== s.session_id"
                class="pin-badge"
                name="pushpin-2-fill"
                :size="12"
              />
              <span class="time">{{ relTime(s.updated_at ?? s.created_at) }}</span>
            </div>
          </template>
        </template>
        <div v-if="!groups.length && !looseTasks.length" class="empty">
          {{ search ? "没有匹配的会话" : "暂无会话，点上方「新建任务」" }}
        </div>
      </template>

      <!-- 任务视图：全部会话按时间平铺 -->
      <template v-else>
        <div class="sb-title">任务</div>
        <div
          v-for="s in filtered"
          :key="s.session_id"
          class="task-row flat"
          :class="{ active: s.session_id === activeId }"
          :title="rowTitle(s)"
          @click="emit('select', s.session_id)"
          @contextmenu.prevent="openCtxMenu($event, s)"
        >
          <span class="dot" :class="dotClass(s)"></span>
          <input
            v-if="renaming === s.session_id"
            v-model="renameValue"
            class="rename-input"
            @click.stop
            @keydown.enter.prevent="commitRename(s)"
            @keydown.esc.prevent="cancelRename"
            @blur="commitRename(s)"
          />
          <span v-else class="txt">{{ s.name || basename(s.cwd) }}</span>
          <Icon
            v-if="isPinned(s.session_id) && renaming !== s.session_id"
            class="pin-badge"
            name="pushpin-2-fill"
            :size="12"
          />
          <span class="time">{{ relTime(s.updated_at ?? s.created_at) }}</span>
        </div>
        <div v-if="!filtered.length" class="empty">
          {{ search ? "没有匹配的会话" : "暂无会话，点上方「新建任务」" }}
        </div>
      </template>
    </div>

    <!-- 会话右键菜单 -->
    <div
      v-if="ctxMenu"
      class="ctx-menu"
      :style="{ left: ctxMenu.x + 'px', top: ctxMenu.y + 'px' }"
      @click.stop
      @contextmenu.prevent
    >
      <button class="ctx-item" @click="togglePin(ctxMenu!.s.session_id)">
        {{ isPinned(ctxMenu!.s.session_id) ? "取消置顶" : "置顶任务" }}
      </button>
      <button class="ctx-item" @click="startRename(ctxMenu!.s)">重命名任务</button>
      <div class="ctx-sep"></div>
      <button
        v-if="revealPath && ctxMenu!.s.cwd"
        class="ctx-item"
        @click="revealPath(ctxMenu!.s.cwd); closeCtxMenu()"
      >在资源管理器中打开</button>
      <button v-if="ctxMenu!.s.cwd" class="ctx-item" @click="copyText(ctxMenu!.s.cwd)">复制项目路径</button>
      <button v-if="ctxMenu!.s.file" class="ctx-item" @click="copyText(ctxMenu!.s.file)">复制会话文件路径</button>
      <button class="ctx-item" @click="copyText(ctxMenu!.s.session_id)">复制会话 ID</button>
      <template v-if="showSettingsBtn !== false">
        <div class="ctx-sep"></div>
        <button class="ctx-item" @click="emit('open-settings'); closeCtxMenu()">前往配置</button>
      </template>
      <div class="ctx-sep"></div>
      <button class="ctx-item danger" @click="confirmRemoveSession(ctxMenu!.s)">删除会话</button>
    </div>

    <!-- 项目右键菜单 -->
    <div
      v-if="projMenu"
      class="ctx-menu"
      :style="{ left: projMenu.x + 'px', top: projMenu.y + 'px' }"
      @click.stop
      @contextmenu.prevent
    >
      <button class="ctx-item" @click="emit('open-project', projMenu!.g.cwd); closeCtxMenu()">新建任务</button>
      <button class="ctx-item" @click="emit('browse-project', projMenu!.g.cwd); closeCtxMenu()">查看项目文件</button>
      <div class="ctx-sep"></div>
      <button
        v-if="revealPath"
        class="ctx-item"
        @click="revealPath(projMenu!.g.cwd); closeCtxMenu()"
      >在资源管理器中打开</button>
      <button class="ctx-item" @click="copyText(projMenu!.g.cwd)">复制项目路径</button>
      <div class="ctx-sep"></div>
      <button class="ctx-item danger" @click="confirmRemoveProject(projMenu!.g)">移除项目</button>
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
      ><Icon name="settings-3-line" :size="17" /></button>
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
  font-size: calc(15px * var(--pd-font-scale));
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
  font-size: calc(13px * var(--pd-font-scale));
  user-select: none;
}
.nav-item:hover { background: var(--pd-bg-hover); }
.nav-item.active { background: var(--pd-bg-active); color: var(--pd-text); }
.nav-item.disabled { opacity: 0.45; pointer-events: none; }
.nav-item .kbd {
  margin-left: auto;
  font-size: calc(11px * var(--pd-font-scale));
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
  font-size: calc(12px * var(--pd-font-scale));
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
  font-size: calc(12px * var(--pd-font-scale));
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
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
  padding: 12px 10px 6px;
}
/* 区块标题（与「项目」并列的「任务」区）：可点击折叠 */
.section-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  background: none;
  border: none;
  padding: 12px 10px 6px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
  text-align: left;
  user-select: none;
}
.section-head:hover { color: var(--pd-text-2); }
.section-head .g-count { margin-left: auto; }
.section-head svg { color: var(--pd-text-4); transition: transform 0.12s; }
.section-head svg.fold { transform: rotate(-90deg); }
.folder-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 8px;
  color: var(--pd-text-2);
  cursor: pointer;
  font-size: calc(13px * var(--pd-font-scale));
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
  font-size: calc(10.5px * var(--pd-font-scale));
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
  font-size: calc(13px * var(--pd-font-scale));
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
  font-size: calc(11px * var(--pd-font-scale));
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
.pin-badge { color: var(--pd-accent); flex: none; }
.rename-input {
  flex: 1;
  min-width: 0;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-accent);
  border-radius: 5px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  font-family: inherit;
  padding: 1px 6px;
  outline: none;
}

/* ---- 会话右键菜单 ---- */
.ctx-menu {
  position: fixed;
  min-width: 176px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  padding: 5px;
  box-shadow: var(--pd-shadow);
  z-index: 100;
}
.ctx-item {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  border-radius: 7px;
  padding: 7px 10px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
}
.ctx-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.ctx-item.danger { color: var(--pd-red); }
.ctx-item.danger:hover { background: var(--pd-bg-hover); color: var(--pd-red); }
.ctx-sep { height: 1px; background: var(--pd-border-soft); margin: 4px 6px; }
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
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
  user-select: none;
}
.show-more:hover { color: var(--pd-text-3); }
.empty {
  padding: 12px 10px;
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
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
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 700;
}
.uname { font-size: calc(13px * var(--pd-font-scale)); color: var(--pd-text-2); }
</style>
