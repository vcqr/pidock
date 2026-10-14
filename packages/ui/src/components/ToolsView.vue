<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { FILE_PICKER, type DataBus } from "../databus.js";
import Icon from "./Icon.vue";
import { appConfirm } from "../confirm.js";
import MdContent from "./MdContent.vue";
import McpServerForm, { type McpFormState } from "./McpServerForm.vue";

/**
 * Full main-area manager page for 插件 / 技能 / MCP，
 * shown when the corresponding sidebar nav item is clicked.
 */
const props = defineProps<{ kind: "plugins" | "skills" | "mcp"; bus: DataBus }>();

interface Row {
  id: string;
  name: string;
  badge?: string;
  sub: string;
  type?: string;
  /** raw mcp.json entry, kept for form prefill */
  entry?: any;
  enabled?: boolean;
  togglable: boolean;
}


const meta = {
  plugins: {
    title: "插件",
    sub: "来源：~/.pi/agent/extensions（全局）与项目 .pi/extensions，改动对新会话生效。",
    placeholder: "搜索已安装的插件",
    icon: "puzzle-2-line",
    empty: "还没有安装插件。把 .ts 扩展文件放进 ~/.pi/agent/extensions 即可。",
  },
  skills: {
    title: "技能",
    sub: "来源：~/.pi/agent/skills（全局）与项目 .pi/skills，改动对新会话生效。",
    placeholder: "搜索已安装的技能",
    icon: "magic-line",
    empty: "还没有安装技能。把含 SKILL.md 的目录放进 ~/.pi/agent/skills 即可。",
  },
  mcp: {
    title: "MCP 服务器",
    sub: "pi 的 MCP 由扩展提供，这里管理 mcp.json 中的服务器配置。",
    placeholder: "搜索 MCP 服务器",
    icon: "plug-line",
    empty: "还没有服务器配置，点右上角「添加」创建。",
  },
} as const;

const loading = ref(true);
const notice = ref<string | null>(null);
const q = ref("");
const tab = ref("全部");
const rows = ref<Row[]>([]);

// ---- mcp form state ----
const mcpPath = ref("");
const rawConfig = ref<any>(null);
const draftProbe = ref<ProbeState | null>(null); // 「测试连接」对当前表单草稿（未保存）的结果
const draftProbing = ref(false);

function emptyMcpForm(): McpFormState {
  return {
    name: "",
    type: "stdio",
    command: "",
    args: "",
    url: "",
    env: "",
    headers: "",
    description: "",
    timeout: "",
    cwd: "",
    advancedOpen: false,
  };
}

let flashTimer: ReturnType<typeof setTimeout> | undefined;
function flash(msg: string): void {
  notice.value = msg;
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (notice.value = null), 2500);
}

function scopeBadge(scope: unknown): string | undefined {
  if (scope === "global") return "全局";
  if (scope === "project") return "项目";
  return scope ? String(scope) : undefined;
}

// ---- 安装（技能 / 插件）：从网址下载 或 本地文件导入 ----
const pickFile = inject(FILE_PICKER, null);
const installOpen = ref(false);
const installMode = ref<"url" | "local">("url");
const installUrl = ref("");
const installSrcPath = ref("");
const installName = ref("");
const installBusy = ref(false);

function openInstall(): void {
  installMode.value = "url";
  installUrl.value = "";
  installSrcPath.value = "";
  installName.value = "";
  installOpen.value = true;
}

async function pickInstallFile(): Promise<void> {
  const p = await pickFile?.();
  if (p) installSrcPath.value = p;
}

async function submitInstall(): Promise<void> {
  const kind = props.kind;
  const url = installUrl.value.trim();
  const srcPath = installSrcPath.value.trim();
  const name = installName.value.trim() || undefined;
  if (installMode.value === "url" && !url) return flash("请填写下载网址");
  if (installMode.value === "local" && !srcPath) return flash("请选择文件");
  installBusy.value = true;
  try {
    if (kind === "plugins") {
      const r = await props.bus.request("config.extensions.install", installMode.value === "url" ? { url, filename: name } : { srcPath });
      flash(`已安装「${String(r.file).split(/[\\/]/).pop()}」，对新会话生效`);
    } else {
      const r = await props.bus.request("config.skills.install", installMode.value === "url" ? { url, name } : { srcPath, name });
      flash(`已安装技能「${r.name}」，对新会话生效`);
    }
    installOpen.value = false;
    await load();
  } catch (err) {
    flash(String(err));
  } finally {
    installBusy.value = false;
  }
}

function parseMcp(config: any): Row[] {
  const servers = serversContainer(config, false);
  if (!servers) return [];
  return Object.entries(servers).map(([name, raw]: [string, any]) => {
    const v = raw && typeof raw === "object" ? raw : {};
    const rawType = v.type ? String(v.type) : v.command ? "stdio" : v.url ? "http" : "stdio";
    const type =
      rawType === "stdio" ? "STDIO" : rawType === "streamable-http" ? "STREAMABLE-HTTP" : rawType.toUpperCase();
    const detail = v.url
      ? String(v.url)
      : [v.command, ...(Array.isArray(v.args) ? v.args : [])].filter(Boolean).join(" ");
    return {
      id: name,
      name,
      sub: detail || "—",
      type,
      entry: v,
      enabled: v.enabled !== false,
      togglable: true,
    };
  });
}

/** the object holding servers inside mcp.json; creates the container when create=true */
function serversContainer(config: any, create: boolean): Record<string, any> | null {
  if (!config || typeof config !== "object") return null;
  if (config.mcpServers && typeof config.mcpServers === "object") return config.mcpServers;
  if (config.servers && typeof config.servers === "object") return config.servers;
  if (!create) return null;
  config.mcpServers = {};
  return config.mcpServers;
}

async function load(): Promise<void> {
  loading.value = true;
  try {
    if (props.kind === "plugins") {
      const r = await props.bus.request("config.extensions.list", {});
      rows.value = (r.extensions ?? []).map((x: any) => ({
        id: x.file,
        name: x.name,
        badge: scopeBadge(x.scope),
        sub: x.file,
        enabled: !!x.enabled,
        togglable: true,
      }));
    } else if (props.kind === "skills") {
      const r = await props.bus.request("config.skills.list", {});
      rows.value = (r.skills ?? []).map((x: any) => ({
        id: x.path,
        name: x.name,
        badge: scopeBadge(x.scope),
        sub: x.description || x.path,
        enabled: !!x.enabled,
        togglable: true,
      }));
    } else {
      const r = await props.bus.request("config.mcp.get");
      rawConfig.value = r.config ?? {};
      mcpPath.value = r.path ?? "";
      rows.value = parseMcp(rawConfig.value);
      void loadApprovals();
    }
  } catch (err) {
    flash(String(err));
  } finally {
    loading.value = false;
  }
}
onMounted(load);

// switching kinds without unmount keeps this component alive — reload per kind
watch(
  () => props.kind,
  () => {
    q.value = "";
    tab.value = "全部";
    closeDetail();
    closeExtDetail();
    closeMcpDetail();
    probeCache.value = {};
    void load();
  },
);

