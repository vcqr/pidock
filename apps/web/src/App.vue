<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, provide, ref, watch } from "vue";
import { darkTheme, NConfigProvider } from "naive-ui";
import {
  appConfirm,
  AutomationView,
  ChatView,
  ExpertsView,
  Icon,
  SessionSidebar,
  SettingsView,
  StatePill,
  ATTACHMENT_LOADER,
  createAgentStore,
  initTheme,
  themeMode,
  toggleTheme,
  type AgentStore,
} from "@pidock/ui";
import { ApiError, AuthClient, loadAuth, clearAuth } from "./auth.js";
import { createWebBus, type WebBus } from "./bus.js";
import AdminInvites from "./components/AdminInvites.vue";
import MachineCard from "./components/MachineCard.vue";
import MachinePopover from "./components/MachinePopover.vue";
import { osMeta, type MachineUi } from "./machine.js";

initTheme();
const naiveTheme = computed(() => (themeMode.value === "dark" ? darkTheme : undefined));

const auth = ref<AuthClient | null>(null);
const bus = ref<WebBus | null>(null);
const store = ref<AgentStore | null>(null);
const bootError = ref<string | null>(null);

// login form
const serverUrl = ref("http://localhost:8080");
const email = ref("");
const password = ref("");
const inviteCode = ref("");
const loginBusy = ref(false);
const loginError = ref<string | null>(null);
const registerMode = ref(false);
/** 邀请码管理面板（仅管理员可见入口） */
const showInvites = ref(false);

/** 首页节点搜索过滤（主机名/系统/版本/ID），Ctrl+K 聚焦 */
const nodeFilter = ref("");
const searchInput = ref<HTMLInputElement | null>(null);
const filteredMachines = computed(() => {
  const q = nodeFilter.value.trim().toLowerCase();
  if (!q) return machines.value;
  return machines.value.filter(
    (m) =>
      [m.hostname, m.machine_id, m.os, m.version ?? ""].some((v) => v.toLowerCase().includes(q)) ||
      osMeta(m.os).name.toLowerCase().includes(q),
  );
});

function onGlobalKey(e: KeyboardEvent): void {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k" && view.value === "home") {
    e.preventDefault();
    searchInput.value?.focus();
  }
}

// machines（类型与 OS 元数据见 machine.ts）
const machines = ref<MachineUi[]>([]);
const activeMachineId = ref<string | null>(null);
const activeMachine = computed(() => machines.value.find((m) => m.machine_id === activeMachineId.value));
const activeOsMeta = computed(() => osMeta(activeMachine.value?.os));
/** 每台机器的会话数（首页卡片与机器详情浮层用），null=尚未取到 */
const sessionCounts = ref<Record<string, number | null>>({});
/** home=机器卡片墙；ws=会话工作区（登录后先见 home） */
const view = ref<"home" | "ws">("home");
const showMachineInfo = ref(false);

async function refreshMachines(): Promise<void> {
  if (!auth.value || !bus.value) return;
  try {
    const r = await auth.value.request(`/machines?token=${auth.value.token}`);
    machines.value = r.machines ?? [];
    if (!activeMachineId.value) {
      const firstOnline = machines.value.find((m) => m.online) ?? machines.value[0];
      if (firstOnline) selectMachine(firstOnline.machine_id);
    }
  } catch (err) {
    if (err instanceof ApiError && err.status === 401) return; // auth layer handles logout
    console.warn("machines refresh failed", err);
  }
}

async function deleteMachine(m: MachineUi): Promise<void> {
  if (!auth.value) return;
  if (!(await appConfirm({ title: `删除节点「${m.hostname || m.machine_id.slice(0, 8)}」？`, message: "将删除该节点及其全部同步数据。", danger: true }))) return;
  try {
    await auth.value.request(`/machines/${m.machine_id}?token=${auth.value.token}`, { method: "DELETE" });
    if (activeMachineId.value === m.machine_id) {
      activeMachineId.value = null;
      if (bus.value) bus.value.setMachine("");
    }
    await refreshMachines();
  } catch (err) {
    console.warn("delete machine failed", err);
  }
}

async function selectMachine(machineId: string): Promise<void> {
  activeMachineId.value = machineId;
  bus.value?.setMachine(machineId);
  await store.value?.refreshSessions();
}

/** 拉取每台机器的会话数（首页卡片/详情浮层），单台失败保留旧值 */
async function refreshSessionCounts(): Promise<void> {
  if (!auth.value) return;
  await Promise.all(
    machines.value.map(async (m) => {
      try {
        const r = await auth.value!.request(`/machines/${m.machine_id}/sessions?token=${auth.value!.token}`);
        sessionCounts.value[m.machine_id] = (r.sessions ?? []).length;
      } catch {
        /* 保留上次计数 */
      }
    }),
  );
}

