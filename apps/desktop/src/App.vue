<script setup lang="ts">
import { computed, onMounted, provide, ref, watch } from "vue";
import { darkTheme, NConfigProvider, NSplit } from "naive-ui";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ChatView,
  ProvidersView,
  SessionSidebar,
  SettingsView,
  StatePill,
  ToolsView,
  ATTACHMENT_LOADER,
  FOLDER_PICKER,
  REVEAL_PATH,
  createAgentStore,
  initTheme,
  themeMode,
  toggleTheme,
  type AgentStore,
} from "@pidock/ui";
import { createTauriBus } from "./bus";
import SyncPanel from "./SyncPanel.vue";

initTheme();
const naiveTheme = computed(() => (themeMode.value === "dark" ? darkTheme : undefined));

const store = ref<AgentStore | null>(null);
const bootError = ref<string | null>(null);
const showSettings = ref(false);
const settingsTab = ref<"models" | "extensions" | "skills" | "mcp">("models");
/** main-area view: chat by default, tool managers when the sidebar nav is clicked */
const mainView = ref<"chat" | "plugins" | "skills" | "providers" | "mcp">("chat");
/** 新建任务模式：右侧显示默认对话页，发送首条消息后自动创建会话并退出该模式 */
const newTaskMode = ref(false);
/** 新建任务预选的项目目录（侧栏项目分组点击）；seq 自增让重复点击同一目录也能重新应用选中 */
const newTaskCwd = ref<{ cwd: string; seq: number } | null>(null);
const bus = createTauriBus();

function startNewTask(cwd?: string): void {
  mainView.value = "chat";
  newTaskMode.value = true;
  newTaskCwd.value = cwd ? { cwd, seq: (newTaskCwd.value?.seq ?? 0) + 1 } : null;
}

// ---- 左栏（会话侧栏）拖拽调宽（NSplit），宽度记忆在 localStorage ----
const SIDEBAR_KEY = "pidock.sidebarWidth";
const sidebarWidth = ref<number | null>(readSidebarWidth());
function readSidebarWidth(): number | null {
  const v = Number(localStorage.getItem(SIDEBAR_KEY));
  return Number.isFinite(v) && v >= 190 && v <= 520 ? Math.round(v) : null;
}
function onSidebarSize(s: string | number): void {
  // size 始终传 px 字符串，回调回来的也是 px 字符串
  const px = typeof s === "string" ? parseFloat(s) : NaN;
  if (Number.isFinite(px)) sidebarWidth.value = Math.round(Math.min(520, Math.max(190, px)));
}
function saveSidebar(): void {
  try {
    if (sidebarWidth.value != null) localStorage.setItem(SIDEBAR_KEY, String(sidebarWidth.value));
  } catch {
    // localStorage 不可用时忽略
  }
}

provide(ATTACHMENT_LOADER, (id: string) => bus.loadAttachment(id));

// 系统原生目录选择器（输入卡片项目菜单的「打开文件夹」）
provide(FOLDER_PICKER, async () => {
  try {
    return await invoke<string | null>("pick_folder");
  } catch {
    return null;
  }
});

// 在系统文件管理器中打开目录（会话右键菜单）
provide(REVEAL_PATH, async (path: string) => {
  try {
    await invoke("reveal_path", { path });
  } catch {
    // 打开失败时静默（如路径已不存在）
  }
});

// custom frameless-window titlebar controls
const appWin = (() => {
  try {
    return getCurrentWindow();
  } catch {
    return null; // running in a plain browser (vite dev without tauri)
  }
})();
const isMax = ref(false);
async function refreshMax(): Promise<void> {
  if (!appWin) return;
  try {
    isMax.value = await appWin.isMaximized();
  } catch {
    /* not in tauri */
  }
}
function minimize(): void {
  void appWin?.minimize();
}
function toggleMaximize(): void {
  void appWin?.toggleMaximize();
}
function closeWindow(): void {
  void appWin?.close();
}

onMounted(async () => {
  // 会话打开（新建/点选）后退出新建任务模式
  watch(
    () => store.value?.activeId,
    (v) => {
      if (v) newTaskMode.value = false;
    },
  );

  void refreshMax();
  if (appWin) {
    try {
      await appWin.onResized(() => void refreshMax());
    } catch {
      /* not in tauri */
    }
  }
  try {
    const s = createAgentStore(bus);
    await s.start();
    store.value = s;

    // auto-refresh the session list when an unknown session shows up
    let pending = false;
    void bus.onEvent((e) => {
      if (!e.session_id || pending) return;
      const known = s.sessions.some((x) => x.session_id === e.session_id);
      if (!known) {
        pending = true;
        setTimeout(() => {
          pending = false;
          void s.refreshSessions();
        }, 800);
      }
    });
    setInterval(() => void s.refreshSessions(), 30000);
  } catch (err) {
    bootError.value = String(err);
  }
});
</script>