// ---- skill detail (源文件 + SKILL.md 渲染) ----
const detail = ref<Row | null>(null);
const detailDir = ref("");
const detailFiles = ref<Array<{ file: string; size: number; binary?: boolean }>>([]);
const activeFile = ref("SKILL.md");
const fileText = ref("");
const detailLoading = ref(false);
const fileLoading = ref(false);
const binaryFile = ref(false);

const isMarkdown = computed(() => /\.md$/i.test(activeFile.value));

/** markdown front-matter (--- blocks) is metadata, not body — hide it in the reader */
function stripFrontMatter(text: string): string {
  if (!text.startsWith("---")) return text;
  const end = text.indexOf("\n---", 3);
  if (end < 0) return text;
  return text.slice(end + 4).replace(/^\s+/, "");
}

async function openDetail(it: Row): Promise<void> {
  detail.value = it;
  activeFile.value = "SKILL.md";
  fileText.value = "";
  await loadDetail();
}

function closeDetail(): void {
  detail.value = null;
  detailFiles.value = [];
  fileText.value = "";
}

async function loadDetail(): Promise<void> {
  const it = detail.value;
  if (!it) return;
  detailLoading.value = true;
  try {
    const r = await props.bus.request("config.skills.files", { path: it.id });
    detailFiles.value = r.files ?? [];
    detailDir.value = r.dir ?? "";
    await readDetailFile(activeFile.value);
  } catch (err) {
    flash(String(err));
  } finally {
    detailLoading.value = false;
  }
}

async function readDetailFile(file: string): Promise<void> {
  const it = detail.value;
  if (!it) return;
  fileLoading.value = true;
  activeFile.value = file;
  binaryFile.value = false;
  try {
    const r = await props.bus.request("config.skills.read", { path: it.id, file });
    const text = r.text ?? "";
    fileText.value = isMarkdown.value ? stripFrontMatter(text) : text;
  } catch (err) {
    if (String(err).includes("binary_file")) {
      // host 拒绝返回二进制内容，占位提示而不是乱码
      binaryFile.value = true;
      fileText.value = "";
    } else {
      fileText.value = "无法读取文件：" + String(err);
    }
  } finally {
    fileLoading.value = false;
  }
}

// ---- plugin (扩展) detail：单文件源码阅读器 ----
const extDetail = ref<Row | null>(null);
const extText = ref("");
const extLoading = ref(false);
const extBinary = ref(false);

function baseName(p: string): string {
  const norm = p.replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1) || p;
}

/** 用代码围栏走 MdContent 的 markdown-it + highlight.js 管线，拿到 TS 语法高亮 */
const extFenced = computed(() => {
  if (!extText.value) return "";
  const body = extText.value.endsWith("\n") ? extText.value : extText.value + "\n";
  return "````ts\n" + body + "````";
});

async function openExtDetail(it: Row): Promise<void> {
  extDetail.value = it;
  extText.value = "";
  extBinary.value = false;
  await loadExtDetail();
}

function closeExtDetail(): void {
  extDetail.value = null;
  extText.value = "";
}

async function loadExtDetail(): Promise<void> {
  const it = extDetail.value;
  if (!it) return;
  extLoading.value = true;
  try {
    const r = await props.bus.request("config.extensions.read", { file: it.id });
    extText.value = r.text ?? "";
    extBinary.value = false;
  } catch (err) {
    if (String(err).includes("binary_file")) {
      extBinary.value = true;
      extText.value = "";
    } else {
      flash(String(err));
    }
  } finally {
    extLoading.value = false;
  }
}

const hasScope = computed(() => rows.value.some((r) => r.badge === "全局" || r.badge === "项目"));
const tabs = computed<string[]>(() =>
  props.kind === "mcp" || !hasScope.value ? [] : ["全部", "全局", "项目"],
);

const filtered = computed(() => {
  let list = rows.value;
  if (tab.value === "全局") list = list.filter((r) => r.badge === "全局");
  else if (tab.value === "项目") list = list.filter((r) => r.badge === "项目");
  const s = q.value.trim().toLowerCase();
  if (s) list = list.filter((r) => `${r.name} ${r.sub}`.toLowerCase().includes(s));
  return list;
});

async function toggle(it: Row): Promise<void> {
  try {
    if (props.kind === "mcp") {
      // pi 约定：enabled: false = 保留条目但不连接；启用时删除该字段保持文件干净
      const container = serversContainer(rawConfig.value, false);
      if (!container || !(it.name in container)) return;
      const entry = container[it.name];
      const next = entry.enabled === false;
      if (next) delete entry.enabled;
      else entry.enabled = false;
      await props.bus.request("config.mcp.set", { config: rawConfig.value });
      it.enabled = next;
      flash(next ? `已启用「${it.name}」` : `已停用「${it.name}」，不再连接`);
    } else if (props.kind === "plugins") {
      await props.bus.request("config.extensions.toggle", { file: it.id, enabled: !it.enabled });
      it.enabled = !it.enabled;
      flash("改动对新会话生效");
    } else {
      await props.bus.request("config.skills.toggle", { path: it.id, enabled: !it.enabled });
      it.enabled = !it.enabled;
      flash("改动对新会话生效");
    }
  } catch (err) {
    flash(String(err));
  }
}

// ---- mcp form ----

function envToLines(env: unknown): string {
  if (!env || typeof env !== "object") return "";
  return Object.entries(env as Record<string, unknown>)
    .map(([k, v]) => `${k}=${v ?? ""}`)
    .join("\n");
}

function parseLines(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
}

function parseEnv(text: string): Record<string, string> | undefined {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const i = t.indexOf("=");
    if (i <= 0) continue;
    out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
  }
  return Object.keys(out).length ? out : undefined;
}

/** 按 shell 规则切分整行命令（引号内空格不切分），用于「命令」框直接粘贴 "obscura mcp" 这类完整命令行 */
function splitCommandLine(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: '"' | "'" | null = null;
  for (const ch of line.trim()) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === " " || ch === "\t") {
      if (cur) {
        out.push(cur);
        cur = "";
      }
    } else {
      cur += ch;
    }
  }
  if (cur) out.push(cur);
  return out;
}

/** mcp.json 条目 → 表单状态（详情页与编辑共用） */
function formStateFromEntry(name: string, v: any): McpFormState {
  const e = v && typeof v === "object" ? v : {};
  const env = envToLines(e.env);
  return {
    name,
    type: e.command ? "stdio" : e.url ? (e.type === "streamable-http" ? "streamable-http" : "http") : "stdio",
    command: typeof e.command === "string" ? e.command : "",
    args: Array.isArray(e.args) ? e.args.join("\n") : "",
    url: typeof e.url === "string" ? e.url : "",
    env,
    headers: envToLines(e.headers),
    description: typeof e.description === "string" ? e.description : "",
    timeout: typeof e.timeout === "number" && Number.isFinite(e.timeout) ? String(e.timeout) : "",
    cwd: typeof e.cwd === "string" ? e.cwd : "",
    // stdio 的超时/工作目录/环境变量都收在高级设置里，有值时展开
    advancedOpen: !!env || !!(typeof e.cwd === "string" && e.cwd) || (typeof e.timeout === "number" && Number.isFinite(e.timeout)),
  };
}