/** 首页卡片点击：选中机器并进入工作区 */
async function enterMachine(machineId: string): Promise<void> {
  await selectMachine(machineId);
  showMachineInfo.value = false;
  view.value = "ws";
}

/** 返回机器卡片墙 */
function goHome(): void {
  showMachineInfo.value = false;
  view.value = "home";
  void refreshMachines();
  void refreshSessionCounts();
}

/** 项目文件浏览面板（侧栏项目右键「查看项目文件」）；seq 自增支持同项目重复触发刷新 */
const browseCwd = ref<{ cwd: string; seq: number } | null>(null);

/** 移除项目：把该 cwd 下全部会话从列表移除（注册表条目；磁盘文件保留，确认弹窗在侧栏） */
function removeProject(cwd: string): void {
  const s = store.value;
  if (!s) return;
  void s.removeSessions(s.sessions.filter((x) => x.cwd === cwd).map((x) => x.session_id));
}

// ---- 页面模式（与桌面 App 同构）：自动化/专家页与聊天共用主区 ----
const showSettings = ref(false);
/** 设置中心打开时定位的页面（接受新 pane id 与旧 tab 名） */
const settingsPane = ref("general");
const showAutomation = ref(false);
const showExperts = ref(false);
/** 新建任务模式：发送首条消息后自动创建会话并退出该模式 */
const newTaskMode = ref(false);
/** 新任务预雇佣的专家（专家页「雇佣」进入） */
const newTaskExpert = ref<{ id: string; name: string } | null>(null);
/** 新建任务预选的项目目录（侧栏项目分组点击） */
const newTaskCwd = ref<{ cwd: string; seq: number } | null>(null);

function startNewTask(cwd?: string, expert?: { id: string; name: string } | null): void {
  newTaskMode.value = true;
  newTaskExpert.value = expert ?? null;
  newTaskCwd.value = cwd ? { cwd, seq: (newTaskCwd.value?.seq ?? 0) + 1 } : null;
}

function openSettings(tab?: string): void {
  settingsPane.value = tab ?? "general";
  showSettings.value = true;
}

/** 专家页「对话」：立即创建绑定该专家的会话并进入聊天（host 落主目录，测试环境纯净） */
async function testExpert(e: { id: string; name: string }): Promise<void> {
  showExperts.value = false;
  showAutomation.value = false;
  const s = store.value;
  if (!s) return;
  try {
    await s.newSession(undefined, undefined, e.id);
  } catch (err) {
    s.lastError = err instanceof Error ? err.message : String(err);
  }
}

// 会话打开（新建/点选）后退出新建任务模式
watch(
  () => store.value?.activeId,
  (v) => {
    if (v) newTaskMode.value = false;
  },
);

async function doLogin(): Promise<void> {
  loginBusy.value = true;
  loginError.value = null;
  try {
    const client = new AuthClient(null as never, () => logout());
    if (registerMode.value) {
      await client.register(serverUrl.value.trim(), email.value.trim(), password.value, inviteCode.value);
    }
    const state = await client.login(serverUrl.value.trim(), email.value.trim(), password.value);
    boot(state, client);
  } catch (err) {
    loginError.value = String(err instanceof ApiError ? err.message : err);
  } finally {
    loginBusy.value = false;
  }
}

function logout(): void {
  clearAuth();
  auth.value = null;
  bus.value = null;
  store.value = null;
  machines.value = [];
  activeMachineId.value = null;
  sessionCounts.value = {};
  showMachineInfo.value = false;
  showInvites.value = false;
  view.value = "home";
}

// provide must happen at setup level; the bus may attach later
provide(ATTACHMENT_LOADER, (id: string) => {
  const b = bus.value;
  if (!b) return Promise.reject(new Error("not connected"));
  return b.loadAttachment(id);
});

