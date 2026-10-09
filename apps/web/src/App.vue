<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from "vue";
import { darkTheme, NConfigProvider } from "naive-ui";
import {
  appConfirm,
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
  GIT_API,
  FS_LIST,
  createAgentStore,
  initTheme,
  themeMode,
  toggleTheme,
  type AgentStore,
} from "@pidock/ui";
import {
  ApiError,
  AuthClient,
  clearAuth,
  exchangeSsoCode,
  fetchAuthMethods,
  loadAuth,
  type AuthMethods,
  type LoginPending,
} from "./auth.js";
import { createWebBus, type WebBus } from "./bus.js";
import AdminAuthCfg from "./components/AdminAuthCfg.vue";
import AdminInvites from "./components/AdminInvites.vue";
import AdminUsers from "./components/AdminUsers.vue";
import AccessTokens from "./components/AccessTokens.vue";
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
// server 地址：生产环境 = 页面来源（web 由 server 托管），不暴露给用户；
// dev 前端跑在 5174，默认指本机 8080；特殊部署可用 ?server= 覆盖。
const serverUrl =
  new URLSearchParams(window.location.search).get("server") ??
  (import.meta.env.DEV ? "http://localhost:8080" : window.location.origin);
const email = ref("");
const password = ref("");
const inviteCode = ref("");
const loginBusy = ref(false);
const loginError = ref<string | null>(null);
const registerMode = ref(false);
/** 登录方式分段：标准（本地账号）/ LDAP（启用后显示） */
const loginMode = ref<"standard" | "ldap">("standard");

function switchLoginMode(mode: "standard" | "ldap"): void {
  loginMode.value = mode;
  if (mode === "ldap") registerMode.value = false;
}
/** 服务端可用的登录方式（/auth/methods；拉取失败 = 仅密码登录） */
const authMethods = ref<AuthMethods | null>(null);
/** 拉取登录方式（登录页据此渲染 SSO 按钮等）；失败静默 = 仅密码登录 */
async function refreshMethods(): Promise<void> {
  authMethods.value = await fetchAuthMethods(serverUrl);
}

// Turnstile 人机验证：服务端启用时登录/注册前须拿到 token（一次性，失败后 reset 重来）
const turnstileToken = ref("");
const turnstileBox = ref<HTMLElement | null>(null);
const turnstileEnabled = computed(
  () => !!authMethods.value?.turnstile?.enabled && !!authMethods.value?.turnstile?.site_key,
);
let turnstileWidget = "";

interface TurnstileApi {
  render: (el: HTMLElement, params: Record<string, unknown>) => string;
  reset: (id?: string) => void;
  remove: (id?: string) => void;
}
declare global {
  interface Window {
    turnstile?: TurnstileApi;
  }
}

function loadTurnstile(): Promise<TurnstileApi> {
  if (window.turnstile) return Promise.resolve(window.turnstile);
  return new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    s.async = true;
    s.onload = () =>
      window.turnstile
        ? resolve(window.turnstile)
        : reject(new Error("turnstile loaded but missing global"));
    s.onerror = () => reject(new Error("Turnstile 脚本加载失败"));
    document.head.appendChild(s);
  });
}

/** 渲染 widget（幂等）；主题跟随应用主题，切主题由 watch 重挂 */
async function mountTurnstile(): Promise<void> {
  const siteKey = authMethods.value?.turnstile?.site_key;
  if (turnstileWidget || !siteKey || !turnstileBox.value) return;
  try {
    const ts = await loadTurnstile();
    if (!turnstileBox.value) return; // 脚本加载期间表单可能已随登录卸载
    turnstileWidget = ts.render(turnstileBox.value, {
      sitekey: siteKey,
      theme: themeMode.value === "dark" ? "dark" : "light",
      callback: (token: string) => (turnstileToken.value = token),
      "expired-callback": () => (turnstileToken.value = ""),
      "error-callback": () => (turnstileToken.value = ""),
    });
  } catch (err) {
    console.warn("turnstile mount failed", err);
  }
}