/** 本表单托管的条目键；其余键（oauth/exposure 等手工配置）原样保留 */
const MANAGED_ENTRY_KEYS = ["command", "args", "env", "url", "type", "description", "timeout", "headers", "cwd"];

/** 表单状态 → mcp.json 条目；空值键不写入 */
function entryFromForm(state: McpFormState, oldEntry: any): any {
  const old = oldEntry && typeof oldEntry === "object" ? { ...oldEntry } : {};
  for (const k of MANAGED_ENTRY_KEYS) delete old[k];
  const entry: any = { ...old };
  if (state.description.trim()) entry.description = state.description.trim();
  const sec = Number(state.timeout.trim());
  if (state.timeout.trim() && Number.isFinite(sec) && sec > 0) entry.timeout = sec;
  if (state.type === "stdio") {
    // 「命令」允许整行粘贴（如 "obscura mcp"）：首 token 为可执行文件，其余并入 args
    const parts = splitCommandLine(state.command);
    entry.command = parts[0] ?? "";
    const args = [...parts.slice(1), ...parseLines(state.args)];
    if (args.length) entry.args = args;
    const env = parseEnv(state.env);
    if (env) entry.env = env;
    if (state.cwd.trim()) entry.cwd = state.cwd.trim();
  } else {
    entry.url = state.url.trim();
    entry.type = state.type;
    const headers = parseEnv(state.headers);
    if (headers) entry.headers = headers;
  }
  return entry;
}

/** 共用保存：校验 → 写入容器 → config.mcp.set 落盘 → 重载列表；返回错误信息或 null */
async function saveMcpEntry(state: McpFormState, originalKey: string): Promise<string | null> {
  const name = state.name.trim();
  if (!name) return "名称不能为空";
  const container = serversContainer(rawConfig.value ?? {}, true)!;
  if (originalKey !== name && container[name]) return `服务器「${name}」已存在`;
  if (originalKey && !container[originalKey]) return `服务器「${originalKey}」不存在，请刷新后重试`;
  if (state.type === "stdio" && !state.command.trim()) return "命令不能为空";
  if (state.type !== "stdio" && !state.url.trim()) return "URL 不能为空";
  container[name] = entryFromForm(state, originalKey ? container[originalKey] : undefined);
  if (originalKey && originalKey !== name) delete container[originalKey];
  try {
    await props.bus.request("config.mcp.set", { config: rawConfig.value });
  } catch (err) {
    return String(err);
  }
  await load();
  return null;
}

function openMcpAdd(): void {
  // 添加与编辑共用详情页；isNew 模式下 mcpDetailName 为空串，保存后回填真名
  mcpDetailName.value = "";
  mcpDetailIsNew.value = true;
  mcpDetailTab.value = "general";
  toolFilter.value = "";
  expandedTools.value = new Set();
  detailForm.value = emptyMcpForm();
  draftProbe.value = null;
}

/** 「测试连接」：用当前表单草稿（未保存）探测 */
async function probeDraft(): Promise<void> {
  if (draftProbing.value) return;
  if (detailForm.value.type === "stdio" && !detailForm.value.command.trim()) return flash("请先填写命令");
  if (detailForm.value.type !== "stdio" && !detailForm.value.url.trim()) return flash("请先填写 URL");
  draftProbing.value = true;
  draftProbe.value = { status: "probing" };
  try {
    const r = await props.bus.request("config.mcp.probe", {
      name: detailForm.value.name.trim() || "draft",
      entry: entryFromForm(detailForm.value, undefined),
    });
    draftProbe.value = probeStateFrom(r);
  } catch (err) {
    draftProbe.value = { status: "error", error: String(err) };
  } finally {
    draftProbing.value = false;
  }
}

// ---- mcp 服务器详情页（通用 / 工具标签 + 探测 + 自动批准） ----

interface ProbeState {
  status: "probing" | "ok" | "error";
  version?: string;
  serverName?: string;
  tools?: Array<{ name: string; description: string; inputSchema?: unknown }>;
  error?: string;
  needsAuth?: boolean;
}

function probeStateFrom(r: any): ProbeState {
  if (r?.ok) {
    return {
      status: "ok",
      version: r.serverInfo?.version ? String(r.serverInfo.version) : "",
      serverName: r.serverInfo?.name ? String(r.serverInfo.name) : "",
      tools: Array.isArray(r.tools) ? r.tools : [],
    };
  }
  return { status: "error", error: String(r?.error ?? "连接失败"), needsAuth: !!r?.needsAuth };
}

/** 探测结果缓存（进程内），键为服务器名 */
const probeCache = ref<Record<string, ProbeState>>({});
const probeBusyName = ref("");

async function probeServer(name: string): Promise<void> {
  if (probeBusyName.value) return;
  probeBusyName.value = name;
  const probing: Record<string, ProbeState> = { ...probeCache.value };
  probing[name] = { status: "probing" };
  probeCache.value = probing;
  try {
    const r = await props.bus.request("config.mcp.probe", { name });
    const next: Record<string, ProbeState> = { ...probeCache.value };
    next[name] = probeStateFrom(r);
    probeCache.value = next;
  } catch (err) {
    const next: Record<string, ProbeState> = { ...probeCache.value };
    next[name] = { status: "error", error: String(err) };
    probeCache.value = next;
  } finally {
    probeBusyName.value = "";
  }
}

// ---- 每工具自动批准（pidock settings.json 的 mcpApprovals，缺省 = 自动批准） ----
const approvals = ref<Record<string, Record<string, boolean>>>({});

async function loadApprovals(): Promise<void> {
  try {
    const r = await props.bus.request("pidock.settings.get", {});
    approvals.value = r?.settings?.mcpApprovals ?? {};
  } catch {
    approvals.value = {};
  }
}

function isAutoApproved(server: string, tool: string): boolean {
  return approvals.value[server]?.[tool] !== false;
}

async function setAutoApproved(server: string, tool: string, allowed: boolean): Promise<void> {
  const next: Record<string, Record<string, boolean>> = {};
  for (const [s, per] of Object.entries(approvals.value)) next[s] = { ...per };
  const per = { ...(next[server] ?? {}) };
  if (allowed) delete per[tool];
  else per[tool] = false;
  if (Object.keys(per).length) next[server] = per;
  else delete next[server];
  approvals.value = next;
  try {
    await props.bus.request("pidock.settings.set", { mcpApprovals: next });
    flash(allowed ? `「${tool}」已恢复自动批准` : `「${tool}」将不再自动批准，调用时请求确认`);
  } catch (err) {
    flash(String(err));
    await loadApprovals(); // 写盘失败回滚到落盘状态
  }
}