function boot(state: any, client: AuthClient): void {
  auth.value = client;
  const b = createWebBus(client);
  bus.value = b;
  const s = createAgentStore(b);
  store.value = s;
  void s.start().then(async () => {
    b.onMachineStatus((machineId, status) => {
      const m = machines.value.find((x) => x.machine_id === machineId);
      if (m) m.online = status === "online";
      else void refreshMachines();
      void refreshSessionCounts();
      if (machineId === activeMachineId.value) {
        // online/offline affects composer state; refresh session statuses too
        void s.refreshSessions();
      }
    });
    await refreshMachines();
    void refreshSessionCounts();
    // periodic machines refresh for liveness
    setInterval(refreshMachines, 10000);

    // session list is push-driven: the server publishes a session snapshot on
    // every ingest upsert; only a slow sweep remains as a safety net
    b.onSessionUpserted((machineId, session) => {
      s.upsertSessionSummary({
        session_id: session.session_id,
        cwd: session.cwd || "",
        name: session.title || undefined,
        model: session.model || undefined,
        expert_id: session.expert_id || undefined,
        expert_name: session.expert_name || undefined,
        parent_session_id: session.parent_session_id || undefined,
        created_at: session.created_at || new Date().toISOString(),
        state: session.status === "running" ? "responding" : (session.status || "idle"),
        open: true,
      });
      if (machineId !== activeMachineId.value) void refreshMachines();
    });
    // WS（重）连成功即对账：机器在线状态与会话列表不用等 10s/30s 轮询兜底
    b.onReconnect(() => {
      void refreshMachines();
      void refreshSessionCounts();
      void s.refreshSessions();
    });
    setInterval(() => void s.refreshSessions(), 30000);
  });
}

onMounted(() => {
  window.addEventListener("keydown", onGlobalKey);
  const saved = loadAuth();
  if (saved) {
    const client = new AuthClient(saved, () => logout());
    boot(saved, client);
  }
});

onBeforeUnmount(() => window.removeEventListener("keydown", onGlobalKey));

const sessionsEmpty = computed(() => {
  const items = store.value?.sessions ?? [];
  return items.length === 0 && activeMachineId.value !== null;
});
</script>

