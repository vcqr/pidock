<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import type { DataBus } from "../databus.js";
import { REVEAL_PATH, WINDOW_CONTROLS } from "../databus.js";
import Icon from "./Icon.vue";
import ProvidersView from "./ProvidersView.vue";
import ToolsView from "./ToolsView.vue";
import { applyTheme, themePref, applyFontSettings, fontSettings, type ThemePref } from "../theme.js";

/**
 * 设置中心：左侧分组导航 + 右侧内容页（参考主流 AI 客户端布局）。
 * 设置是唯一的配置中心：插件/技能/MCP/供应商的完整管理直接内嵌本页
 * （复用 ToolsView / ProvidersView），主区只保留会话。
 * 页面按 pi 的真实能力取舍：常规 / 外观 / 模型 / 供应商 / 快捷键 /
 * 记忆(AGENTS.md) / 插件 / MCP / 技能 / 命令 / 使用统计 / 引导。
 */
const props = withDefaults(defineProps<{ bus: DataBus; initialPane?: string }>(), {
  initialPane: "general",
});
const emit = defineEmits<{
  close: [];
  resetLayout: [];
}>();

type PaneId =
  | "general"
  | "appearance"
  | "models"
  | "providers"
  | "shortcuts"
  | "memory"
  | "plugins"
  | "mcp"
  | "skills"
  | "commands"
  | "usage"
  | "sync"
  | "guide";

const PANE_IDS = new Set<PaneId>([
  "general", "appearance", "models", "providers", "shortcuts", "memory",
  "plugins", "mcp", "skills", "commands", "usage", "sync", "guide",
]);

/** legacy tab names (旧设置中心的 initialTab) → 新页面 */
const LEGACY_PANE: Record<string, PaneId> = {
  models: "models",
  providers: "providers",
  extensions: "plugins",
  skills: "skills",
  mcp: "mcp",
};

const initial = LEGACY_PANE[props.initialPane] ?? (props.initialPane as PaneId);
const pane = ref<PaneId>(initial && PANE_IDS.has(initial) ? initial : "general");

interface NavItem {
  id: PaneId;
  label: string;
  icon: string;
  count?: () => number | undefined;
}
const navSections: Array<{ title: string; items: NavItem[] }> = [
  {
    title: "基础设置",
    items: [
      { id: "general", label: "常规", icon: "settings-3-line" },
      { id: "appearance", label: "外观", icon: "palette-line" },
      { id: "shortcuts", label: "键盘快捷键", icon: "keyboard-line" },
    ],
  },
  {
    title: "模型",
    items: [
      { id: "models", label: "模型设置", icon: "box-3-line" },
      { id: "providers", label: "供应商与密钥", icon: "key-2-line" },
    ],
  },
  {
    title: "Agent 能力",
    items: [
      { id: "memory", label: "记忆", icon: "brain-line" },
      { id: "plugins", label: "插件", icon: "puzzle-2-line", count: () => counts.value.plugins },
      { id: "mcp", label: "MCP 服务器", icon: "plug-line", count: () => counts.value.mcp },
      { id: "skills", label: "技能", icon: "magic-line", count: () => counts.value.skills },
      { id: "commands", label: "命令", icon: "terminal-line" },
    ],
  },
  {
    title: "数据与同步",
    items: [
      { id: "usage", label: "使用统计", icon: "bar-chart-line" },
      { id: "sync", label: "云同步", icon: "cloud-line" },
    ],
  },
  {
    title: "引导",
    items: [{ id: "guide", label: "引导", icon: "rocket-line" }],
  },
];

/** 这些页面的内容是完整内嵌的管理页（自带标题与滚动），不需要设置层再包标题 */
const EMBED_PANES = new Set<PaneId>(["plugins", "mcp", "skills", "providers"]);

const revealPath = inject(REVEAL_PATH, null);
/** 桌面端无边框窗口控制；设置页全屏盖住标题栏时，由它在页头补齐窗口按钮 */
const win = inject(WINDOW_CONTROLS, null);

let flashTimer: ReturnType<typeof setTimeout> | undefined;
const notice = ref<string | null>(null);
function flash(msg: string): void {
  notice.value = msg;
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (notice.value = null), 2500);
}

// ---------------------------------------------------------------- shared state

const loading = ref(true);
const agentDir = ref("");
/** ~/.pi/agent/settings.json 的本地副本（保存成功后同步更新） */
const settings = ref<Record<string, any>>({});

/** 三类资源的全量列表（nav 计数、摘要卡、命令页共用） */
const extAll = ref<any[]>([]);
const skillAll = ref<any[]>([]);
const mcpServers = ref<Record<string, any>>({});
const counts = computed(() => ({
  plugins: extAll.value.length,
  skills: skillAll.value.length,
  mcp: Object.keys(mcpServers.value).length,
}));

async function loadCore(): Promise<void> {
  loading.value = true;
  try {
    const g = await props.bus.request("config.get");
    settings.value = g.settings ?? {};
    agentDir.value = g.agent_dir ?? "";
    const [ext, sk, mcp] = await Promise.all([
      props.bus.request("config.extensions.list", {}).catch(() => null),
      props.bus.request("config.skills.list", {}).catch(() => null),
      props.bus.request("config.mcp.get", {}).catch(() => null),
    ]);
    extAll.value = ext?.extensions ?? [];
    skillAll.value = sk?.skills ?? [];
    mcpServers.value = mcp?.config?.mcpServers ?? mcp?.config?.servers ?? {};
  } catch (err) {
    flash(String(err));
  } finally {
    loading.value = false;
  }
  void ensureMemory();
}
onMounted(loadCore);

