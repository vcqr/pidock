<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { FILE_PICKER, type DataBus } from "../databus.js";
import Icon from "./Icon.vue";
import MdContent from "./MdContent.vue";

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
const showForm = ref(false);
const formMode = ref<"add" | "edit">("add");
const formKey = ref(""); // original server name when editing
const fName = ref("");
const fType = ref<"stdio" | "http" | "streamable-http">("stdio");
const fCommand = ref("");
const fArgs = ref("");
const fUrl = ref("");
const fEnv = ref("");
const advancedOpen = ref(false);

// pi 官方约定（pi.dev/docs/mcp）：type 取 stdio | http | streamable-http，
// SSE 不受支持；enabled: false 表示保留条目但不连接
const typeOptions = [
  { value: "stdio", label: "标准输入 / 输出 (stdio)" },
  { value: "http", label: "HTTP (http)" },
  { value: "streamable-http", label: "可流式传输的 HTTP (streamable-http)" },
];

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

function openAdd(): void {
  formMode.value = "add";
  formKey.value = "";
  fName.value = "";
  fType.value = "stdio";
  fCommand.value = "";
  fArgs.value = "";
  fUrl.value = "";
  fEnv.value = "";
  advancedOpen.value = false;
  showForm.value = true;
}

function openEdit(it: Row): void {
  const v = it.entry ?? {};
  formMode.value = "edit";
  formKey.value = it.name;
  fName.value = it.name;
  fType.value = v.command
    ? "stdio"
    : v.url
      ? v.type === "streamable-http"
        ? "streamable-http"
        : "http"
      : "stdio";
  fCommand.value = v.command ?? "";
  fArgs.value = Array.isArray(v.args) ? v.args.join("\n") : "";
  fUrl.value = v.url ?? "";
  fEnv.value = envToLines(v.env);
  advancedOpen.value = !!fEnv.value;
  showForm.value = true;
}

async function saveForm(): Promise<void> {
  const name = fName.value.trim();
  if (!name) return flash("名称不能为空");
  const container = serversContainer(rawConfig.value ?? {}, true)!;
  if (formMode.value === "add" && container[name]) return flash(`服务器「${name}」已存在`);
  if (formMode.value === "edit" && formKey.value !== name && container[name]) {
    return flash(`服务器「${name}」已存在`);
  }
  if (fType.value === "stdio" && !fCommand.value.trim()) return flash("命令不能为空");
  if (fType.value !== "stdio" && !fUrl.value.trim()) return flash("URL 不能为空");

  // keep unknown extra keys of the original entry, replace the managed ones
  const old = formMode.value === "edit" ? { ...(container[formKey.value] ?? {}) } : {};
  for (const k of ["command", "args", "env", "url", "type"]) delete old[k];
  const entry: any = { ...old };
  if (fType.value === "stdio") {
    entry.command = fCommand.value.trim();
    const args = parseLines(fArgs.value);
    if (args.length) entry.args = args;
    const env = parseEnv(fEnv.value);
    if (env) entry.env = env;
  } else {
    entry.url = fUrl.value.trim();
    entry.type = fType.value;
  }

  if (formMode.value === "edit" && formKey.value !== name) delete container[formKey.value];
  container[name] = entry;
  try {
    await props.bus.request("config.mcp.set", { config: rawConfig.value });
    flash(formMode.value === "add" ? `已添加「${name}」` : `已保存「${name}」`);
    showForm.value = false;
    await load();
  } catch (err) {
    flash(String(err));
  }
}