/** token 已被服务端消费（siteverify 一次性），失败后重置让用户重新过验证 */
function resetTurnstile(): void {
  turnstileToken.value = "";
  if (turnstileWidget) window.turnstile?.reset(turnstileWidget);
}

watch(
  turnstileEnabled,
  (on) => {
    if (on) void nextTick(mountTurnstile);
  },
  { immediate: true },
);

// Turnstile 渲染后主题不可改，切主题需卸载重挂
watch(themeMode, () => {
  if (!turnstileEnabled.value || !window.turnstile) return;
  if (turnstileWidget) window.turnstile.remove(turnstileWidget);
  turnstileWidget = "";
  turnstileToken.value = "";
  void nextTick(mountTurnstile);
});

/** 跳转 IdP 发起 SSO 登录；记住 server 地址供回调后交换 token */
function ssoLogin(): void {
  const base = serverUrl.replace(/\/+$/, "");
  sessionStorage.setItem("pidock.sso.server", base);
  window.location.href = `${base}/auth/sso/oidc/login`;
}

// 邮箱验证码两步验证：密码通过（登录）或预检通过（注册）后进入验证码步骤
const otpStep = ref(false);
const otpPurpose = ref<"login" | "register">("login");
const otpChallenge = ref("");
const otpMaskedEmail = ref("");
const otpCode = ref("");
const otpCooldown = ref(0);
let otpTimerId = 0;

function startOtpCooldown(): void {
  otpCooldown.value = 60;
  window.clearInterval(otpTimerId);
  otpTimerId = window.setInterval(() => {
    if (otpCooldown.value > 0) otpCooldown.value--;
    if (otpCooldown.value === 0) window.clearInterval(otpTimerId);
  }, 1000);
}

function enterOtpStep(
  pending: { challenge: string; email: string },
  purpose: "login" | "register",
): void {
  otpPurpose.value = purpose;
  otpStep.value = true;
  otpChallenge.value = pending.challenge;
  otpMaskedEmail.value = pending.email;
  otpCode.value = "";
  loginError.value = null;
  startOtpCooldown();
}

/** 验证码输入只留数字、最多 6 位 */
function onOtpInput(e: Event): void {
  otpCode.value = (e.target as HTMLInputElement).value.replace(/\D/g, "").slice(0, 6);
}

/** 返回密码表单：turnstile token 已被上次提交消费，重置重拿 */
function backToPassword(): void {
  otpStep.value = false;
  otpPurpose.value = "login";
  otpChallenge.value = "";
  otpCode.value = "";
  loginError.value = null;
  resetTurnstile();
}

async function verifyOtp(): Promise<void> {
  if (loginBusy.value || otpCode.value.trim().length !== 6) return;
  loginBusy.value = true;
  loginError.value = null;
  try {
    const client = new AuthClient(null as never, () => logout());
    const state =
      otpPurpose.value === "register"
        ? await client.verifyRegisterCode(serverUrl, otpChallenge.value, otpCode.value)
        : await client.verifyLoginCode(serverUrl, otpChallenge.value, otpCode.value);
    otpStep.value = false;
    boot(state, client);
  } catch (err) {
    loginError.value = String(err instanceof ApiError ? err.message : err);
  } finally {
    loginBusy.value = false;
  }
}