/** 写一个 settings.json 字段（全部作用于新会话） */
async function setSetting(key: string, value: unknown, msg = "已保存，对新会话生效"): Promise<void> {
  const old = settings.value[key];
  settings.value[key] = value; // optimistic
  try {
    await props.bus.request("config.settings.set", { patch: { [key]: value } });
    flash(msg);
  } catch (err) {
    settings.value[key] = old;
    flash(String(err));
  }
}

function boolSetting(key: string, fallback = false): boolean {
  const v = settings.value[key];
  return typeof v === "boolean" ? v : fallback;
}

// ---------------------------------------------------------------- 代理设置

type ProxyMode = "direct" | "http" | "system";
interface ProxyConf {
  mode: ProxyMode;
  url: string;
  noProxy: string;
  caPath: string;
}
const proxy = ref<ProxyConf>({ mode: "direct", url: "", noProxy: "", caPath: "" });
const proxyLoaded = ref(false);
const proxySaving = ref(false);
const PROXY_MODE_HINT: Record<ProxyMode, string> = {
  direct: "所有请求直连，不使用代理",
  system: "自动读取系统代理设置（Windows Internet 选项）",
  http: "手动指定 HTTP 代理地址",
};

async function ensureProxy(force = false): Promise<void> {
  if (proxyLoaded.value && !force) return;
  try {
    const r = await props.bus.request("pidock.settings.get");
    const p = r.settings?.proxy ?? {};
    proxy.value = {
      mode: p.mode === "http" || p.mode === "system" ? p.mode : "direct",
      url: p.url ?? "",
      noProxy: p.noProxy ?? "",
      caPath: p.caPath ?? "",
    };
    // AskUserQuestion 等待超时（settings.json ask.timeoutSec，秒 → 分钟展示）
    const sec = Number(r.settings?.ask?.timeoutSec);
    askTimeoutMin.value = Number.isFinite(sec) && sec >= 0 ? sec / 60 : 3;
    proxyLoaded.value = true;
  } catch (err) {
    flash(String(err));
  }
}

async function saveProxy(): Promise<void> {
  if (proxy.value.mode === "http" && !proxy.value.url.trim()) {
    flash("请填写代理地址");
    return;
  }
  proxySaving.value = true;
  try {
    await props.bus.request("pidock.settings.set", { proxy: proxy.value });
    flash("代理已保存，重启 PiDock 后生效");
  } catch (err) {
    flash(String(err));
  } finally {
    proxySaving.value = false;
  }
}

// --------------------------------------------------------- AskUserQuestion 超时

/** 提问等待超时（分钟，0 = 一直等待）；settings.json ask.timeoutSec 以秒存储 */
const askTimeoutMin = ref<number>(3);
const askTimeoutSaving = ref(false);

async function saveAskTimeout(): Promise<void> {
  const min = Number(askTimeoutMin.value);
  if (!Number.isFinite(min) || min < 0 || min > 60) {
    flash("等待超时需在 0–60 分钟之间（0 = 一直等待）");
    return;
  }
  askTimeoutSaving.value = true;
  try {
    await props.bus.request("pidock.settings.set", { ask: { timeoutSec: Math.round(min * 60) } });
    flash("提问等待超时已保存，下一次提问生效");
  } catch (err) {
    flash(String(err));
  } finally {
    askTimeoutSaving.value = false;
  }
}

// ---------------------------------------------------------------- 模型设置

interface ModelUi {
  provider: string;
  id: string;
  name: string;
  reasoning: boolean;
}
interface ProviderUi {
  id: string;
  auth: string;
  models: number;
  default: boolean;
}
const models = ref<ModelUi[]>([]);
const providers = ref<ProviderUi[]>([]);
const modelsLoaded = ref(false);
const modelsLoading = ref(false);
const modelQuery = ref("");

const defaultModelKey = computed(
  () => `${settings.value.defaultProvider ?? ""}/${settings.value.defaultModel ?? ""}`,
);

const filteredModels = computed(() => {
  const q = modelQuery.value.trim().toLowerCase();
  const list = q
    ? models.value.filter((m) => `${m.provider}/${m.id}`.toLowerCase().includes(q))
    : models.value.slice();
  return list.slice(0, 200);
});

async function ensureModels(force = false): Promise<void> {
  if (modelsLoaded.value && !force) return;
  modelsLoading.value = true;
  try {
    const [m, p] = await Promise.all([
      props.bus.request("config.models.list"),
      props.bus.request("config.providers.list"),
    ]);
    models.value = m.models ?? [];
    providers.value = p.providers ?? [];
    modelsLoaded.value = true;
  } catch (err) {
    flash(String(err));
  } finally {
    modelsLoading.value = false;
  }
}

async function setDefaultModel(m: ModelUi): Promise<void> {
  try {
    await props.bus.request("config.models.set_default", { provider: m.provider, model: m.id });
    settings.value.defaultProvider = m.provider;
    settings.value.defaultModel = m.id;
    flash(`默认模型已设为 ${m.provider}/${m.id}`);
  } catch (err) {
    flash(String(err));
  }
}

const thinkingLevels = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

// ---------------------------------------------------------------- 记忆 (AGENTS.md)

const memoryText = ref("");
const memoryPath = ref("");
const memoryExists = ref(false);
const memoryLoaded = ref(false);
const memorySaving = ref(false);

async function ensureMemory(force = false): Promise<void> {
  if (memoryLoaded.value && !force) return;
  try {
    const r = await props.bus.request("config.agents.read");
    memoryText.value = r.text ?? "";
    memoryPath.value = r.path ?? "";
    memoryExists.value = !!r.exists;
    memoryLoaded.value = true;
  } catch (err) {
    flash(String(err));
  }
}

async function saveMemory(): Promise<void> {
  memorySaving.value = true;
  try {
    const r = await props.bus.request("config.agents.write", { text: memoryText.value });
    memoryPath.value = r.path ?? memoryPath.value;
    memoryExists.value = true;
    flash("记忆已保存，对新会话生效");
  } catch (err) {
    flash(String(err));
  } finally {
    memorySaving.value = false;
  }
}