// ---- mcp 详情页状态（添加与编辑共用；isNew = 新建未保存） ----
const mcpDetailName = ref<string | null>(null);
const mcpDetailIsNew = ref(false);
const mcpDetailTab = ref<"general" | "tools">("general");
const detailForm = ref<McpFormState>(emptyMcpForm());
const detailSaving = ref(false);
const toolFilter = ref("");

const mcpDetailRow = computed<Row | null>(() =>
  mcpDetailName.value ? rows.value.find((r) => r.name === mcpDetailName.value) ?? null : null,
);
/** 详情页（新建/编辑）打开时整页替换：隐藏列表页头（标题/路径/搜索框） */
const mcpDetailOpen = computed(() => props.kind === "mcp" && mcpDetailName.value !== null);
const mcpProbeState = computed<ProbeState | undefined>(() =>
  mcpDetailName.value ? probeCache.value[mcpDetailName.value] : undefined,
);
const mcpToolCount = computed(() =>
  mcpProbeState.value?.status === "ok" ? mcpProbeState.value.tools?.length ?? 0 : 0,
);
const connText = computed(() => {
  const s = mcpProbeState.value;
  if (!s || s.status === "probing") return "探测中…";
  if (s.status === "ok") return "已连接";
  return s.needsAuth ? "需要认证" : "未连接";
});
const connClass = computed(() => {
  const s = mcpProbeState.value;
  return s?.status === "ok" ? "on" : s?.needsAuth ? "warn" : "err";
});
const detailTools = computed(() => {
  const s = mcpProbeState.value;
  if (!s || s.status !== "ok" || !s.tools) return [];
  const f = toolFilter.value.trim().toLowerCase();
  return f ? s.tools.filter((t) => `${t.name} ${t.description}`.toLowerCase().includes(f)) : s.tools;
});

/** 展开了参数面板的工具名集合（详情页切换时重置） */
const expandedTools = ref<Set<string>>(new Set());

function toggleToolExpand(name: string): void {
  const next = new Set(expandedTools.value);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  expandedTools.value = next;
}

interface ToolParam {
  name: string;
  type: string;
  description: string;
  required: boolean;
  enums: string[];
  defaultValue?: string;
}

const MAX_ENUM_SHOWN = 16;

/** 把 MCP 工具的 JSON Schema inputSchema 摊平成参数行（仅顶层属性；object/array 类型不再下钻） */
function toolParams(t: { inputSchema?: unknown }): ToolParam[] {
  const schema = t.inputSchema as Record<string, any> | undefined;
  const props = schema?.properties;
  if (!props || typeof props !== "object" || Array.isArray(props)) return [];
  const required: unknown[] = Array.isArray(schema.required) ? schema.required : [];
  return Object.entries(props as Record<string, any>).map(([name, s]) => {
    const type = Array.isArray(s?.type)
      ? s.type.join("|")
      : typeof s?.type === "string"
        ? s.type
        : s?.anyOf || s?.oneOf
          ? "any"
          : "unknown";
    const rawEnum: unknown[] | undefined = Array.isArray(s?.enum) ? (s.enum as unknown[]) : undefined;
    const enums = rawEnum ? rawEnum.slice(0, MAX_ENUM_SHOWN).map((v: unknown) => String(v)) : [];
    const d = s?.default;
    return {
      name,
      type,
      description: typeof s?.description === "string" ? s.description.slice(0, 300) : "",
      required: required.includes(name),
      enums,
      defaultValue: d === undefined ? undefined : typeof d === "string" ? d : JSON.stringify(d),
    };
  });
}

function openMcpDetail(it: Row): void {
  mcpDetailName.value = it.name;
  mcpDetailIsNew.value = false;
  mcpDetailTab.value = "general";
  toolFilter.value = "";
  expandedTools.value = new Set();
  draftProbe.value = null;
  detailForm.value = formStateFromEntry(it.name, it.entry);
  if (!probeCache.value[it.name]) void probeServer(it.name);
}

function closeMcpDetail(): void {
  mcpDetailName.value = null;
  mcpDetailIsNew.value = false;
}

async function saveDetail(): Promise<void> {
  const original = mcpDetailIsNew.value ? "" : mcpDetailName.value ?? "";
  detailSaving.value = true;
  const err = await saveMcpEntry(detailForm.value, original);
  detailSaving.value = false;
  if (err) return flash(err);
  const newName = detailForm.value.name.trim();
  if (mcpDetailIsNew.value) {
    // 新建保存：详情转正为该服务器并直接探测工具
    mcpDetailName.value = newName;
    mcpDetailIsNew.value = false;
    draftProbe.value = null;
    mcpDetailTab.value = "tools";
    void probeServer(newName);
    flash(`已添加「${newName}」，改动对新会话生效`);
    return;
  }
  if (newName !== original) {
    // 改名后行已重建，详情跟随新名，探测缓存一并迁移
    const moved: Record<string, ProbeState> = {};
    for (const [k, v] of Object.entries(probeCache.value)) moved[k === original ? newName : k] = v;
    probeCache.value = moved;
    mcpDetailName.value = newName;
  }
  flash("已保存，改动对新会话生效");
}

async function removeServer(it: Row): Promise<void> {
  if (!(await appConfirm({ title: `删除 MCP 服务器「${it.name}」？`, danger: true }))) return;
  try {
    const container = serversContainer(rawConfig.value, false);
    if (container && it.name in container) {
      delete container[it.name];
      await props.bus.request("config.mcp.set", { config: rawConfig.value });
      flash(`已删除「${it.name}」`);
      if (mcpDetailName.value === it.name) closeMcpDetail();
      await load();
    }
  } catch (err) {
    flash(String(err));
  }
}
</script>