async function resendOtp(): Promise<void> {
  if (loginBusy.value || otpCooldown.value > 0) return;
  loginBusy.value = true;
  loginError.value = null;
  try {
    const client = new AuthClient(null as never, () => logout());
    const r = await client.resendCode(serverUrl, otpChallenge.value);
    otpChallenge.value = r.challenge;
    otpMaskedEmail.value = r.email;
    startOtpCooldown();
  } catch (err) {
    loginError.value = String(err instanceof ApiError ? err.message : err);
  } finally {
    loginBusy.value = false;
  }
}

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
/** home=机器卡片墙；ws=会话工作区（登录后先见 home）；admin/tokens=共享左栏的内容区切换 */
const view = ref<"home" | "ws" | "admin" | "tokens">("home");
/** 管理面板内部的标签页（离开再回来保持选中） */
const adminPane = ref<"invites" | "users" | "auth">("invites");
/** 左栏管理组菜单：定位到对应面板 */
function goAdmin(pane: "invites" | "users" | "auth"): void {
  adminPane.value = pane;
  view.value = "admin";
}
const showMachineInfo = ref(false);
/** 手机端（≤768px）会话侧栏变抽屉，此为开关；桌面端样式不渲染遮罩与偏移 */
const wsSideOpen = ref(false);

async function refreshMachines(): Promise<void> {
  if (!auth.value || !bus.value) return;
  try {
    const r = await auth.value.request(`/machines`);
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
    await auth.value.request(`/machines/${m.machine_id}`, { method: "DELETE" });
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
        const r = await auth.value!.request(`/machines/${m.machine_id}/sessions`);
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
  wsSideOpen.value = false;
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

// 新建会话（首条消息创建）后退出新建任务模式；侧栏点选在 @select 里直接退出
// ——重选同一会话时 activeId 不变，watch 不会触发；手机端同时收起抽屉
watch(
  () => store.value?.activeId,
  (v) => {
    if (v) {
      newTaskMode.value = false;
      wsSideOpen.value = false;
    }
  },
);

async function doLogin(): Promise<void> {
  loginBusy.value = true;
  loginError.value = null;
  try {
    const client = new AuthClient(null as never, () => logout());
    if (loginMode.value === "ldap") {
      const state = await client.loginLdap(
        serverUrl,
        email.value.trim(),
        password.value,
        turnstileToken.value,
      );
      boot(state, client);
      return;
    }
    if (registerMode.value) {
      const r = await client.register(
        serverUrl,
        email.value.trim(),
        password.value,
        inviteCode.value,
        turnstileToken.value,
      );
      // 注册验证码：预检已过、验证码已发，进验证码步骤（验证通过即建号并自动登录）
      if ("pending" in r) {
        enterOtpStep(r, "register");
        return;
      }
    }
    const r = await client.login(serverUrl, email.value.trim(), password.value, turnstileToken.value);
    if ("pending" in r) {
      enterOtpStep(r, "login");
      return;
    }
    boot(r, client);
  } catch (err) {
    loginError.value = String(err instanceof ApiError ? err.message : err);
    resetTurnstile(); // token 一次性，重新过验证再试
  } finally {
    loginBusy.value = false;
  }
}

function logout(): void {
  // 先尽力而为吊销服务端 refresh token，再清本地（网络失败不阻塞登出）
  void auth.value?.logoutRemote();
  clearAuth();
  folderPick.value?.resolve(null);
  folderPick.value = null;
  auth.value = null;
  bus.value = null;
  store.value = null;
  machines.value = [];
  activeMachineId.value = null;
  sessionCounts.value = {};
  showMachineInfo.value = false;
  view.value = "home";
}

// provide must happen at setup level; the bus may attach later
provide(ATTACHMENT_LOADER, (id: string) => {
  const b = bus.value;
  if (!b) return Promise.reject(new Error("not connected"));
  return b.loadAttachment(id);
});

// git 分支与目录列举（远程控制视图的分支 chip 与「打开文件夹」弹层）：
// 命令中继到当前控制的目标机器上执行，机器侧跑旧版桌面端时会报 not supported
async function machineReq(method: string, params: unknown): Promise<any> {
  const b = bus.value;
  if (!b) throw new Error("not connected");
  return await b.request(method, params);
}
provide(GIT_API, {
  info: (cwd: string) => machineReq("git_info", { cwd }),
  branches: (cwd: string) => machineReq("git_branches", { cwd }),
  checkout: (cwd: string, branch: string, create?: boolean) =>
    machineReq("git_checkout", { cwd, branch, create: create ?? false }),
});
provide(FS_LIST, (path?: string) => machineReq("fs_list", { path: path ?? null }));

// 「打开文件夹」与文件选择（头像/技能包/插件导入）自绘弹层（与桌面共用 FolderBrowser）：
// fs_list 中继到目标机器，拿到的路径也是机器本地路径，交给 experts.read_avatar_file /
// config.*.install 等在机器侧读取。Promise 化，取消返回 null。
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

  // SSO 回调：?sso_code= 一次性换取 token 对；?sso_error= 展示失败原因
  const params = new URLSearchParams(window.location.search);
  const ssoCode = params.get("sso_code");
  const ssoError = params.get("sso_error");
  if (ssoCode || ssoError) {
    history.replaceState({}, "", window.location.pathname);
    const ssoServer =
      sessionStorage.getItem("pidock.sso.server") ?? window.location.origin;
    sessionStorage.removeItem("pidock.sso.server");
    if (ssoError) {
      loginError.value = ssoError;
    } else {
      loginBusy.value = true;
      exchangeSsoCode(ssoServer, ssoCode!)
        .then((state) => {
          const client = new AuthClient(state, () => logout());
          boot(state, client);
        })
        .catch((err) => {
          loginError.value = String(err instanceof ApiError ? err.message : err);
        })
        .finally(() => {
          loginBusy.value = false;
        });
    }
  }

  void refreshMethods();
  const saved = loadAuth();
  if (saved) {
    const client = new AuthClient(saved, () => logout());
    boot(saved, client);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalKey);
  if (turnstileWidget) window.turnstile?.remove(turnstileWidget);
  window.clearInterval(otpTimerId);
});