// ------------------------------------------------------- 插件/MCP/技能 计数
// 完整管理页直接内嵌 ToolsView；这里仅保留列表数据供 nav 角标与命令页使用

// ---------------------------------------------------------------- 使用统计

interface Usage {
  scanned_sessions: number;
  messages: { user: number; assistant: number };
  tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number };
  cost: number;
  by_model: Array<{ key: string; sessions: number; messages: number; tokens: number }>;
  by_day: Array<{ day: string; messages: number; tokens: number }>;
  top_sessions: Array<{
    session_id: string;
    name?: string;
    cwd: string;
    model?: string;
    messages: number;
    tokens: number;
    created_at: string;
  }>;
}
const usage = ref<Usage | null>(null);
const usageLoading = ref(false);

async function ensureUsage(force = false): Promise<void> {
  if (usage.value && !force) return;
  usageLoading.value = true;
  try {
    usage.value = await props.bus.request("stats.usage", {});
  } catch (err) {
    flash(String(err));
  } finally {
    usageLoading.value = false;
  }
}

function fmtTokens(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(2) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1) + "K";
  return String(n);
}

function baseName(p: string): string {
  const norm = (p || "").replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1) || norm || "—";
}

const maxDayTokens = computed(() => Math.max(1, ...(usage.value?.by_day ?? []).map((d) => d.tokens)));
/** 最近 30 天（不足补空） */
const usageDays = computed(() => usage.value?.by_day?.slice(-30) ?? []);

// ---------------------------------------------------------------- 命令页

const commandList = computed(() =>
  skillAll.value.map((x) => ({
    name: x.name,
    description: x.description || "",
    enabled: !!x.enabled,
  })),
);

// ---------------------------------------------------------------- 引导

const guideSteps = computed<Array<{ id: PaneId; title: string; desc: string; done: boolean; cta: string }>>(() => {
  const keyOk = providers.value.some((p) => p.auth !== "missing");
  const modelOk = !!settings.value.defaultModel;
  const extOk = counts.value.plugins > 0 || counts.value.skills > 0 || counts.value.mcp > 0;  return [
    {
      id: "providers",
      title: "配置模型密钥",
      desc: "为模型供应商填入 API Key，或确认环境变量已就位。",
      done: keyOk,
      cta: keyOk ? "查看" : "去配置",
    },
    {
      id: "models",
      title: "选择默认模型",
      desc: "新会话默认使用的供应商与模型，可随时在输入卡片里切换。",
      done: modelOk,
      cta: "去选择",
    },
    {
      id: "memory",
      title: "写下你的记忆",
      desc: "AGENTS.md 会作为长期上下文注入每次会话，适合放偏好与规范。",
      done: memoryExists.value,
      cta: "去书写",
    },
    {
      id: "skills",
      title: "装上技能与插件",
      desc: "技能、插件、MCP 服务器扩展 Agent 能力，放入目录即被发现。",
      done: extOk,
      cta: "去管理",
    },
    {
      id: "general",
      title: "确认项目信任策略",
      desc: "决定项目内 .pi 资源（扩展/技能/设置）默认是否被信任。",
      done: !!settings.value.defaultProjectTrust && settings.value.defaultProjectTrust !== "ask",
      cta: "去设置",
    },
  ];
});

// ---------------------------------------------------------------- 懒加载分发

watch(
  pane,
  (p) => {
    if (p === "models" || p === "guide") void ensureModels();
    else if (p === "memory") void ensureMemory();
    else if (p === "usage") void ensureUsage();
    else if (p === "general") void ensureProxy();
  },
  { immediate: true },
);

/** 外观页字号档位（写入 --pd-font-scale） */
const FONT_SCALE_OPTIONS: Array<{ label: string; value: number }> = [
  { label: "小", value: 0.9 },
  { label: "标准", value: 1 },
  { label: "大", value: 1.12 },
  { label: "特大", value: 1.25 },
];

function switchTheme(m: ThemePref): void {
  applyTheme(m);
}

function resetSidebarWidth(): void {
  emit("resetLayout");
  flash("已恢复默认布局");
}

// 快捷键清单（与 Composer/App 实际绑定保持一致）
const shortcuts = [
  { keys: ["Enter"], desc: "发送消息" },
  { keys: ["Shift", "Enter"], desc: "输入框换行" },
  { keys: ["Esc"], desc: "关闭弹层 / 菜单" },
  { keys: ["Ctrl", "N"], desc: "新建任务" },
  { keys: ["@"], desc: "提及工作区文件" },
  { keys: ["/"], desc: "打开技能与命令菜单" },
];

function openFolder(): void {
  if (agentDir.value) void revealPath?.(agentDir.value);
}
</script>