<template>
  <div class="tools-page">
    <div class="wrap">
      <header v-if="!mcpDetailOpen" class="head">
        <div class="title-block">
          <h1>
            {{ meta[kind].title }}
            <span v-if="!loading" class="count">{{ rows.length }}</span>
          </h1>
          <p class="sub">{{ meta[kind].sub }}</p>
          <p v-if="kind === 'mcp' && mcpPath" class="sub mono">{{ mcpPath }}</p>
        </div>
        <div class="head-actions">
          <button class="ghost-btn" title="刷新" @click="load">
            <Icon name="refresh-line" :size="15" />
          </button>
          <button v-if="kind === 'mcp'" class="dark-btn" @click="openMcpAdd">
            <Icon name="add-line" :size="14" />添加
          </button>
          <button v-if="kind === 'plugins' || kind === 'skills'" class="dark-btn" @click="openInstall">
            <Icon name="download-cloud-2-line" :size="14" />安装
          </button>
        </div>
      </header>

      <div v-if="tabs.length" class="tabs">
        <button v-for="t in tabs" :key="t" :class="{ on: tab === t }" @click="tab = t">{{ t }}</button>
      </div>

      <div v-if="!mcpDetailOpen" class="search-row">
        <Icon name="search-line" :size="15" />
        <input v-model="q" :placeholder="meta[kind].placeholder" />
      </div>

      <!-- skill detail: 源文件 + 内容渲染 -->
      <template v-if="detail">
        <header class="det-head">
          <button class="back-btn" title="返回" @click="closeDetail">
            <Icon name="arrow-left-line" :size="16" />
          </button>
          <h1 class="det-title">{{ detail.name }}</h1>
          <span v-if="detail.badge" class="badge">{{ detail.badge }}</span>
          <label class="switch" :title="detail.enabled ? '点击停用' : '点击启用'" @click.stop>
            <input type="checkbox" :checked="detail.enabled" @change="toggle(detail)" />
            <span class="slider"></span>
          </label>
          <span class="flex-sp"></span>
          <button class="ghost-btn" title="刷新" @click="loadDetail">
            <Icon name="refresh-line" :size="15" />
          </button>
        </header>
        <p class="det-desc" :title="detail.sub">{{ detail.sub }}</p>
        <p v-if="detailDir" class="sub mono det-dir">{{ detailDir }}</p>

        <div class="det-body">
          <aside class="files">
            <div class="files-title">源文件</div>
            <div v-if="detailLoading" class="state small">加载中…</div>
            <button
              v-for="f in detailFiles"
              :key="f.file"
              class="file-item"
              :class="{ on: f.file === activeFile }"
              :title="f.file"
              @click="readDetailFile(f.file)"
            >
              <Icon name="file-line" :size="13" />
              <span class="file-name">{{ f.file }}</span>
              <span v-if="f.binary" class="type-badge">二进制</span>
            </button>
          </aside>
          <section class="content">
            <div class="content-head">
              <Icon name="file-line" :size="14" />
              <span>{{ activeFile }}</span>
              <span class="flex-sp"></span>
              <span v-if="fileLoading" class="loading-mark">加载中…</span>
            </div>
            <div class="content-body">
              <div v-if="binaryFile" class="binary-note">
                <Icon name="file-line" :size="16" />
                二进制文件，不展示内容
              </div>
              <MdContent v-else-if="isMarkdown" :source="fileText" />
              <pre v-else class="raw">{{ fileText }}</pre>
            </div>
          </section>
        </div>
      </template>

      <!-- plugin (扩展) detail：源码阅读器 -->
      <template v-else-if="extDetail">
        <header class="det-head">
          <button class="back-btn" title="返回" @click="closeExtDetail">
            <Icon name="arrow-left-line" :size="16" />
          </button>
          <h1 class="det-title">{{ extDetail.name }}</h1>
          <span v-if="extDetail.badge" class="badge">{{ extDetail.badge }}</span>
          <label class="switch" :title="extDetail.enabled ? '点击停用' : '点击启用'" @click.stop>
            <input type="checkbox" :checked="extDetail.enabled" @change="toggle(extDetail)" />
            <span class="slider"></span>
          </label>
          <span class="flex-sp"></span>
          <button class="ghost-btn" title="刷新" @click="loadExtDetail">
            <Icon name="refresh-line" :size="15" />
          </button>
        </header>
        <p class="sub mono det-dir" :title="extDetail.id">{{ extDetail.id }}</p>

        <div class="content ext-content">
          <div class="content-head">
            <Icon name="file-line" :size="14" />
            <span>{{ baseName(extDetail.id) }}</span>
            <span class="flex-sp"></span>
            <span v-if="extLoading" class="loading-mark">加载中…</span>
          </div>
          <div class="content-body">
            <div v-if="extBinary" class="binary-note">
              <Icon name="file-line" :size="16" />
              二进制文件，不展示内容
            </div>
            <MdContent v-else :source="extFenced" />
          </div>
        </div>
      </template>

      <!-- mcp 服务器详情（新建/编辑共用）：通用 / 工具 -->
      <template v-else-if="kind === 'mcp' && mcpDetailName !== null">
        <header class="det-head">
          <button class="back-btn" title="返回" @click="closeMcpDetail">
            <Icon name="arrow-left-line" :size="16" />
          </button>
          <h1 class="det-title">{{ mcpDetailIsNew ? "新建 MCP 服务器" : mcpDetailRow?.name }}</h1>
          <template v-if="!mcpDetailIsNew && mcpDetailRow">
            <span v-if="mcpDetailRow.type" class="type-badge">{{ mcpDetailRow.type }}</span>
            <span v-if="mcpDetailRow.badge" class="badge">{{ mcpDetailRow.badge }}</span>
            <span class="conn-badge" :class="connClass">{{ connText }}</span>
            <template v-if="mcpDetailRow.enabled && mcpProbeState?.status === 'ok' && mcpProbeState?.version">
              <span class="ver-badge">v{{ mcpProbeState?.version }}</span>
            </template>
            <label class="switch" :title="mcpDetailRow.enabled ? '点击停用' : '点击启用'" @click.stop>
              <input type="checkbox" :checked="mcpDetailRow.enabled" @change="toggle(mcpDetailRow)" />
              <span class="slider"></span>
            </label>
          </template>
        </header>
        <p v-if="!mcpDetailIsNew && mcpDetailRow?.entry?.description" class="det-desc">{{ mcpDetailRow.entry.description }}</p>

        <div class="tabs">
          <button :class="{ on: mcpDetailTab === 'general' }" @click="mcpDetailTab = 'general'">通用</button>
          <button
            :class="{ on: mcpDetailTab === 'tools' }"
            :disabled="mcpDetailIsNew"
            :title="mcpDetailIsNew ? '保存后可连接服务器并预览工具' : undefined"
            @click="mcpDetailTab = 'tools'"
          >
            工具<span v-if="mcpToolCount" class="tab-count">{{ mcpToolCount }}</span>
          </button>
        </div>

        <!-- 通用：连接配置 -->
        <div v-if="mcpDetailTab === 'general'" class="mcp-form-wrap">
          <McpServerForm v-model="detailForm" />
          <div v-if="draftProbe" class="probe-row draft">
            <template v-if="draftProbing || draftProbe.status === 'probing'">
              <span class="state small inline">探测中…</span>
            </template>
            <template v-else-if="draftProbe.status === 'ok'">
              <span class="conn-badge on">已连接</span>
              <span class="probe-meta">
                {{ draftProbe.serverName || "服务器" }}<template v-if="draftProbe.version">
                  · v{{ draftProbe.version }}</template
                >
                · {{ draftProbe.tools?.length ?? 0 }} 个工具
              </span>
            </template>
            <template v-else>
              <span class="conn-badge err">{{ draftProbe.needsAuth ? "需要认证" : "连接失败" }}</span>
              <span class="probe-meta err">{{ draftProbe.error }}</span>
            </template>
          </div>
          <div class="form-actions">
            <button class="ghost-text" :disabled="draftProbing" @click="probeDraft">测试连接</button>
            <button class="dark-btn" :disabled="detailSaving" @click="saveDetail">
              {{ detailSaving ? "保存中…" : mcpDetailIsNew ? "添加" : "保存" }}
            </button>
          </div>
        </div>

        <!-- 工具：探测结果 + 每工具自动批准 -->
        <div v-else class="tools-tab">
          <template v-if="mcpDetailIsNew || !mcpDetailRow">
            <p class="hint">先在「通用」中保存配置，保存后会自动连接服务器并预览工具。</p>
          </template>
          <template v-else>
            <div class="probe-row">
              <button class="ghost-btn" :disabled="!!probeBusyName" title="重新探测" @click="probeServer(mcpDetailRow.name)">
                <Icon name="refresh-line" :size="14" />
              </button>
              <template v-if="!mcpProbeState || mcpProbeState.status === 'probing'">
                <span class="state small inline">探测中…</span>
              </template>
              <template v-else-if="mcpProbeState.status === 'ok'">
                <span class="conn-badge on">已连接</span>
                <span class="probe-meta">
                  {{ mcpProbeState.serverName || mcpDetailRow.name }}<template v-if="mcpProbeState.version">
                    · v{{ mcpProbeState.version }}</template
                  >
                  · {{ mcpProbeState.tools?.length ?? 0 }} 个工具
                </span>
              </template>
              <template v-else>
                <span class="conn-badge err">{{ mcpProbeState.needsAuth ? "需要认证" : "连接失败" }}</span>
                <span class="probe-meta err">{{ mcpProbeState.error }}</span>
              </template>
            </div>

            <template v-if="mcpProbeState?.status === 'ok'">
              <div class="search-row tool-search">
                <Icon name="search-line" :size="15" />
                <input v-model="toolFilter" placeholder="过滤工具" />
              </div>
              <div class="tool-head">
                <span>可用工具</span>
                <span>自动批准</span>
              </div>
              <div v-for="t in detailTools" :key="t.name" class="tool-item">
                <div class="tool-row" @click="toggleToolExpand(t.name)">
                  <span class="tool-chev" :class="{ open: expandedTools.has(t.name) }" title="参数">
                    <Icon name="arrow-down-s-line" :size="14" />
                  </span>
                  <div class="tool-info">
                    <b>{{ t.name }}</b>
                    <div v-if="t.description" class="sub-text">{{ t.description }}</div>
                  </div>
                  <label
                    class="switch"
                    :title="isAutoApproved(mcpDetailRow.name, t.name) ? '自动批准已开启' : '自动批准已关闭，调用时将请求确认'"
                    @click.stop
                  >
                    <input
                      type="checkbox"
                      :checked="isAutoApproved(mcpDetailRow.name, t.name)"
                      @change="setAutoApproved(mcpDetailRow.name, t.name, ($event.target as HTMLInputElement).checked)"
                    />
                    <span class="slider"></span>
                  </label>
                </div>
                <div v-if="expandedTools.has(t.name)" class="tool-params">
                  <template v-if="toolParams(t).length">
                    <div class="tp-head">参数（{{ toolParams(t).length }}）</div>
                    <div v-for="p in toolParams(t)" :key="p.name" class="tp-row">
                      <div class="tp-name">
                        <b>{{ p.name }}</b>
                        <i v-if="p.required" title="必填">*</i>
                        <span class="tp-type">{{ p.type }}</span>
                      </div>
                      <div v-if="p.description" class="tp-desc">{{ p.description }}</div>
                      <div v-if="p.enums.length" class="tp-enum">
                        <span class="tp-enum-label">允许的值</span>
                        <code v-for="v in p.enums" :key="v">{{ v }}</code>
                      </div>
                      <div v-if="p.defaultValue !== undefined" class="tp-enum">
                        <span class="tp-enum-label">默认</span>
                        <code>{{ p.defaultValue }}</code>
                      </div>
                    </div>
                  </template>
                  <div v-else class="tp-empty">该工具无需参数</div>
                </div>
              </div>
              <div v-if="!detailTools.length" class="state small">
                {{ toolFilter ? "没有匹配的工具" : "服务器未提供工具" }}
              </div>
              <p class="hint">关闭自动批准后，会话内调用该工具会先弹出确认；改动立即对进行中的会话生效。</p>
            </template>
            <template v-else-if="mcpProbeState?.status === 'error'">
              <p class="hint">
                无法获取工具列表，请检查配置或网络后点击刷新重试。<template v-if="mcpProbeState.needsAuth">
                  若服务器需要 Bearer token，请在「通用」中填写请求头。</template
                >
              </p>
            </template>
          </template>
        </div>
      </template>

      <template v-else>
        <div v-if="loading" class="state">加载中…</div>
        <div v-else class="list">
          <div
            v-for="it in filtered"
            :key="it.id"
            class="row"
            :class="{
              dim: kind === 'mcp' && !it.enabled,
              clickable: kind === 'skills' || kind === 'plugins' || kind === 'mcp',
            }"
            @click="
              kind === 'skills'
                ? openDetail(it)
                : kind === 'plugins'
                  ? openExtDetail(it)
                  : kind === 'mcp'
                    ? openMcpDetail(it)
                    : undefined
            "
          >
            <span class="tile"><Icon :name="meta[kind].icon" :size="18" /></span>
            <div class="info">
              <div class="name-row">
                <span v-if="kind === 'mcp'" class="dot" :class="{ on: it.enabled }"></span>
                <b>{{ it.name }}</b>
                <span v-if="it.badge" class="badge">{{ it.badge }}</span>
                <span v-if="it.type" class="type-badge">{{ it.type }}</span>
                <span v-if="kind === 'mcp' && probeCache[it.name]?.status === 'ok' && probeCache[it.name]?.version" class="ver-badge">
                  {{ probeCache[it.name]?.version }}
                </span>
              </div>
              <div class="sub-text" :title="it.sub">{{ it.sub }}</div>
            </div>
            <label v-if="it.togglable" class="switch" :title="it.enabled ? '点击停用' : '点击启用'" @click.stop>
              <input type="checkbox" :checked="it.enabled" @change="toggle(it)" />
              <span class="slider"></span>
            </label>
            <template v-if="kind === 'mcp'">
              <button class="row-btn" title="编辑" @click.stop="openMcpDetail(it)">
                <Icon name="edit-2-line" :size="14" />
              </button>
              <button class="row-btn danger" title="删除" @click.stop="removeServer(it)">
                <Icon name="delete-bin-line" :size="14" />
              </button>
            </template>
          </div>
          <div v-if="!filtered.length" class="state">
            {{ q ? "没有匹配项" : meta[kind].empty }}
          </div>
        </div>
      </template>

      <div v-if="notice" class="notice">{{ notice }}</div>
    </div>

    <!-- install dialog (skills / plugins) -->
    <div v-if="installOpen" class="dialog-mask" @click.self="installOpen = false">
      <div class="dialog">
        <header class="d-head">
          <h2>安装{{ kind === "skills" ? "技能" : "插件" }}</h2>
          <button class="d-close" title="关闭" @click="installOpen = false">
            <Icon name="close-line" :size="15" />
          </button>
        </header>
        <p class="d-sub">
          {{
            kind === "skills"
              ? "支持 zip / .tgz 压缩包（须含 SKILL.md），安装到 ~/.pi/agent/skills。"
              : "支持单个 .ts / .js 源文件，安装到 ~/.pi/agent/extensions。"
          }}
          技能与插件会被加载执行，请只安装可信来源。
        </p>

        <div class="seg">
          <button :class="{ on: installMode === 'url' }" @click="installMode = 'url'">从网址下载</button>
          <button :class="{ on: installMode === 'local' }" @click="installMode = 'local'">从本地导入</button>
        </div>

        <template v-if="installMode === 'url'">
          <div class="field">
            <label>下载网址 <i>*</i></label>
            <input
              v-model="installUrl"
              :placeholder="kind === 'skills' ? 'https://example.com/my-skill.zip' : 'https://example.com/my-plugin.ts'"
              @keydown.enter="submitInstall"
            />
          </div>
          <div class="field">
            <label>{{ kind === "skills" ? "安装名称（可选，默认取包名）" : "保存文件名（可选，默认取网址末段）" }}</label>
            <input v-model="installName" :placeholder="kind === 'skills' ? 'my-skill' : 'my-plugin.ts'" @keydown.enter="submitInstall" />
          </div>
        </template>
        <template v-else>
          <div class="field">
            <label>本地文件 <i>*</i></label>
            <div class="pick-row">
              <input :value="installSrcPath" class="pick-display" placeholder="点击右侧按钮选择" disabled />
              <button class="ghost-btn" @click="pickInstallFile">选择文件</button>
            </div>
          </div>
          <div v-if="kind === 'skills'" class="field">
            <label>安装名称（可选，默认取包名）</label>
            <input v-model="installName" placeholder="my-skill" @keydown.enter="submitInstall" />
          </div>
        </template>

        <footer class="d-foot">
          <button class="cancel" @click="installOpen = false">取消</button>
          <button class="dark-btn" :disabled="installBusy" @click="submitInstall">
            {{ installBusy ? "安装中…" : "安装" }}
          </button>
        </footer>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tools-page {
  flex: 1;
  overflow-y: auto;
  background: var(--pd-bg);
  min-height: 0;
}
.wrap {
  padding: 30px 36px 48px;
}
.head {
  display: flex;
  align-items: flex-start;
  gap: 16px;
}
.title-block { flex: 1; min-width: 0; }
h1 {
  margin: 0;
  font-size: calc(21px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
  display: flex;
  align-items: center;
  gap: 10px;
}
.count {
  font-size: calc(12px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text-3);
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  padding: 1px 9px;
}
.sub {
  margin: 6px 0 0;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.5;
}
.sub.mono { font-family: var(--pd-mono); font-size: calc(11.5px * var(--pd-font-scale)); color: var(--pd-text-4); }
.head-actions { display: flex; align-items: center; gap: 8px; flex: none; }
.ghost-btn {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text-3);
  cursor: pointer;
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
}
.dark-btn:hover { background: var(--pd-accent); color: var(--pd-on-accent); }

