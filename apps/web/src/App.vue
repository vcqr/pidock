<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { darkTheme, NConfigProvider } from "naive-ui";
import {
  ChatView,
  SessionSidebar,
  StatePill,
  ATTACHMENT_LOADER,
  createAgentStore,
  initTheme,
  themeMode,
  toggleTheme,
  type AgentStore,
} from "@pidock/ui";
import { provide } from "vue";
import { ApiError, AuthClient, loadAuth, clearAuth } from "./auth.js";
import { createWebBus, type WebBus } from "./bus.js";

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
const loginBusy = ref(false);
const loginError = ref<string | null>(null);
const registerMode = ref(false);

// machines
interface MachineUi {
  machine_id: string;
  hostname: string;
  os: string;
  online: boolean;
  last_seen: string;
}
const machines = ref<MachineUi[]>([]);
const activeMachineId = ref<string | null>(null);
const activeMachine = computed(() => machines.value.find((m) => m.machine_id === activeMachineId.value));

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
  if (!window.confirm(`删除机器「${m.hostname || m.machine_id.slice(0, 8)}」及其全部同步数据？`)) return;
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

async function doLogin(): Promise<void> {
  loginBusy.value = true;
  loginError.value = null;
  try {
    const client = new AuthClient(null as never, () => logout());
    if (registerMode.value) {
      await client.register(serverUrl.value.trim(), email.value.trim(), password.value);
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
      if (machineId === activeMachineId.value) {
        // online/offline affects composer state; refresh session statuses too
        void s.refreshSessions();
      }
    });
    await refreshMachines();
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
        created_at: session.created_at || new Date().toISOString(),
        state: session.status === "running" ? "responding" : (session.status || "idle"),
        open: true,
      });
      if (machineId !== activeMachineId.value) void refreshMachines();
    });
    setInterval(() => void s.refreshSessions(), 30000);
  });
}

onMounted(() => {
  const saved = loadAuth();
  if (saved) {
    const client = new AuthClient(saved, () => logout());
    boot(saved, client);
  }
});

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
      <button class="primary" :disabled="loginBusy || !email || !password">
        {{ loginBusy ? "提交中…" : registerMode ? "注册并登录" : "登录" }}
      </button>
      <button class="ghost" type="button" @click="registerMode = !registerMode">
        {{ registerMode ? "已有账号？去登录" : "没有账号？注册一个" }}
      </button>
      <div v-if="loginError" class="login-error">{{ loginError }}</div>
    </form>
  </div>

  <!-- main -->
  <div v-else-if="store" class="layout">
    <aside class="machines">
      <div class="side-head">
        <span class="brand-sm">π</span>
        <b>机器</b>
        <button class="ghost" :title="themeMode === 'dark' ? '切换亮色' : '切换暗色'" @click="toggleTheme()">
          {{ themeMode === "dark" ? "☀" : "☾" }}
        </button>
        <button class="ghost" @click="logout">退出</button>
      </div>
      <div
        v-for="m in machines"
        :key="m.machine_id"
        class="machine"
        :class="{ active: m.machine_id === activeMachineId }"
        @click="selectMachine(m.machine_id)"
      >
        <div class="m-name">
          <span class="dot" :class="{ on: m.online }" />
          {{ m.hostname || m.machine_id.slice(0, 8) }}
          <button
            v-if="!m.online"
            class="del"
            title="删除该机器及其同步数据"
            @click.stop="deleteMachine(m)"
          >✕</button>
        </div>
        <div class="m-sub">{{ m.os }} · {{ m.online ? "在线" : "离线" }}</div>
      </div>
      <div v-if="!machines.length" class="empty">
        还没有机器上线。启动桌面端并在「☁ 云同步」里登录同一账号。
      </div>
    </aside>

    <div class="col">
      <div class="side-head sessions-head">
        <b>会话</b>
        <StatePill :state="activeMachine?.online ? store.agentState : 'idle'" />
      </div>
      <SessionSidebar
        :sessions="store.sessions"
        :active-id="store.activeId"
        :home-dir="store.homeDir"
        :show-settings-btn="false"
        @select="(id) => store?.openSession(id)"
        @rename="(id, name) => store?.renameSession(id, name)"
      />
    </div>

    <main class="main">
      <header class="topbar">
        <StatePill :state="store.agentState" />
        <span class="transport">{{ bus?.transport }} · {{ activeMachine?.hostname || "未选机器" }}</span>
        <span class="who">{{ auth?.userId.slice(0, 8) }}…</span>
      </header>
      <div v-if="sessionsEmpty" class="hint">该机器还没有同步的会话，或在桌面端新建后开启同步。</div>
      <ChatView
        v-else
        :store="store"
        :disabled="!activeMachine?.online"
        :disabled-hint="'机器离线，无法远程控制'"
      />
    </main>
  </div>
  <div v-else class="login-screen">加载中…</div>
  </n-config-provider>
</template>

<style scoped src="./app.css"></style>