<template>
  <div class="settings">
    <header class="head" data-tauri-drag-region>
      <button class="back-btn" title="返回工作区" @click="emit('close')">
        <Icon name="arrow-left-line" :size="16" />
        <span>返回工作区</span>
      </button>
      <h1>设置</h1>
      <span class="flex-sp"></span>
      <span v-if="agentDir" class="agent-dir" :title="agentDir + '（点击打开）'" @click="openFolder">
        {{ agentDir }}
      </span>
      <template v-if="win">
        <span class="win-sep"></span>
        <button class="wbtn" title="最小化" @click="win.minimize()">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 12h14" /></svg>
        </button>
        <button class="wbtn" :title="win.isMax.value ? '还原' : '最大化'" @click="win.toggleMaximize()">
          <svg v-if="win.isMax.value" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M9 9h10v10H9z" /><path d="M5 15V5h10" /></svg>
          <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M6 6h12v12H6z" /></svg>
        </button>
        <button class="wbtn close" title="关闭" @click="win.close()">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18" /></svg>
        </button>
      </template>
    </header>

    <div class="body">
      <nav class="nav">
        <div v-for="sec in navSections" :key="sec.title" class="nav-sec">
          <div class="nav-sec-title">{{ sec.title }}</div>
          <button
            v-for="it in sec.items"
            :key="it.id"
            class="nav-item"
            :class="{ on: pane === it.id }"
            @click="pane = it.id"
          >
            <Icon :name="it.icon" :size="16" />
            <span class="nav-label">{{ it.label }}</span>
            <span v-if="it.count?.()" class="nav-count">{{ it.count() }}</span>
          </button>
        </div>
      </nav>

      <section class="content" :class="{ embed: EMBED_PANES.has(pane) }">
        <div v-if="notice" class="notice">{{ notice }}</div>

        <!-- ============ 常规 ============ -->
        <template v-if="pane === 'general'">
          <h2>常规</h2>
          <p class="pane-sub">pi 全局配置（{{ agentDir }}/settings.json），改动对新会话生效。</p>

          <div class="group">
            <div class="row">
              <div class="row-text">
                <b>启动时静默</b>
                <span>不显示启动横幅与提示信息</span>
              </div>
              <label class="switch" @click.prevent="setSetting('quietStartup', !boolSetting('quietStartup'))">
                <input type="checkbox" :checked="boolSetting('quietStartup')" />
                <span class="slider"></span>
              </label>
            </div>
            <div class="row">
              <div class="row-text">
                <b>技能斜杠命令</b>
                <span>在输入框中以 /skill:名称 调用技能</span>
              </div>
              <label class="switch" @click.prevent="setSetting('enableSkillCommands', !boolSetting('enableSkillCommands', true))">
                <input type="checkbox" :checked="boolSetting('enableSkillCommands', true)" />
                <span class="slider"></span>
              </label>
            </div>
            <div class="row">
              <div class="row-text">
                <b>隐藏思考块</b>
                <span>不展示模型的思考（thinking）内容</span>
              </div>
              <label class="switch" @click.prevent="setSetting('hideThinkingBlock', !boolSetting('hideThinkingBlock'))">
                <input type="checkbox" :checked="boolSetting('hideThinkingBlock')" />
                <span class="slider"></span>
              </label>
            </div>
            <div class="row">
              <div class="row-text">
                <b>项目信任策略</b>
                <span>项目内 .pi 资源（扩展/技能/设置）的默认信任方式</span>
              </div>
              <select
                class="sel"
                :value="settings.defaultProjectTrust ?? 'ask'"
                @change="setSetting('defaultProjectTrust', ($event.target as HTMLSelectElement).value)"
              >
                <option value="ask">每次询问</option>
                <option value="always">总是信任</option>
                <option value="never">从不信任</option>
              </select>
            </div>
          </div>

          <h3 class="grp-title">对话交互</h3>
          <div class="group">
            <div class="row col">
              <div class="row-text">
                <b>提问等待超时（分钟）</b>
                <span>AskUserQuestion 提问无人回答时的等待时限，超时按取消处理、模型自行继续；0 = 一直等待。全局生效，下一次提问起启用</span>
              </div>
              <input
                v-model.number="askTimeoutMin"
                class="txt"
                type="number"
                min="0"
                max="60"
                step="1"
                placeholder="3"
              />
            </div>
            <div class="row">
              <div class="row-text"><span>完全访问（full）权限模式下同样生效</span></div>
              <button class="dark-btn" :disabled="askTimeoutSaving" @click="saveAskTimeout">
                {{ askTimeoutSaving ? "保存中…" : "保存" }}
              </button>
            </div>
          </div>

          <h3 class="grp-title">代理设置</h3>
          <div class="group">
            <div class="row col">
              <div class="row-text">
                <b>代理模式</b>
                <span>{{ PROXY_MODE_HINT[proxy.mode] }}</span>
              </div>
              <div class="seg">
                <button :class="{ on: proxy.mode === 'direct' }" @click="proxy.mode = 'direct'">直连</button>
                <button :class="{ on: proxy.mode === 'system' }" @click="proxy.mode = 'system'">跟随系统</button>
                <button :class="{ on: proxy.mode === 'http' }" @click="proxy.mode = 'http'">HTTP 代理</button>
              </div>
            </div>
            <div v-if="proxy.mode === 'http'" class="row col">
              <div class="row-text">
                <b>代理地址</b>
                <span>对模型 API、MCP 等出站请求生效</span>
              </div>
              <input v-model="proxy.url" class="txt" placeholder="http://127.0.0.1:7890" spellcheck="false" />
            </div>
            <div class="row col">
              <div class="row-text">
                <b>绕过列表（NO_PROXY）</b>
                <span>逗号分隔，匹配的主机不走代理；跟随系统时与系统例外合并</span>
              </div>
              <input v-model="proxy.noProxy" class="txt" placeholder="localhost,127.0.0.1,.internal" spellcheck="false" />
            </div>
            <div class="row col">
              <div class="row-text">
                <b>自定义 CA 证书</b>
                <span>PEM 文件路径，自签名或企业代理所需；留空使用内置证书</span>
              </div>
              <input v-model="proxy.caPath" class="txt" placeholder="C:\certs\corp-root.pem" spellcheck="false" />
            </div>
            <div class="row">
              <div class="row-text">
                <span>代理为进程级配置，保存后重启 PiDock 生效</span>
              </div>
              <button class="dark-btn" :disabled="proxySaving" @click="saveProxy">
                {{ proxySaving ? "保存中…" : "保存" }}
              </button>
            </div>
          </div>

          <h3 class="grp-title">数据目录</h3>
          <div class="group">
            <div class="row">
              <div class="row-text">
                <b>pi 配置目录</b>
                <span class="mono">{{ agentDir || "—" }}</span>
              </div>
              <button v-if="revealPath && agentDir" class="ghost-btn" @click="openFolder">
                <Icon name="folder-open-line" :size="14" />打开
              </button>
            </div>
          </div>
        </template>

        <!-- ============ 外观 ============ -->
        <template v-else-if="pane === 'appearance'">
          <h2>外观</h2>
          <p class="pane-sub">界面主题立即生效；标题栏 ☀/☾ 按钮可随时切换。</p>

          <div class="group">
            <div class="row col">
              <div class="row-text">
                <b>界面主题</b>
                <span>跟随系统时自动响应系统深浅色切换</span>
              </div>
              <div class="theme-cards">
                <button class="theme-card" :class="{ on: themePref === 'dark' }" @click="switchTheme('dark')">
                  <Icon name="moon-line" :size="18" />
                  <span>暗色</span>
                </button>
                <button class="theme-card" :class="{ on: themePref === 'light' }" @click="switchTheme('light')">
                  <Icon name="sun-line" :size="18" />
                  <span>亮色</span>
                </button>
                <button class="theme-card" :class="{ on: themePref === 'system' }" @click="switchTheme('system')">
                  <Icon name="computer-line" :size="18" />
                  <span>跟随系统</span>
                </button>
              </div>
            </div>
            <div class="row">
              <div class="row-text">
                <b>恢复默认布局</b>
                <span>重置会话侧栏宽度</span>
              </div>
              <button class="ghost-btn" @click="resetSidebarWidth">重置</button>
            </div>
          </div>

          <h3 class="grp-title">字体</h3>
          <div class="group">
            <div class="row col">
              <div class="row-text">
                <b>字号</b>
                <span>全局缩放界面文字，立即生效（当前 {{ Math.round(fontSettings.scale * 100) }}%）</span>
              </div>
              <div class="theme-cards">
                <button
                  v-for="o in FONT_SCALE_OPTIONS"
                  :key="o.label"
                  class="theme-card"
                  :class="{ on: fontSettings.scale === o.value }"
                  @click="applyFontSettings({ scale: o.value })"
                >
                  <span>{{ o.label }}</span>
                </button>
              </div>
            </div>
            <div class="row">
              <div class="row-text">
                <b>界面字体</b>
                <span>留空使用默认；填 CSS font-family 值，如 "Microsoft YaHei"</span>
              </div>
              <input
                class="txt"
                :value="fontSettings.family"
                placeholder="默认"
                spellcheck="false"
                @change="applyFontSettings({ family: ($event.target as HTMLInputElement).value })"
              />
            </div>
            <div class="row">
              <div class="row-text">
                <b>等宽字体</b>
                <span>代码、diff 与文件预览使用；留空默认 Consolas</span>
              </div>
              <input
                class="txt"
                :value="fontSettings.monoFamily"
                placeholder="默认"
                spellcheck="false"
                @change="applyFontSettings({ monoFamily: ($event.target as HTMLInputElement).value })"
              />
            </div>
            <div class="row">
              <div class="row-text">
                <b>恢复默认字体</b>
                <span>字号 100%，清空自定义字体族</span>
              </div>
              <button class="ghost-btn" @click="applyFontSettings({ scale: 1, family: '', monoFamily: '' })">重置</button>
            </div>
          </div>
        </template>

        <!-- ============ 模型设置 ============ -->
        <template v-else-if="pane === 'models'">
          <h2>模型设置</h2>
          <p class="pane-sub">
            默认模型作用于新会话，当前会话可在输入卡片中临时切换；密钥、自定义供应商与模型能力配置在
            <b>供应商与密钥</b> 页。
          </p>

          <div class="group">
            <div class="row">
              <div class="row-text">
                <b>当前默认</b>
                <span class="mono">{{ defaultModelKey === "/" ? "未设置" : defaultModelKey }}</span>
              </div>
              <select
                class="sel"
                :value="settings.defaultThinkingLevel ?? 'off'"
                title="默认思考等级"
                @change="setSetting('defaultThinkingLevel', ($event.target as HTMLSelectElement).value)"
              >
                <option v-for="l in thinkingLevels" :key="l" :value="l">思考：{{ l }}</option>
              </select>
            </div>
          </div>

          <h3 class="grp-title">设为默认模型（显示前 {{ filteredModels.length }} 个）</h3>
          <div class="search-row">
            <Icon name="search-line" :size="14" />
            <input v-model="modelQuery" placeholder="搜索模型，如 minimax / claude / kimi" />
          </div>
          <div v-if="modelsLoading" class="state">加载中…</div>
          <div v-else class="model-list">
            <div
              v-for="m in filteredModels"
              :key="m.provider + '/' + m.id"
              class="model-row"
              :class="{ on: `${m.provider}/${m.id}` === defaultModelKey }"
              @click="setDefaultModel(m)"
            >
              <Icon v-if="`${m.provider}/${m.id}` === defaultModelKey" name="check-line" :size="14" class="ok" />
              <div class="model-name">
                <b>{{ m.id }}</b>
                <span>{{ m.provider }}</span>
              </div>
              <span v-if="m.reasoning" class="type-badge">推理</span>
            </div>
            <div v-if="!filteredModels.length" class="state">没有匹配的模型</div>
          </div>
        </template>

        <!-- ============ 供应商与密钥（内嵌完整管理页） ============ -->
        <template v-else-if="pane === 'providers'">
          <ProvidersView :bus="bus" />
        </template>

        <!-- ============ 键盘快捷键 ============ -->
        <template v-else-if="pane === 'shortcuts'">
          <h2>键盘快捷键</h2>
          <p class="pane-sub">当前版本的固定快捷键。</p>
          <div class="group">
            <div v-for="s in shortcuts" :key="s.desc" class="row">
              <div class="row-text"><b>{{ s.desc }}</b></div>
              <span class="kbd-row">
                <kbd v-for="(k, i) in s.keys" :key="i">{{ k }}</kbd>
              </span>
            </div>
          </div>
        </template>

        <!-- ============ 记忆 ============ -->
        <template v-else-if="pane === 'memory'">
          <h2>记忆</h2>
          <p class="pane-sub">
            全局 AGENTS.md 会作为长期上下文注入每一次会话，适合记录你的偏好、规范与常用约定。项目级记忆请编辑项目根目录的 AGENTS.md。
          </p>
          <div class="group">
            <div class="row">
              <div class="row-text">
                <b class="mono">{{ memoryPath || "加载中…" }}</b>
                <span>{{ memoryExists ? "已创建" : "尚未创建，保存后自动创建" }}</span>
              </div>
              <button class="dark-btn" :disabled="memorySaving" @click="saveMemory">
                <Icon name="save-3-line" :size="14" />{{ memorySaving ? "保存中…" : "保存" }}
              </button>
            </div>
          </div>
          <textarea
            v-model="memoryText"
            class="memory-editor"
            placeholder="# 我的记忆&#10;&#10;- 偏好简洁的提交信息（Conventional Commits，中文）&#10;- 测试优先：改动后先跑 pnpm test"
            spellcheck="false"
          ></textarea>
        </template>

        <!-- ============ 插件 / MCP / 技能（内嵌完整管理页） ============ -->
        <ToolsView
          v-else-if="pane === 'plugins' || pane === 'mcp' || pane === 'skills'"
          :kind="pane"
          :bus="bus"
        />

        <!-- ============ 命令 ============ -->
        <template v-else-if="pane === 'commands'">
          <h2>命令</h2>
          <p class="pane-sub">
            技能在输入框中以 <code>/skill:名称</code> 调用（开关见「常规 · 技能斜杠命令」）；插件也可注册命令（pi
            未提供枚举接口，可在插件页查看源码）。
          </p>
          <h3 class="grp-title">可用命令（{{ commandList.length }}）</h3>
          <div class="group">
            <div v-for="s in commandList" :key="s.name" class="row" :class="{ dim: !s.enabled }">
              <div class="row-text">
                <b class="mono">/skill:{{ s.name }}</b>
                <span class="clamp">{{ s.description || "—" }}</span>
              </div>
              <span class="badge" :class="s.enabled ? 'ok2' : 'warn'">{{ s.enabled ? "可用" : "已停用" }}</span>
            </div>
            <div v-if="!commandList.length" class="state">还没有可用命令，先安装技能或插件</div>
          </div>
        </template>

        <!-- ============ 使用统计 ============ -->
        <template v-else-if="pane === 'usage'">
          <header class="pane-head">
            <div>
              <h2>使用统计</h2>
              <p class="pane-sub">
                来自本地会话记录（{{ usage?.scanned_sessions ?? 0 }} 个会话）
              </p>
            </div>
            <button class="ghost-btn" title="刷新" @click="ensureUsage(true)">
              <Icon name="refresh-line" :size="14" />
            </button>
          </header>

          <div v-if="usageLoading && !usage" class="state">统计中，大会话历史可能需要几秒…</div>
          <template v-else-if="usage">
            <div class="stat-cards">
              <div class="stat-card">
                <span class="stat-num">{{ usage.scanned_sessions }}</span>
                <span class="stat-label">会话</span>
              </div>
              <div class="stat-card">
                <span class="stat-num">{{ usage.messages.user }}</span>
                <span class="stat-label">用户消息</span>
              </div>
              <div class="stat-card">
                <span class="stat-num">{{ usage.messages.assistant }}</span>
                <span class="stat-label">助手回复</span>
              </div>
              <div class="stat-card">
                <span class="stat-num">{{ fmtTokens(usage.tokens.total) }}</span>
                <span class="stat-label">总 Tokens</span>
              </div>
              <div v-if="usage.cost > 0" class="stat-card">
                <span class="stat-num">${{ usage.cost.toFixed(2) }}</span>
                <span class="stat-label">累计费用</span>
              </div>
            </div>

            <h3 class="grp-title">近 30 天活跃</h3>
            <div class="chart">
              <div
                v-for="d in usageDays"
                :key="d.day"
                class="bar-col"
                :title="`${d.day} · ${d.messages} 条回复 · ${fmtTokens(d.tokens)} tokens`"
              >
                <div class="bar" :style="{ height: Math.max(3, (d.tokens / maxDayTokens) * 72) + 'px' }"></div>
                <span class="bar-day">{{ d.day.slice(8) }}</span>
              </div>
              <div v-if="!usageDays.length" class="state">暂无数据</div>
            </div>

            <h3 class="grp-title">按模型</h3>
            <div class="group">
              <div v-for="m in usage.by_model.slice(0, 10)" :key="m.key" class="row">
                <div class="row-text">
                  <b class="mono">{{ m.key }}</b>
                  <span>{{ m.sessions }} 会话 · {{ m.messages }} 次调用</span>
                </div>
                <span class="tok">{{ fmtTokens(m.tokens) }}</span>
              </div>
              <div v-if="!usage.by_model.length" class="state">暂无数据</div>
            </div>

            <h3 class="grp-title">消耗最多的会话</h3>
            <div class="group">
              <div v-for="s in usage.top_sessions" :key="s.session_id" class="row">
                <div class="row-text">
                  <b>{{ s.name || baseName(s.cwd) }}</b>
                  <span class="mono clamp">{{ s.model ?? "?" }} · {{ s.messages }} 条消息 · {{ s.created_at.slice(0, 10) }}</span>
                </div>
                <span class="tok">{{ fmtTokens(s.tokens) }}</span>
              </div>
              <div v-if="!usage.top_sessions.length" class="state">暂无数据</div>
            </div>
          </template>
        </template>

        <!-- ============ 云同步（内容由宿主注入；桌面端为 SyncSettings） ============ -->
        <template v-else-if="pane === 'sync'">
          <h2>云同步</h2>
          <p class="pane-sub">会话实时上云，Web 端登录同一账号即可查看并远程控制（发消息 / 停止）。</p>
          <slot name="sync">
            <div class="state">当前环境不支持云同步</div>
          </slot>
        </template>

        <!-- ============ 引导 ============ -->
        <template v-else-if="pane === 'guide'">
          <h2>引导</h2>
          <p class="pane-sub">几步完成 PiDock 初始配置，随时可以回来查看。</p>
          <div class="steps">
            <div v-for="(st, i) in guideSteps" :key="i" class="step" :class="{ done: st.done }">
              <div class="step-dot">
                <Icon :name="st.done ? 'check-line' : 'arrow-right-line'" :size="13" />
              </div>
              <div class="step-text">
                <b>{{ st.title }}</b>
                <span>{{ st.desc }}</span>
              </div>
              <button class="ghost-btn" @click="pane = st.id">{{ st.cta }}</button>
            </div>
          </div>
          <div class="group">
            <div class="row">
              <div class="row-text">
                <b>云同步</b>
                <span>登录账号后会话实时上云，网页端可远程控制。</span>
              </div>
              <button class="ghost-btn" @click="pane = 'sync'">去开启</button>
            </div>
          </div>
        </template>
      </section>
    </div>
  </div>