async function removeServer(it: Row): Promise<void> {
  if (!window.confirm(`删除 MCP 服务器「${it.name}」？`)) return;
  try {
    const container = serversContainer(rawConfig.value, false);
    if (container && it.name in container) {
      delete container[it.name];
      await props.bus.request("config.mcp.set", { config: rawConfig.value });
      flash(`已删除「${it.name}」`);
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
      <header class="head">
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
          <button v-if="kind === 'mcp'" class="dark-btn" @click="openAdd">
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

      <div class="search-row">
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

      <template v-else>
        <div v-if="loading" class="state">加载中…</div>
        <div v-else class="list">
          <div
            v-for="it in filtered"
            :key="it.id"
            class="row"
            :class="{
              dim: kind === 'mcp' && !it.enabled,
              clickable: kind === 'skills' || kind === 'plugins',
            }"
            @click="kind === 'skills' ? openDetail(it) : kind === 'plugins' ? openExtDetail(it) : undefined"
          >
            <span class="tile"><Icon :name="meta[kind].icon" :size="18" /></span>
            <div class="info">
              <div class="name-row">
                <span v-if="kind === 'mcp'" class="dot" :class="{ on: it.enabled }"></span>
                <b>{{ it.name }}</b>
                <span v-if="it.badge" class="badge">{{ it.badge }}</span>
                <span v-if="it.type" class="type-badge">{{ it.type }}</span>
              </div>
              <div class="sub-text" :title="it.sub">{{ it.sub }}</div>
            </div>
            <label v-if="it.togglable" class="switch" :title="it.enabled ? '点击停用' : '点击启用'" @click.stop>
              <input type="checkbox" :checked="it.enabled" @change="toggle(it)" />
              <span class="slider"></span>
            </label>
            <template v-if="kind === 'mcp'">
              <button class="row-btn" title="编辑" @click.stop="openEdit(it)">
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

    <!-- mcp add/edit dialog -->
    <div v-if="showForm" class="dialog-mask" @click.self="showForm = false">
      <div class="dialog">
        <header class="d-head">
          <h2>{{ formMode === "add" ? "添加 MCP 服务器" : "编辑 MCP 服务器" }}</h2>
          <button class="d-close" title="关闭" @click="showForm = false">
            <Icon name="close-line" :size="15" />
          </button>
        </header>
        <p class="d-sub">填写连接信息后保存，内容写入 mcp.json；其余配置可稍后直接编辑该文件调整。</p>

        <div class="field">
          <label>名称 <i>*</i></label>
          <input v-model="fName" placeholder="名称" @keydown.enter="saveForm" />
        </div>

        <div class="field">
          <label>类型 <i>*</i></label>
          <select v-model="fType">
            <option v-for="o in typeOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
          </select>
        </div>

        <template v-if="fType === 'stdio'">
          <div class="field">
            <label>命令 <i>*</i></label>
            <input v-model="fCommand" placeholder="uvx or npx" @keydown.enter="saveForm" />
          </div>
          <div class="field">
            <label>参数</label>
            <textarea v-model="fArgs" rows="3" placeholder="arg1&#10;arg2" spellcheck="false"></textarea>
          </div>
          <div class="advanced">
            <button class="adv-head" @click="advancedOpen = !advancedOpen">
              高级设置
              <span class="adv-chev" :class="{ open: advancedOpen }"><Icon name="arrow-down-s-line" :size="13" /></span>
            </button>
            <div v-if="advancedOpen" class="field">
              <label>环境变量</label>
              <textarea v-model="fEnv" rows="3" placeholder="KEY=value&#10;ANOTHER_KEY=value" spellcheck="false"></textarea>
            </div>
          </div>
        </template>

        <div v-else class="field">
          <label>URL <i>*</i></label>
          <input v-model="fUrl" placeholder="http://host:port/mcp" @keydown.enter="saveForm" />
        </div>

        <footer class="d-foot">
          <button class="cancel" @click="showForm = false">取消</button>
          <button class="dark-btn" @click="saveForm">{{ formMode === "add" ? "添加" : "保存" }}</button>
        </footer>
      </div>
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
  font-size: 21px;
  font-weight: 700;
  color: var(--pd-text);
  display: flex;
  align-items: center;
  gap: 10px;
}
.count {
  font-size: 12px;
  font-weight: 600;
  color: var(--pd-text-3);
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  padding: 1px 9px;
}
.sub {
  margin: 6px 0 0;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.5;
}
.sub.mono { font-family: Consolas, monospace; font-size: 11.5px; color: var(--pd-text-4); }
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
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.dark-btn:hover { background: var(--pd-accent); color: #1a1a1a; }

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
  font-size: 13.5px;
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
  font-size: 13px;
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
  font-size: 14px;
  font-weight: 600;
}
.badge {
  font-size: 10.5px;
  color: var(--pd-accent-text);
  background: var(--pd-accent-soft);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
}
.type-badge {
  font-size: 10px;
  color: var(--pd-text-3);
  background: var(--pd-bg-hover);
  border-radius: 5px;
  padding: 1.5px 7px;
  flex: none;
  font-family: Consolas, monospace;
}
.sub-text {
  margin-top: 4px;
  font-size: 12.5px;
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
  font-size: 13px;
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
  font-size: 20px;
  font-weight: 700;
  color: var(--pd-text);
}
.det-desc {
  margin: 12px 0 0;
  font-size: 13px;
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
  font-size: 12px;
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
  font-size: 12.5px;
  cursor: pointer;
  font-family: Consolas, monospace;
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
  font-size: 13px;
  font-family: Consolas, monospace;
}
.content-head svg { color: var(--pd-text-3); }
.loading-mark { font-size: 11.5px; color: var(--pd-text-4); font-family: inherit; }
.content-body { padding: 18px 22px; overflow-x: auto; }
.binary-note {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--pd-text-4);
  font-size: 12.5px;
  padding: 20px 0;
}
.file-item .type-badge { flex: none; margin-left: auto; font-size: 9.5px; }
.raw {
  margin: 0;
  font-family: Consolas, monospace;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--pd-text-2);
  white-space: pre-wrap;
  word-break: break-word;
}

.notice {
  margin-top: 14px;
  font-size: 12.5px;
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
  font-size: 12.5px;
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
  font-size: 12.5px;
  font-family: Consolas, monospace;
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
  font-size: 17px;
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
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.55;
}
.field { margin-top: 16px; }
.field label {
  display: block;
  font-size: 13px;
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
  font-size: 13px;
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
.field textarea { font-family: Consolas, monospace; font-size: 12.5px; }

.advanced { margin-top: 16px; border-top: 1px solid var(--pd-border-soft); padding-top: 12px; }
.adv-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: 13px;
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
  font-size: 13px;
  cursor: pointer;
}
.cancel:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
</style>