.tabs {
  display: flex;
  gap: 18px;
  margin-top: 18px;
  border-bottom: 1px solid var(--pd-border-soft);
}
.tabs button {
  background: none;
  border: none;
  color: var(--pd-text-3);
  font-size: calc(13.5px * var(--pd-font-scale));
  padding: 8px 2px;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
}
.tabs button:hover { color: var(--pd-text-2); }
.tabs button.on {
  color: var(--pd-text);
  border-bottom-color: var(--pd-accent);
}
.tabs button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.search-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 16px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  padding: 8px 12px;
  color: var(--pd-text-4);
}
.search-row:focus-within { border-color: #4a4a4a; }
.search-row input {
  flex: 1;
  background: none;
  border: none;
  outline: none;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
}
.search-row input::placeholder { color: var(--pd-text-4); }

.list { margin-top: 16px; }
.row {
  display: flex;
  align-items: center;
  gap: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 14px 16px;
  margin-bottom: 10px;
  transition: border-color 0.15s;
}
.row:hover { border-color: var(--pd-border); }
.row.dim .name-row b { color: var(--pd-text-3); }
.row.dim .sub-text { color: var(--pd-text-4); }
.row.dim .tile { opacity: 0.45; }
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: none;
  background: var(--pd-text-4);
}
.dot.on { background: var(--pd-green); }
.tile {
  width: 38px;
  height: 38px;
  flex: none;
  border-radius: 10px;
  display: grid;
  place-items: center;
  background: var(--pd-accent-soft);
  color: var(--pd-accent);
}
.info { flex: 1; min-width: 0; }
.name-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.name-row b {
  color: var(--pd-text);
  font-size: calc(14px * var(--pd-font-scale));
  font-weight: 600;
}
.badge {
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
}
.type-badge {
  font-size: calc(10px * var(--pd-font-scale));
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
  font-family: var(--pd-mono);
}
.sub-text {
  margin-top: 4px;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.55;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-all;
}
.row-btn {
  width: 28px;
  height: 28px;
  flex: none;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-4);
  cursor: pointer;
}
.row-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.row-btn.danger:hover { background: var(--pd-red-soft); color: var(--pd-red-text); }