<template>
  <n-config-provider :theme="naiveTheme">
  <!-- login -->
  <div v-if="!auth" class="login-screen">
    <form class="login-card" @submit.prevent="doLogin">
      <div class="brand"><span class="logo">π</span> PiDock Web</div>
      <label>Server<input v-model="serverUrl" /></label>
      <label>邮箱<input v-model="email" type="email" autocomplete="username" /></label>
      <label>密码<input v-model="password" type="password" autocomplete="current-password" /></label>
      <label v-if="registerMode"
        >邀请码<input
          v-model="inviteCode"
          placeholder="向管理员索取"
          autocomplete="off"
          spellcheck="false"
      /></label>
      <button
        class="primary"
        :disabled="loginBusy || !email || !password || (registerMode && !inviteCode.trim())"
      >
        {{ loginBusy ? "提交中…" : registerMode ? "注册并登录" : "登录" }}
      </button>
      <button class="ghost" type="button" @click="registerMode = !registerMode">
        {{ registerMode ? "已有账号？去登录" : "没有账号？注册一个" }}
      </button>
      <div v-if="loginError" class="login-error">{{ loginError }}</div>
    </form>
  </div>

  <!-- home：机器卡片墙 -->
  <div v-else-if="store && view === 'home'" class="home">
    <header class="home-head">
      <div class="home-brand">
        <span class="logo">π</span>
        <b>节点</b>
        <span class="home-sub">选择一台 Agent 开始工作</span>
      </div>
      <div class="home-search">
        <Icon name="search-line" :size="14" />
        <input
          ref="searchInput"
          v-model="nodeFilter"
          placeholder="搜索节点…（Ctrl+K）"
          spellcheck="false"
        />
        <button v-if="nodeFilter" class="clear" title="清空" @click="nodeFilter = ''">✕</button>
      </div>
      <div class="home-actions">
        <button
          v-if="auth?.role === 'admin'"
          class="ghost"
          title="邀请码管理"
          @click="showInvites = true"
        >
          <Icon name="key-2-line" :size="15" />
        </button>
        <button class="ghost" :title="themeMode === 'dark' ? '切换亮色' : '切换暗色'" @click="toggleTheme()">
          <Icon :name="themeMode === 'dark' ? 'sun-line' : 'moon-line'" :size="15" />
        </button>
        <button class="ghost" @click="logout">退出</button>
      </div>
    </header>
    <div class="home-body">
      <div v-if="machines.length && filteredMachines.length" class="mgrid">
        <MachineCard
          v-for="m in filteredMachines"
          :key="m.machine_id"
          :machine="m"
          :session-count="sessionCounts[m.machine_id] ?? null"
          @enter="enterMachine(m.machine_id)"
          @delete="deleteMachine(m)"
        />
      </div>
      <div v-else-if="machines.length" class="home-empty">
        <p><b>没有匹配「{{ nodeFilter.trim() }}」的节点</b></p>
        <p>换个关键词试试，或清空搜索条件。</p>
      </div>
      <div v-else class="home-empty">
        <svg class="he-icon" viewBox="0 0 24 24" fill="currentColor"><path :d="osMeta().path" /></svg>
        <p><b>还没有节点上线</b></p>
        <p>启动桌面端并在「☁ 云同步」里登录同一账号，<br />节点会自动注册到这里。</p>
      </div>
    </div>
  </div>

  <!-- workspace -->
  <div v-else-if="store" class="layout">
    <div class="col">
      <div class="side-head sessions-head">
        <b>会话</b>
      </div>
      <SessionSidebar
        :sessions="store.sessions"
        :active-id="store.activeId"
        :home-dir="store.homeDir"
        :show-settings-btn="true"
        :show-automation="true"
        :show-experts="true"
        :active-tool="showAutomation ? 'automation' : showExperts ? 'experts' : undefined"
        @select="(id) => { showAutomation = false; showExperts = false; store?.openSession(id); }"
        @new-task="() => { showAutomation = false; showExperts = false; startNewTask(); }"
        @open-project="(cwd) => { showAutomation = false; showExperts = false; startNewTask(cwd); }"
        @browse-project="(cwd) => { browseCwd = { cwd, seq: (browseCwd?.seq ?? 0) + 1 }; }"
        @remove-project="removeProject"
        @remove-session="(id) => store?.removeSessions([id])"
        @rename="(id, name) => store?.renameSession(id, name)"
        @open-settings="(tab) => openSettings(tab)"
        @open-automation="() => { showAutomation = !showAutomation; if (showAutomation) showExperts = false; }"
        @open-experts="() => { showExperts = !showExperts; if (showExperts) showAutomation = false; }"
      />
    </div>

    <main class="main">
      <header class="topbar">
        <button class="tb-btn" title="返回节点列表" @click="goHome">
          <Icon name="arrow-left-line" :size="15" />
        </button>
        <div class="mchip-wrap">
          <button class="mchip" title="节点详情" @click="showMachineInfo = !showMachineInfo">
            <span class="mchip-os" :style="{ color: activeOsMeta.color }">
              <svg viewBox="0 0 24 24" fill="currentColor"><path :d="activeOsMeta.path" /></svg>
            </span>
            <span class="mchip-name">{{ activeMachine?.hostname || "未选节点" }}</span>
            <span v-if="activeMachine" class="dot" :class="{ on: activeMachine.online }" />
            <Icon name="arrow-down-s-line" :size="13" />
          </button>
          <MachinePopover
            v-if="showMachineInfo && activeMachine"
            :machine="activeMachine"
            :session-count="sessionCounts[activeMachine.machine_id] ?? null"
            :home-dir="store.homeDir ?? undefined"
            @close="showMachineInfo = false"
          />
        </div>
        <StatePill :state="activeMachine?.online ? store.agentState : 'idle'" />
        <span class="transport">{{ bus?.transport }}</span>
        <span v-if="store.lastError" class="err" :title="store.lastError">{{ store.lastError }}</span>
        <span class="who">{{ auth?.userId.slice(0, 8) }}…</span>
      </header>
      <div v-if="sessionsEmpty && !showAutomation && !showExperts" class="hint">
        该节点还没有同步的会话，或在桌面端新建后开启同步。
      </div>
      <ChatView
        v-show="!sessionsEmpty && !showAutomation && !showExperts"
        :store="store"
        :disabled="!activeMachine?.online"
        :disabled-hint="'节点离线，无法远程控制'"
        :files-cwd="browseCwd"
        :new-task="newTaskMode"
        :new-task-cwd="newTaskCwd"
        :new-task-expert="newTaskExpert"
        :model="store.sessions.find((s) => s.session_id === store?.activeId)?.model"
        @close-files="browseCwd = null"
        @open-settings="(tab) => openSettings(tab)"
        @open-providers="() => openSettings('providers')"
      />
      <!-- 自动化/专家页与聊天共用主区（自动化调度器在桌面本地，经 /commands 远程管理） -->
      <AutomationView
        v-if="showAutomation"
        :bus="bus!"
        @close="showAutomation = false"
        @open-providers="() => openSettings('providers')"
      />
      <ExpertsView
        v-if="showExperts"
        :bus="bus!"
        @close="showExperts = false"
        @open-providers="() => openSettings('providers')"
        @hire="(expert) => { showExperts = false; startNewTask(undefined, expert); }"
        @test="testExpert"
      />
      <SettingsView
        v-if="showSettings"
        :bus="bus!"
        :initial-pane="settingsPane"
        @close="showSettings = false"
        @reset-layout="() => {}"
      />
    </main>
  </div>
  <div v-else class="login-screen">加载中…</div>
  <AdminInvites v-if="showInvites && auth" :client="auth" @close="showInvites = false" />
  </n-config-provider>
</template>

<style scoped src="./app.css"></style>
