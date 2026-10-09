<script setup lang="ts">
import { computed, onMounted, provide, ref, watch } from "vue";
import { darkTheme, NConfigProvider, NSplit } from "naive-ui";
import { getCurrentWindow, currentMonitor, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import {
  AutomationView,
  ChatView,
  ExpertsView,
  FolderBrowser,
  Icon,
  SessionSidebar,
  SettingsView,
  StatePill,
  ATTACHMENT_LOADER,
  FOLDER_PICKER,
  FILE_PICKER,
  REVEAL_PATH,
  GIT_API,
  FS_LIST,
  WINDOW_CONTROLS,
  IS_MAC,
  installTauriDblclickGuard,
  onTitlebarDblclick,
  createAgentStore,
  initTheme,
  themeMode,
  toggleTheme,
  type AgentStore,
} from "@pidock/ui";
import { createBus } from "./bus";
import { ipc } from "./ipc";
import SyncSettings from "./SyncSettings.vue";
import DesktopSettings from "./DesktopSettings.vue";

initTheme();
const naiveTheme = computed(() => (themeMode.value === "dark" ? darkTheme : undefined));

const store = ref<AgentStore | null>(null);
const bootError = ref<string | null>(null);
const showSettings = ref(false);
/** 设置中心打开时定位的页面（接受新 pane id 与旧 tab 名） */
const settingsPane = ref("general");
/** 自动化（定时任务）全屏页 */
const showAutomation = ref(false);
/** 专家管理页（与聊天共用主区，同自动化模式） */
const showExperts = ref(false);
/** 新建任务模式：右侧显示默认对话页，发送首条消息后自动创建会话并退出该模式 */
const newTaskMode = ref(false);
/** 新任务预雇佣的专家（专家页「雇佣」进入） */
const newTaskExpert = ref<{ id: string; name: string } | null>(null);
/** 新建任务预选的项目目录（侧栏项目分组点击）；seq 自增让重复点击同一目录也能重新应用选中 */
const newTaskCwd = ref<{ cwd: string; seq: number } | null>(null);
/** 项目文件浏览面板（侧栏项目右键「查看项目文件」）；seq 自增支持同项目重复触发刷新 */
const browseCwd = ref<{ cwd: string; seq: number } | null>(null);
const bus = createBus();

function startNewTask(cwd?: string, expert?: { id: string; name: string } | null): void {
  newTaskMode.value = true;
  newTaskExpert.value = expert ?? null;
  newTaskCwd.value = cwd ? { cwd, seq: (newTaskCwd.value?.seq ?? 0) + 1 } : null;
}

/** 移除项目：把该 cwd 下全部会话从列表移除（注册表条目；磁盘文件保留，确认弹窗在侧栏） */
function removeProject(cwd: string): void {
  const s = store.value;
  if (!s) return;
  void s.removeSessions(s.sessions.filter((x) => x.cwd === cwd).map((x) => x.session_id));
}

/** 专家页「对话」：立即创建绑定该专家的会话并进入聊天。
 * 工作目录不传（host 落主目录）——测试环境纯净：只有全局资源 + 专家配置。
 * 配置生效性已在 SDK 层验证（scripts/experts-config-verify.ts）：
 * 角色提示词进系统提示词、excludeTools/技能过滤均真实生效。 */
async function testExpert(e: { id: string; name: string }): Promise<void> {
  showExperts.value = false;
  showAutomation.value = false;
  const s = store.value;
  if (!s) return;
  try {
    await s.newSession(undefined, undefined, e.id);
  } catch (err) {
    // 标题栏已有 lastError 展示通道，这里不吞异常即可
    s.lastError = err instanceof Error ? err.message : String(err);
  }
}

function openSettings(tab?: string): void {
  settingsPane.value = tab ?? "general";
  showSettings.value = true;
}

// Ctrl/Cmd+N 新建任务（设置中心快捷键页展示的绑定之一）
function onGlobalKey(e: KeyboardEvent): void {
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
    e.preventDefault();
    startNewTask();
  }
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
/** 设置中心「恢复默认布局」：清掉宽度记忆，回到 260px */
function resetLayout(): void {
  sidebarWidth.value = null;
  try {
    localStorage.removeItem(SIDEBAR_KEY);
  } catch {
    // ignore
  }
}

provide(ATTACHMENT_LOADER, (id: string) => bus.loadAttachment(id));

// git 分支信息与切换（输入卡片项目 chip 旁的分支选择器）；CoreCtx 命令面，webhost 同样支持
provide(GIT_API, {
  info: (cwd: string) => ipc("git_info", { cwd }),
  branches: (cwd: string) => ipc("git_branches", { cwd }),
  checkout: (cwd: string, branch: string, create?: boolean) => ipc("git_checkout", { cwd, branch, create: create ?? false }),
});

// 服务端目录列举：「打开文件夹」与文件选择弹层的数据源（桌面与 webhost 统一，替代系统对话框）
provide(FS_LIST, (path?: string) => ipc("fs_list", { path: path ?? null }));

// 「打开文件夹」与文件选择（头像/技能包/插件导入）统一走自绘 FolderBrowser 弹层：
// 点行进入子目录、路径栏可跳转，dir 模式「选择当前目录」/ file 模式「选择此文件」确认，
// Esc·取消返回 null。桌面与 web 模式行为一致（webhost 下 fs_list 即节点机目录）。
const folderPick = ref<null | {
  resolve: (p: string | null) => void;
  mode: "dir" | "file";
  accept?: string[];
}>(null);
provide(FOLDER_PICKER, () => openFolderPick("dir"));
const IMAGE_ACCEPT = [".png", ".jpg", ".jpeg", ".webp", ".gif"];
const INSTALL_ACCEPT = [".zip", ".tgz", ".tar.gz", ".gz", ".ts", ".js"];
provide(FILE_PICKER, (kind?: "install" | "image") =>
  openFolderPick("file", kind === "image" ? IMAGE_ACCEPT : INSTALL_ACCEPT),
);
function openFolderPick(mode: "dir" | "file", accept?: string[]): Promise<string | null> {
  return new Promise((resolve) => {
    folderPick.value?.resolve(null); // 上一次未收尾的选择请求按取消处理
    folderPick.value = { resolve, mode, accept };
  });
}
function onFolderPicked(p: string): void {
  folderPick.value?.resolve(p);
  folderPick.value = null;
}
function onFolderPickCancel(): void {
  folderPick.value?.resolve(null);
  folderPick.value = null;
}

// 在系统文件管理器中打开目录（会话右键菜单）
provide(REVEAL_PATH, async (path: string) => {
  try {
    await ipc("reveal_path", { path });
  } catch {
    // 打开失败时静默（如路径已不存在 / webhost 模式不支持）
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
const isFull = ref(false);
async function refreshWinState(): Promise<void> {
  if (!appWin) return;
  try {
    isMax.value = await appWin.isMaximized();
    isFull.value = await appWin.isFullscreen();
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
function toggleFullscreen(): void {
  void appWin?.setFullscreen(!isFull.value);
}
/** 绿灯菜单「移到半屏」：按显示器工作区（去掉 Dock/菜单栏）平铺到左/右一半 */
async function tileWindow(side: "left" | "right"): Promise<void> {
  if (!appWin) return;
  try {
    if (isFull.value) await appWin.setFullscreen(false);
    const mon = await currentMonitor();
    if (!mon) return;
    const wa = mon.workArea;
    const half = Math.floor(wa.size.width / 2);
    const x = side === "left" ? wa.position.x : wa.position.x + half;
    await appWin.setPosition(new PhysicalPosition(x, wa.position.y));
    await appWin.setSize(new PhysicalSize(half, wa.size.height));
  } catch {
    /* 显示器信息拿不到等瞬态，忽略 */
  }
}
function closeWindow(): void {
  void appWin?.close();
}

// 无边框窗口控制：设置中心等全屏页面盖住标题栏时，用它补齐窗口按钮
if (appWin) {
  provide(WINDOW_CONTROLS, {
    minimize,
    toggleMaximize,
    toggleFullscreen,
    tile: (side) => void tileWindow(side),
    close: closeWindow,
    isMax,
    isFullscreen: isFull,
  });
}

onMounted(async () => {
  window.addEventListener("keydown", onGlobalKey);
  installTauriDblclickGuard();
  // 新建会话（首条消息创建）后退出新建任务模式；侧栏点选在 @select 里直接退出
  // ——重选同一会话时 activeId 不变，watch 不会触发
  watch(
    () => store.value?.activeId,
    (v) => {
      if (v) newTaskMode.value = false;
    },
  );

  void refreshWinState();
  if (appWin) {
    try {
      await appWin.onResized(() => void refreshWinState());
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
        :show-settings-btn="true"
        :show-automation="true"
        :show-experts="true"
        :active-tool="showAutomation ? 'automation' : showExperts ? 'experts' : undefined"
        :width="sidebarWidth ?? undefined"
        @select="(id) => { showAutomation = false; showExperts = false; newTaskMode = false; store?.openSession(id); }"
        @new-task="() => { showAutomation = false; showExperts = false; startNewTask(); }"
        @open-project="(cwd) => { showAutomation = false; showExperts = false; startNewTask(cwd); }"
        @browse-project="(cwd) => { showAutomation = false; showExperts = false; browseCwd = { cwd, seq: (browseCwd?.seq ?? 0) + 1 }; }"
        @remove-project="removeProject"
        @remove-session="(id) => store?.removeSessions([id])"
        @rename="(id, name) => store?.renameSession(id, name)"
        @open-settings="(tab) => openSettings(tab)"
        @open-automation="() => { showAutomation = !showAutomation; if (showAutomation) showExperts = false; }"
        @open-experts="() => { showExperts = !showExperts; if (showExperts) showAutomation = false; }"
      >
      </SessionSidebar>
      </template>
        <template #resize-trigger><div class="rz-line" /></template>
        <template #2>
      <main class="main">
        <!-- deep：整条子树可拖拽；双击最大化走 onTitlebarDblclick（内建脚本被守卫拦截） -->
        <header class="titlebar" data-tauri-drag-region="deep" @dblclick="onTitlebarDblclick($event, toggleMaximize)">
          <StatePill :state="store.agentState" />
          <span class="transport">{{ bus.transport }}</span>
          <span v-if="store.lastError" class="err" :title="store.lastError">{{ store.lastError }}</span>
          <span class="flex-sp"></span>
          <button
            class="tbtn"
            :title="themeMode === 'dark' ? '切换亮色' : '切换暗色'"
            @click="toggleTheme()"
          >
            <Icon :name="themeMode === 'dark' ? 'sun-line' : 'moon-line'" :size="15" />
          </button>
          <!-- Windows/Linux 方块窗口按钮：只在 Tauri 壳里渲染；浏览器里不显示 -->
          <template v-if="appWin && !IS_MAC">
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
          </template>
        </header>
        <!-- 自动化/专家页与聊天共用主区：侧栏保留；ChatView 用 v-show 保活，切回来不丢草稿 -->
        <ChatView
          v-show="!showAutomation && !showExperts"
          :store="store"
          :new-task="newTaskMode"
          :new-task-cwd="newTaskCwd"
          :new-task-expert="newTaskExpert"
          :files-cwd="browseCwd"
          @close-files="browseCwd = null"
          :model="store.sessions.find((s) => s.session_id === store?.activeId)?.model"
          @open-settings="(tab) => openSettings(tab)"
          @open-providers="() => openSettings('providers')"
        />
        <AutomationView
          v-if="showAutomation"
          :bus="bus"
          @close="showAutomation = false"
          @open-providers="() => openSettings('providers')"
        />
        <ExpertsView
          v-if="showExperts"
          :bus="bus"
          @close="showExperts = false"
          @open-providers="() => openSettings('providers')"
          @hire="(expert) => { showExperts = false; startNewTask(undefined, expert); }"
          @test="testExpert"
        />
        <SettingsView
          v-if="showSettings"
          :bus="bus"
          :initial-pane="settingsPane"
          :extra-panes="[{ id: 'desktop', label: '桌面', icon: 'computer-line' }]"
          @close="showSettings = false"
          @reset-layout="resetLayout"
        >
          <template #sync><SyncSettings /></template>
          <template #pane-desktop><DesktopSettings /></template>
        </SettingsView>
      </main>
        </template>
      </n-split>
    </div>
    <div v-else class="boot-splash">
      <div class="splash-mark">π</div>
      <div class="splash-line">
        <Icon name="loader-2-line" :size="15" class="splash-spin" />
        <span>正在启动 pi-host…</span>
      </div>
    </div>
    <FolderBrowser
      v-if="folderPick"
      :mode="folderPick.mode"
      :accept="folderPick.accept"
      @pick="onFolderPicked"
      @close="onFolderPickCancel"
    />
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
  -webkit-user-select: none; /* 旧 WKWebView 只认前缀写法，缺失时双击标题栏会选中文字 */
  flex: none;
}
.transport { font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-4); }
.err {
  font-size: calc(12px * var(--pd-font-scale));
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

/* 启动过渡页：居中 π 水印 + 旋转指示，与错误态（.boot-error 红色）区分 */
.boot-splash {
  height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 20px;
  background: var(--pd-bg);
  user-select: none;
}
.splash-mark {
  font-size: 72px;
  line-height: 1;
  font-family: Georgia, "Times New Roman", serif;
  font-style: italic;
  color: var(--pd-text-3);
  opacity: 0.3;
}
.splash-line {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
}
.splash-line svg {
  display: block;
  animation: splash-spin 0.9s linear infinite;
}
@keyframes splash-spin {
  to { transform: rotate(360deg); }
}
</style>