.switch {
  position: relative;
  flex: none;
  width: 42px;
  height: 23px;
  cursor: pointer;
}
.switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}
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

.state {
  color: var(--pd-text-4);
  font-size: calc(13px * var(--pd-font-scale));
  padding: 28px 4px;
  line-height: 1.7;
}
.state.small { padding: 8px 4px; }

.flex-sp { flex: 1; }
.row.clickable { cursor: pointer; }

/* ---- skill detail ---- */
.det-head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.back-btn {
  width: 30px;
  height: 30px;
  flex: none;
  display: grid;
  place-items: center;
  background: none;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.back-btn:hover { color: var(--pd-text); background: var(--pd-bg-hover); }
.det-title {
  margin: 0;
  font-size: calc(20px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
}
.det-desc {
  margin: 12px 0 0;
  font-size: calc(13px * var(--pd-font-scale));
  color: var(--pd-text-2);
  line-height: 1.6;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.det-dir { margin-top: 6px; }
.ext-content { margin-top: 20px; }
:deep(.ext-content pre) {
  background: var(--pd-code-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 10px;
  padding: 14px 16px;
  overflow-x: auto;
}
.det-body {
  display: flex;
  gap: 16px;
  margin-top: 20px;
  align-items: flex-start;
}
.files {
  width: 220px;
  flex: none;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 8px;
}
.files-title {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  padding: 4px 8px 8px;
}
.file-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  border-radius: 8px;
  padding: 7px 9px;
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
  font-family: var(--pd-mono);
}
.file-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.file-item.on { background: var(--pd-bg-active); color: var(--pd-text); }
.file-item svg { color: var(--pd-accent); flex: none; }
.file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.content {
  flex: 1;
  min-width: 0;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  overflow: hidden;
}
.content-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--pd-border-soft);
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  font-family: var(--pd-mono);
}
.content-head svg { color: var(--pd-text-3); }
.loading-mark { font-size: calc(11.5px * var(--pd-font-scale)); color: var(--pd-text-4); font-family: inherit; }
.content-body { padding: 18px 22px; overflow-x: auto; }
.binary-note {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--pd-text-4);
  font-size: calc(12.5px * var(--pd-font-scale));
  padding: 20px 0;
}
.file-item .type-badge { flex: none; margin-left: auto; font-size: calc(9.5px * var(--pd-font-scale)); }
.raw {
  margin: 0;
  font-family: var(--pd-mono);
  font-size: calc(12.5px * var(--pd-font-scale));
  line-height: 1.7;
  color: var(--pd-text-2);
  white-space: pre-wrap;
  word-break: break-word;
}