</template>

<style scoped>
.settings {
  /* 全屏：盖住整个窗口（含会话侧栏与标题栏），窗口控制由页头按钮补齐 */
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg);
}
.head {
  display: flex;
  align-items: center;
  gap: 14px;
  height: 46px;
  padding: 0 16px;
  border-bottom: 1px solid var(--pd-border-soft);
  background: var(--pd-bg-panel);
  flex: none;
  user-select: none;
}
.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: none;
  border: none;
  color: var(--pd-text-3);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  padding: 6px 8px;
  border-radius: 8px;
}
.back-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.head h1 {
  margin: 0;
  font-size: calc(15px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
}
.agent-dir {
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
  font-family: var(--pd-mono);
  cursor: pointer;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.agent-dir:hover { color: var(--pd-text-2); }
.flex-sp { flex: 1; }
.win-sep {
  width: 1px;
  height: 16px;
  background: var(--pd-border);
  margin: 0 2px;
}
.wbtn {
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
.wbtn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.wbtn.close:hover { background: #e04444; color: #fff; }

.body {
  flex: 1;
  display: flex;
  min-height: 0;
}
.nav {
  width: 208px;
  flex: none;
  overflow-y: auto;
  padding: 14px 10px 20px;
  border-right: 1px solid var(--pd-border-soft);
  background: var(--pd-bg-panel);
}
.nav-sec { margin-bottom: 14px; }
.nav-sec-title {
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  padding: 0 10px 6px;
}
.nav-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  background: none;
  border: none;
  border-radius: 8px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  padding: 7px 10px;
  cursor: pointer;
  text-align: left;
}
.nav-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.nav-item.on {
  background: var(--pd-bg-active);
  color: var(--pd-text);
  font-weight: 600;
}
.nav-item svg { color: var(--pd-text-3); flex: none; }
.nav-item.on svg { color: var(--pd-accent); }
.nav-label { flex: 1; min-width: 0; }
.nav-count {
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  background: var(--pd-bg-card);
  border-radius: 999px;
  padding: 0 7px;
}

.content {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 26px 34px 48px;
}
/* 内嵌完整管理页（插件/MCP/技能/供应商）时：交给子页面自己滚动与留白 */
.content.embed {
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
}
/* 表单/偏好页：内容列限宽居中，宽窗口下不贴左散开 */
.content:not(.embed) > * {
  max-width: 860px;
  margin-left: auto;
  margin-right: auto;
}
.content > h2 {
  margin: 0;
  font-size: calc(19px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
}
.pane-sub {
  margin: 6px 0 0;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.6;
}
.pane-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.grp-title {
  margin: 26px 0 10px;
  font-size: calc(13.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text-2);
  display: flex;
  align-items: center;
  gap: 12px;
}

.group {
  margin-top: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 4px 16px;
}
.row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 0;
}
.row + .row { border-top: 1px solid var(--pd-border-soft); }
.row.col { flex-wrap: wrap; }
.row.dim { opacity: 0.55; }
.row-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.row-text b {
  font-size: calc(13.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
}
.row-text span {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.5;
}
.row-text span.mono { font-family: var(--pd-mono); font-size: calc(11.5px * var(--pd-font-scale)); word-break: break-all; }
.clamp {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mono { font-family: var(--pd-mono); }

.sel {
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 6px 10px;
  cursor: pointer;
}
.sel:focus { outline: none; border-color: var(--pd-accent); }
.sel option { background: var(--pd-bg-raised); }

/* 分段选择器（代理模式） */
.seg {
  display: inline-flex;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 2px;
  gap: 2px;
}
.seg button {
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-3);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 5px 12px;
  cursor: pointer;
}
.seg button:hover { color: var(--pd-text); }
.seg button.on { background: var(--pd-bg-active); color: var(--pd-text); font-weight: 600; }

/* 单行文本输入（代理地址/绕过列表/证书路径） */
.txt {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: calc(12.5px * var(--pd-font-scale));
  font-family: var(--pd-mono);
  padding: 7px 10px;
}
.txt:focus { outline: none; border-color: var(--pd-accent); }
.txt::placeholder { color: var(--pd-text-4); }

.ghost-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 6px 12px;
  cursor: pointer;
  flex: none;
}
.ghost-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.dark-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--pd-text);
  color: var(--pd-bg);
  border: none;
  border-radius: 9px;
  padding: 7px 14px;
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
  flex: none;
}
.dark-btn:hover { background: var(--pd-accent); color: #1a1a1a; }
.dark-btn:disabled { opacity: 0.6; cursor: default; }

.badge {
  font-size: calc(10.5px * var(--pd-font-scale));
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
}
.badge.acc { color: var(--pd-accent-text); background: var(--pd-accent-soft); }
.badge.ok2 { color: var(--pd-green-text); background: var(--pd-green-soft); }
.badge.warn { color: var(--pd-yellow-text); background: var(--pd-yellow-soft); }
.type-badge {
  font-size: calc(10px * var(--pd-font-scale));
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
  font-family: var(--pd-mono);
}

/* 开关（与 ToolsView 一致） */
.switch {
  position: relative;
  flex: none;
  width: 42px;
  height: 23px;
  cursor: pointer;
}
.switch input { position: absolute; opacity: 0; width: 0; height: 0; }
.slider {
  position: absolute;
  inset: 0;
  border-radius: 999px;
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border);
  transition: background 0.15s, border-color 0.15s;
}
.slider::before {
  content: "";
  position: absolute;
  width: 17px;
  height: 17px;
  border-radius: 50%;
  top: 2px;
  left: 2px;
  background: var(--pd-text-3);
  transition: transform 0.15s, background 0.15s;
}
.switch input:checked + .slider {
  background: var(--pd-green);
  border-color: var(--pd-green);
}
.switch input:checked + .slider::before {
  transform: translateX(19px);
  background: #fff;
}

/* 外观主题卡片 */
.theme-cards { display: flex; gap: 10px; width: 100%; }
.theme-card {
  flex: 1;
  max-width: 180px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  background: var(--pd-bg-raised);
  border: 1.5px solid var(--pd-border);
  border-radius: 12px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  padding: 18px 0 14px;
  cursor: pointer;
}
.theme-card:hover { border-color: var(--pd-text-4); }
.theme-card.on { border-color: var(--pd-accent); color: var(--pd-accent-text); background: var(--pd-accent-soft); }

/* 模型页 */
.search-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  padding: 8px 12px;
  color: var(--pd-text-4);
}
.search-row:focus-within { border-color: var(--pd-accent); }
.search-row input {
  flex: 1;
  background: none;
  border: none;
  outline: none;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
}
.search-row input::placeholder { color: var(--pd-text-4); }
.model-list {
  margin-top: 10px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 4px 8px;
  max-height: 420px;
  overflow-y: auto;
}
.model-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
}
.model-row:hover { background: var(--pd-bg-hover); }
.model-row.on { background: var(--pd-bg-active); }
.model-row .ok { color: var(--pd-green); flex: none; }
.model-name {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.model-name b {
  font-size: calc(13px * var(--pd-font-scale));
  color: var(--pd-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.model-name span { font-size: calc(11.5px * var(--pd-font-scale)); color: var(--pd-text-4); flex: none; }

/* 快捷键 */
.kbd-row { display: inline-flex; gap: 6px; }
kbd {
  font-family: var(--pd-mono);
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-2);
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-bottom-width: 2px;
  border-radius: 6px;
  padding: 2px 8px;
}

/* 记忆编辑器 */
.memory-editor {
  /* 全局样式把 textarea 设为 inline-block，会导致 margin:auto 失效 → 显式块级化 */
  display: block;
  width: 100%;
  box-sizing: border-box;
  margin-top: 14px;
  min-height: 380px;
  resize: vertical;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  color: var(--pd-text);
  font-family: var(--pd-mono);
  font-size: calc(12.5px * var(--pd-font-scale));
  line-height: 1.7;
  padding: 14px 16px;
}
.memory-editor:focus { outline: none; border-color: var(--pd-accent); }
.memory-editor::placeholder { color: var(--pd-text-4); }

/* 统计 */
.stat-cards {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 10px;
  margin-top: 16px;
}
.stat-card {
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.stat-num { font-size: calc(20px * var(--pd-font-scale)); font-weight: 700; color: var(--pd-text); }
.stat-label { font-size: calc(11.5px * var(--pd-font-scale)); color: var(--pd-text-3); }
.chart {
  display: flex;
  align-items: flex-end;
  gap: 3px;
  margin-top: 12px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 14px 14px 8px;
  min-height: 110px;
}
.bar-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  min-width: 0;
}
.bar {
  width: 100%;
  max-width: 22px;
  border-radius: 3px 3px 0 0;
  background: var(--pd-accent);
  opacity: 0.75;
}
.bar-col:hover .bar { opacity: 1; }
.bar-day { font-size: calc(9px * var(--pd-font-scale)); color: var(--pd-text-4); }
.tok {
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text-2);
  font-family: var(--pd-mono);
  flex: none;
}

/* 引导 */
.steps { margin-top: 16px; display: flex; flex-direction: column; gap: 10px; }
.step {
  display: flex;
  align-items: center;
  gap: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 14px 16px;
}
.step.done { border-color: var(--pd-green-soft); }
.step-dot {
  width: 26px;
  height: 26px;
  flex: none;
  display: grid;
  place-items: center;
  border-radius: 50%;
  background: var(--pd-bg-hover);
  color: var(--pd-text-3);
}
.step.done .step-dot { background: var(--pd-green-soft); color: var(--pd-green-text); }
.step-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.step-text b { font-size: calc(13.5px * var(--pd-font-scale)); color: var(--pd-text); }
.step-text span { font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-3); line-height: 1.5; }

.state {
  color: var(--pd-text-4);
  font-size: calc(13px * var(--pd-font-scale));
  padding: 18px 4px;
  line-height: 1.7;
}
.notice {
  margin-bottom: 14px;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 8px;
  padding: 8px 12px;
}
code {
  font-family: var(--pd-mono);
  font-size: calc(11.5px * var(--pd-font-scale));
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1px 5px;
}
</style>