<template>
  <n-config-provider :theme="naiveTheme">
    <div v-if="bootError" class="boot-error">
      <h2>pi-host 启动失败</h2>
      <pre>{{ bootError }}</pre>
      <p>检查 PIDOCK_HOST_CMD / PIDOCK_HOST_DIR 环境变量，或先运行 bun build 编译 host。</p>
    </div>
    <div v-else-if="store" class="layout">
      <n-split
        direction="horizontal"
        class="layout-split"
        :size="`${sidebarWidth ?? 260}px`"
        min="190px"
        max="520px"
        :resize-trigger-size="6"
        :pane1-style="{ display: 'flex' }"
        :pane2-style="{ flex: '1 1 0', minWidth: 0, display: 'flex' }"
        @update:size="onSidebarSize"
        @drag-end="saveSidebar"
      >
        <template #1>
      <SessionSidebar
        :sessions="store.sessions"
        :active-id="store.activeId"
        :home-dir="store.homeDir"
        :show-tool-nav="true"
        :show-settings-btn="true"
        :active-tool="mainView === 'chat' ? undefined : mainView"
        :width="sidebarWidth ?? undefined"
        @select="(id) => { mainView = 'chat'; store?.openSession(id); }"
        @new-task="() => startNewTask()"
        @open-project="(cwd) => startNewTask(cwd)"
        @rename="(id, name) => store?.renameSession(id, name)"
        @open-tools="(t) => (mainView = t)"
        @open-settings="(tab) => { settingsTab = (tab as any) ?? 'models'; showSettings = true; }"
      >
        <template #bottom><SyncPanel /></template>
      </SessionSidebar>
      </template>
        <template #resize-trigger><div class="rz-line" /></template>
        <template #2>
      <main class="main">
        <header class="titlebar" data-tauri-drag-region>
          <StatePill :state="store.agentState" />
          <span class="transport">本地 pi-host</span>
          <span v-if="store.lastError" class="err" :title="store.lastError">{{ store.lastError }}</span>
          <span class="flex-sp"></span>
          <button
            class="tbtn"
            :title="themeMode === 'dark' ? '切换亮色' : '切换暗色'"
            @click="toggleTheme()"
          >{{ themeMode === "dark" ? "☀" : "☾" }}</button>
          <span class="win-sep"></span>
          <button class="tbtn" title="最小化" @click="minimize">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 12h14" /></svg>
          </button>
          <button class="tbtn" :title="isMax ? '还原' : '最大化'" @click="toggleMaximize">
            <svg v-if="isMax" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M9 9h10v10H9z" /><path d="M5 15V5h10" /></svg>
            <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M6 6h12v12H6z" /></svg>
          </button>
          <button class="tbtn close" title="关闭" @click="closeWindow">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18" /></svg>
          </button>
        </header>
        <ChatView
          v-if="mainView === 'chat'"
          :store="store"
          :new-task="newTaskMode"
          :new-task-cwd="newTaskCwd"
          :model="store.sessions.find((s) => s.session_id === store?.activeId)?.model"
          @open-settings="(tab) => { settingsTab = (tab as any) ?? 'models'; showSettings = true; }"
          @open-providers="() => { mainView = 'providers'; }"
        />
        <ProvidersView v-else-if="mainView === 'providers'" :bus="bus" />
        <ToolsView v-else :kind="mainView" :bus="bus" />
        <SettingsView
          v-if="showSettings"
          :bus="bus"
          :initial-tab="settingsTab"
          @close="showSettings = false"
        />
      </main>
        </template>
      </n-split>
    </div>
    <div v-else class="boot-error">正在启动 pi-host…</div>
  </n-config-provider>
</template>

<style scoped>
.layout {
  display: flex;
  height: 100vh;
}
.layout-split {
  flex: 1;
  min-width: 0;
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
.main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  position: relative;
  background: var(--pd-bg);
}
.flex-sp { flex: 1; }
.titlebar {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 42px;
  padding: 0 8px 0 14px;
  background: var(--pd-bg-panel);
  border-bottom: 1px solid var(--pd-border-soft);
  user-select: none;
  flex: none;
}
.transport { font-size: 12px; color: var(--pd-text-4); }
.err {
  font-size: 12px;
  color: var(--pd-red);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 40%;
}
.tbtn {
  width: 34px;
  height: 30px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-3);
  cursor: pointer;
  padding: 0;
}
.tbtn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.tbtn.close:hover { background: #e04444; color: #fff; }
.win-sep {
  width: 1px;
  height: 16px;
  background: var(--pd-border);
  margin: 0 2px;
}
.boot-error {
  padding: 40px;
  color: var(--pd-red);
}
.boot-error pre { color: var(--pd-text-2); white-space: pre-wrap; }
</style>