const sessionsEmpty = computed(() => {
  const items = store.value?.sessions ?? [];
  return items.length === 0 && activeMachineId.value !== null;
});
</script>

<template>
  <n-config-provider :theme="naiveTheme">
  <!-- login：左品牌区 + 右表单 -->
  <div v-if="!auth" class="auth-screen">
    <div class="auth-hero">
      <div class="hero-brand">
        <span class="hero-logo">π</span>
        <b>PiDock</b>
      </div>
      <p class="hero-tag">远程掌控你的 AI Agent 节点</p>
      <ul class="hero-feats">
        <li><Icon name="computer-line" :size="15" />多节点在线状态，Ctrl+K 随手搜</li>
        <li><Icon name="chat-1-line" :size="15" />会话实时同步，浏览器里远程对话</li>
        <li><Icon name="key-2-line" :size="15" />访问令牌接入桌面端，免密不过期</li>
      </ul>
    </div>
    <div class="auth-side">
      <form class="auth-card" @submit.prevent="otpStep ? verifyOtp() : doLogin()">
        <h2 class="auth-title">{{ otpStep ? (otpPurpose === "register" ? "验证邮箱" : "两步验证") : registerMode ? "创建账号" : "欢迎回来" }}</h2>
        <p class="auth-sub">{{
          otpStep
            ? `验证码已发送至 ${otpMaskedEmail}${otpPurpose === "register" ? "，验证后创建账号" : ""}`
            : registerMode
              ? "凭邀请码注册新账号"
              : "登录你的 PiDock 账号"
        }}</p>
        <div v-if="!otpStep && authMethods?.ldap?.enabled" class="auth-tabs">
          <button
            type="button"
            class="auth-tab"
            :class="{ on: loginMode === 'standard' }"
            @click="switchLoginMode('standard')"
          >
            标准登录
          </button>
          <button
            type="button"
            class="auth-tab"
            :class="{ on: loginMode === 'ldap' }"
            @click="switchLoginMode('ldap')"
          >
            LDAP 登录
          </button>
        </div>
        <template v-if="!otpStep">
          <label>{{ loginMode === "ldap" ? "用户名" : "邮箱" }}<input
              v-model="email"
              :type="loginMode === 'ldap' ? 'text' : 'email'"
              :placeholder="loginMode === 'ldap' ? 'LDAP 目录中的用户名' : 'you@example.com'"
              autocomplete="username"
          /></label>
          <label>密码<input v-model="password" type="password" autocomplete="current-password" /></label>
          <label v-if="registerMode"
            >邀请码<input
              v-model="inviteCode"
              placeholder="向管理员索取"
              autocomplete="off"
              spellcheck="false"
          /></label>
          <!-- Turnstile：容器内容由 Cloudflare 脚本接管，这里只占位 -->
          <div v-if="turnstileEnabled" ref="turnstileBox" class="auth-captcha"></div>
        </template>
        <template v-else>
          <label>验证码<input
              :value="otpCode"
              @input="onOtpInput"
              type="text"
              inputmode="numeric"
              autocomplete="one-time-code"
              maxlength="6"
              placeholder="6 位数字"
          /></label>
          <div class="auth-otp-actions">
            <button
              type="button"
              class="auth-link"
              :disabled="loginBusy || otpCooldown > 0"
              @click="resendOtp"
            >
              {{ otpCooldown > 0 ? `重新发送（${otpCooldown}s）` : "重新发送验证码" }}
            </button>
            <button type="button" class="auth-link" @click="backToPassword">返回上一步</button>
          </div>
        </template>
        <button
          class="auth-primary"
          :disabled="
            loginBusy ||
            (otpStep
              ? otpCode.length !== 6
              : !email || !password || (registerMode && !inviteCode.trim()) || (turnstileEnabled && !turnstileToken))
          "
        >
          {{ loginBusy ? "提交中…" : otpStep ? (otpPurpose === "register" ? "验证并创建账号" : "验证并登录") : loginMode === "ldap" ? "LDAP 登录" : registerMode ? "注册并登录" : "登录" }}
        </button>
        <template v-if="!otpStep && authMethods?.oidc?.enabled">
          <div class="auth-divider"><span>或</span></div>
          <button class="auth-sso" type="button" @click="ssoLogin">
            使用 {{ authMethods.oidc.label || "SSO" }} 登录
          </button>
        </template>
        <button
          v-if="!otpStep && loginMode === 'standard' && authMethods?.register?.allowed !== false"
          class="auth-switch"
          type="button"
          @click="registerMode = !registerMode"
        >
          {{ registerMode ? "已有账号？去登录" : "没有账号？注册一个" }}
        </button>
        <div v-if="loginError" class="auth-error">{{ loginError }}</div>
      </form>
    </div>
  </div>

  <!-- 控制台：左栏常驻，右侧内容按 view 切换（节点 / 管理面板 / 访问令牌） -->
  <div v-else-if="store && view !== 'ws'" class="home">
    <aside class="home-side">
      <div class="side-brand">
        <span class="logo">π</span>
        <b>PiDock</b>
      </div>
      <nav class="side-nav">
        <button class="side-item" :class="{ on: view === 'home' }" title="节点列表" @click="view = 'home'">
          <Icon name="computer-line" :size="15" />
          <span>节点</span>
        </button>
        <button
          class="side-item"
          :class="{ on: view === 'tokens' }"
          title="访问令牌"
          @click="view = 'tokens'"
        >
          <Icon name="shield-flash-line" :size="15" />
          <span>访问令牌</span>
        </button>
        <!-- 管理组：仅管理员可见 -->
        <template v-if="auth?.role === 'admin'">
          <div class="side-group">管理</div>
          <button
            class="side-item"
            :class="{ on: view === 'admin' && adminPane === 'invites' }"
            title="邀请码"
            @click="goAdmin('invites')"
          >
            <Icon name="key-2-line" :size="15" />
            <span>邀请码</span>
          </button>
          <button
            class="side-item"
            :class="{ on: view === 'admin' && adminPane === 'users' }"
            title="用户管理"
            @click="goAdmin('users')"
          >
            <Icon name="user-star-line" :size="15" />
            <span>用户管理</span>
          </button>
          <button
            class="side-item"
            :class="{ on: view === 'admin' && adminPane === 'auth' }"
            title="登录方式"
            @click="goAdmin('auth')"
          >
            <Icon name="shield-check-line" :size="15" />
            <span>登录方式</span>
          </button>
        </template>
      </nav>
      <div class="side-foot">
        <button
          class="side-item"
          :title="themeMode === 'dark' ? '切换亮色' : '切换暗色'"
          @click="toggleTheme()"
        >
          <Icon :name="themeMode === 'dark' ? 'sun-line' : 'moon-line'" :size="15" />
          <span>{{ themeMode === "dark" ? "亮色模式" : "暗色模式" }}</span>
        </button>
        <button class="side-item" title="退出登录" @click="logout">
          <Icon name="arrow-right-line" :size="15" />
          <span>退出登录</span>
        </button>
        <div class="side-user" :title="auth?.email">{{ auth?.email }}</div>
      </div>
    </aside>
    <main class="home-main">
      <!-- 节点卡片墙 -->
      <template v-if="view === 'home'">
        <header class="home-head">
          <b>节点</b>
          <span class="home-sub">选择一台 Agent 开始工作</span>
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
      </template>
      <!-- 管理面板：左栏管理组菜单对应的内容 -->
      <div v-else-if="view === 'admin'" class="pane-frame">
        <AdminInvites v-if="adminPane === 'invites'" :client="auth" />
        <AdminUsers v-else-if="adminPane === 'users'" :client="auth" />
        <AdminAuthCfg v-else :client="auth" />
      </div>
      <AccessTokens v-else-if="view === 'tokens'" :client="auth" />
    </main>
  </div>

  <!-- workspace -->
  <div v-else-if="store" class="layout" :class="{ 'ws-side-open': wsSideOpen }">
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
        @select="(id) => { showAutomation = false; showExperts = false; newTaskMode = false; wsSideOpen = false; store?.openSession(id); }"
        @new-task="() => { showAutomation = false; showExperts = false; wsSideOpen = false; startNewTask(); }"
        @open-project="(cwd) => { showAutomation = false; showExperts = false; wsSideOpen = false; startNewTask(cwd); }"
        @browse-project="(cwd) => { wsSideOpen = false; browseCwd = { cwd, seq: (browseCwd?.seq ?? 0) + 1 }; }"
        @remove-project="removeProject"
        @remove-session="(id) => store?.removeSessions([id])"
        @rename="(id, name) => store?.renameSession(id, name)"
        @open-settings="(tab) => { wsSideOpen = false; openSettings(tab); }"
        @open-automation="() => { wsSideOpen = false; showAutomation = !showAutomation; if (showAutomation) showExperts = false; }"
        @open-experts="() => { wsSideOpen = false; showExperts = !showExperts; if (showExperts) showAutomation = false; }"
      />
    </div>
    <div v-if="wsSideOpen" class="side-scrim" @click="wsSideOpen = false" />

    <main class="main">
      <header class="topbar">
        <button class="tb-btn tb-menu" title="会话列表" @click="wsSideOpen = !wsSideOpen">
          <Icon name="menu-line" :size="16" />
        </button>
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
  <div v-else class="boot-screen">加载中…</div>
  <FolderBrowser
    v-if="folderPick"
    :mode="folderPick.mode"
    :accept="folderPick.accept"
    @pick="onFolderPicked"
    @close="onFolderPickCancel"
  />
  </n-config-provider>
</template>

<style scoped src="./app.css"></style>