.notice {
  margin-top: 14px;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 8px;
  padding: 8px 12px;
}

/* ---- dialog ---- */
/* 安装来源分段选择器 */
.seg {
  display: inline-flex;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 2px;
  gap: 2px;
  margin-top: 14px;
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
.pick-row { display: flex; gap: 8px; }
.pick-row .pick-display {
  flex: 1;
  min-width: 0;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale));
  font-family: var(--pd-mono);
  padding: 9px 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* head-actions 的 ghost-btn 是 30×30 图标钮；此处需要常规文字按钮 */
.pick-row .ghost-btn {
  width: auto;
  height: auto;
  white-space: nowrap;
  padding: 8px 14px;
}
.dialog-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: grid;
  place-items: center;
  z-index: 120;
}
.dialog {
  width: min(500px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow-y: auto;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  box-shadow: var(--pd-shadow);
  padding: 20px 22px 18px;
}
.d-head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.d-head h2 {
  margin: 0;
  font-size: calc(17px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
  flex: 1;
}
.d-close {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.d-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.d-sub {
  margin: 6px 0 0;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.55;
}
.field { margin-top: 16px; }
.field label {
  display: block;
  font-size: calc(13px * var(--pd-font-scale));
  color: var(--pd-text-2);
  margin-bottom: 7px;
}
.field label i {
  color: var(--pd-red);
  font-style: normal;
}
.field input,
.field select,
.field textarea {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  font-family: inherit;
  padding: 9px 12px;
}
.field textarea,
.field select {
  resize: vertical;
  font-family: inherit;
  line-height: 1.6;
}
.field select option { background: var(--pd-bg-raised); }
.field input:focus,
.field select:focus,
.field textarea:focus {
  outline: none;
  border-color: var(--pd-accent);
}
.field input::placeholder,
.field textarea::placeholder { color: var(--pd-text-4); }
.field textarea { font-family: var(--pd-mono); font-size: calc(12.5px * var(--pd-font-scale)); }

.advanced { margin-top: 16px; border-top: 1px solid var(--pd-border-soft); padding-top: 12px; }
.adv-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
  padding: 2px 0;
}
.adv-chev { display: grid; place-items: center; color: var(--pd-text-4); transition: transform 0.12s; }
.adv-chev.open { transform: rotate(180deg); }
.advanced .field { margin-top: 12px; }

.d-foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 22px;
}
.cancel {
  background: none;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 9px;
  padding: 7px 16px;
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.cancel:hover { background: var(--pd-bg-hover); color: var(--pd-text); }

/* ---- mcp 详情页 / 探测 / 工具列表 ---- */
.ver-badge {
  font-family: var(--pd-mono);
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
}
.conn-badge {
  font-size: calc(11px * var(--pd-font-scale));
  border-radius: 999px;
  padding: 2px 10px;
  flex: none;
  background: var(--pd-bg-hover);
  color: var(--pd-text-3);
  white-space: nowrap;
}
.conn-badge.on { background: var(--pd-green-soft); color: var(--pd-green-text); }
.conn-badge.err { background: var(--pd-red-soft); color: var(--pd-red-text); }
.conn-badge.warn { background: var(--pd-accent-soft); color: var(--pd-accent-text); }
.mcp-form-wrap { margin-top: 4px; max-width: 660px; }
.form-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
.probe-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 16px;
  flex-wrap: wrap;
}
.probe-row.draft { margin-top: 4px; }
.probe-meta {
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  word-break: break-all;
  min-width: 0;
}
.probe-meta.err { color: var(--pd-red-text); }
.state.small.inline { padding: 0; }
.tab-count {
  margin-left: 6px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
}
.tool-search { margin-top: 16px; }
.tool-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 16px;
  padding: 0 4px 8px;
  border-bottom: 1px solid var(--pd-border-soft);
  color: var(--pd-text-3);
  font-size: calc(12px * var(--pd-font-scale));
}
.tool-item {
  border-bottom: 1px solid var(--pd-border-soft);
}
.tool-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 4px;
  cursor: pointer;
}
.tool-chev {
  width: 22px;
  height: 22px;
  flex: none;
  display: grid;
  place-items: center;
  color: var(--pd-text-4);
  transform: rotate(-90deg);
  transition: transform 0.12s, color 0.12s;
}
.tool-chev.open {
  transform: rotate(0);
  color: var(--pd-text-2);
}
.tool-params {
  padding: 0 4px 12px 32px;
}
.tp-head {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  margin-bottom: 6px;
}
.tp-row {
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  padding: 8px 11px;
  margin-bottom: 6px;
}
.tp-row:last-child { margin-bottom: 0; }
.tp-name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.tp-name b {
  font-family: var(--pd-mono);
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
  word-break: break-all;
}
.tp-name i {
  color: var(--pd-red);
  font-style: normal;
  font-weight: 700;
}
.tp-type {
  font-family: var(--pd-mono);
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1px 7px;
  flex: none;
}
.tp-desc {
  margin-top: 4px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
  line-height: 1.55;
  word-break: break-word;
}
.tp-enum {
  margin-top: 5px;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
}
.tp-enum-label {
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
}
.tp-enum code {
  font-family: var(--pd-mono);
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-2);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1px 7px;
  word-break: break-all;
}
.tp-empty {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
}
.tool-info { flex: 1; min-width: 0; }
.tool-info b {
  color: var(--pd-text);
  font-size: calc(13.5px * var(--pd-font-scale));
  font-weight: 600;
  font-family: var(--pd-mono);
  word-break: break-all;
}
.tool-info .sub-text { margin-top: 3px; -webkit-line-clamp: 2; }
.ghost-text {
  background: none;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 9px;
  padding: 7px 16px;
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.ghost-text:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.ghost-text:disabled { opacity: 0.5; cursor: default; }
.hint {
  margin: 12px 0 0;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  line-height: 1.6;
}

/* ---- 手机端（≤480px），置于文件末尾以覆盖上方规则：头部换行、弹窗贴边 ---- */
@media (max-width: 480px) {
  .wrap { padding: 16px 14px 40px; }
  .head { flex-wrap: wrap; row-gap: 10px; }
  .title-block { flex: 1 1 100%; }
  .head-actions { margin-left: auto; }
  .dialog { width: calc(100vw - 16px); max-height: calc(100dvh - 24px); padding: 16px 14px 14px; }
}
</style>
