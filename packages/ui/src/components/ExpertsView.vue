<script setup lang="ts">
import { inject, onMounted, ref } from "vue";
import type { DataBus } from "../databus.js";
import { FOLDER_PICKER, FILE_PICKER } from "../databus.js";
import Icon from "./Icon.vue";

/**
 * 专家（智能体编排）：主区右栏页面（侧栏保留，同自动化页模式）。
 * 专家 = 预编排的智能体档案：角色提示词（append 到 pi 默认系统提示词）、
 * 全局技能/插件白名单、内置工具增减、知识库目录、私有技能/插件。
 * 模型不归属专家（跟随全局/会话选择），编辑器只保留思考等级与权限模式。
 * 雇佣 = 新建任务时绑定（host 在会话创建时应用全部配置）。
 */

const props = defineProps<{ bus: DataBus }>();
const emit = defineEmits<{ close: []; "open-providers": []; hire: [expert: { id: string; name: string }] }>();

interface Expert {
  id: string;
  name: string;
  description?: string;
  icon?: string;
  avatar?: string;
  avatar_color?: string;
  prompt: string;
  skills: string[];
  extensions: string[];
  tools?: string[];
  exclude_tools?: string[];
  knowledge_dirs: string[];
  model?: string;
  thinking_level?: string;
  permission_mode?: string;
  created_at: string;
  updated_at: string;
}
interface PrivateResource {
  skills: Array<{ name: string; description: string; path: string }>;
  extensions: Array<{ name: string; path: string }>;
}

const experts = ref<Expert[]>([]);
const loading = ref(true);
const notice = ref("");
const noticeErr = ref(false);
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
function flash(msg: string, isErr = false): void {
  notice.value = msg;
  noticeErr.value = isErr;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = ""), 3200);
}

const ICON_CHOICES = [
  "user-star-line",
  "magic-line",
  "brain-line",
  "bug-line",
  "code-line",
  "quill-pen-line",
  "rocket-line",
  "lightbulb-line",
];
/** 字形头像底色盘（缺省 = 主题灰） */
const AVATAR_COLORS = ["", "#f59e0b", "#10b981", "#3b82f6", "#8b5cf6", "#ec4899", "#14b8a6", "#e04444"];
const EXCLUDE_TOOLS = [
  { name: "bash", label: "执行命令" },
  { name: "edit", label: "编辑文件" },
  { name: "write", label: "写入文件" },
];

async function load(): Promise<void> {
  loading.value = true;
  try {
    const r = await props.bus.request("experts.list");
    experts.value = r?.experts ?? [];
  } catch (e) {
    flash(`专家列表加载失败：${errText(e)}`, true);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

// ---------------------------------------------------------------- 编辑器

type EditorTab = "basic" | "skills" | "mcp" | "kb";
const TABS: Array<{ id: EditorTab; label: string }> = [
  { id: "basic", label: "基础信息" },
  { id: "skills", label: "技能" },
  { id: "mcp", label: "MCP 工具" },
  { id: "kb", label: "知识库" },
];
const activeTab = ref<EditorTab>("basic");
const showEditor = ref(false);
const editingId = ref<string | null>(null);
const saving = ref(false);
const form = ref({
  name: "",
  description: "",
  icon: "user-star-line",
  avatar: "",
  avatar_color: "",
  prompt: "",
  skills: [] as string[],
  extensions: [] as string[],
  exclude_tools: [] as string[],
  knowledge_dirs: [] as string[],
});
const newKbDir = ref("");
const folderPicker = inject(FOLDER_PICKER, null);

function openCreate(): void {
  editingId.value = null;
  form.value = {
    name: "",
    description: "",
    icon: "user-star-line",
    avatar: "",
    avatar_color: "",
    prompt: "",
    skills: [],
    extensions: [],
    exclude_tools: [],
    knowledge_dirs: [],
  };
  privates.value = { skills: [], extensions: [] };
  activeTab.value = "basic";
  showEditor.value = true;
  void ensurePickers();
}
function openEdit(e: Expert): void {
  editingId.value = e.id;
  form.value = {
    name: e.name,
    description: e.description ?? "",
    icon: e.icon ?? "user-star-line",
    avatar: e.avatar ?? "",
    avatar_color: e.avatar_color ?? "",
    prompt: e.prompt,
    skills: [...(e.skills ?? [])],
    extensions: [...(e.extensions ?? [])],
    exclude_tools: [...(e.exclude_tools ?? [])],
    knowledge_dirs: [...(e.knowledge_dirs ?? [])],
  };
  activeTab.value = "basic";
  showEditor.value = true;
  void ensurePickers();
  void loadPrivates(e.id);
}

async function save(): Promise<void> {
  if (saving.value) return;
  if (!form.value.name.trim()) {
    activeTab.value = "basic";
    flash("请填写专家名称", true);
    return;
  }
  saving.value = true;
  try {
    await props.bus.request("experts.save", {
      ...(editingId.value ? { id: editingId.value } : {}),
      name: form.value.name.trim(),
      description: form.value.description.trim(),
      icon: form.value.icon,
      avatar: form.value.avatar,
      avatar_color: form.value.avatar_color,
      prompt: form.value.prompt,
      skills: form.value.skills,
      extensions: form.value.extensions,
      exclude_tools: form.value.exclude_tools,
      knowledge_dirs: form.value.knowledge_dirs,
    });
    flash(editingId.value ? "专家已保存" : "专家已创建");
    showEditor.value = false;
    await load();
  } catch (e) {
    flash(errText(e), true);
  } finally {
    saving.value = false;
  }
}

async function remove(e: Expert): Promise<void> {
  if (!window.confirm(`删除专家「${e.name}」？其私有资源一并删除。`)) return;
  try {
    await props.bus.request("experts.delete", { id: e.id });
    flash(`已删除「${e.name}」`);
    await load();
  } catch (err) {
    flash(errText(err), true);
  }
}

function hire(e: Expert): void {
  emit("hire", { id: e.id, name: e.name });
}

// ---------------------------------------------------------------- 选择数据

const skillOptions = ref<Array<{ name: string; description: string }>>([]);
const extOptions = ref<Array<{ name: string; file: string }>>([]);
const pickersLoaded = ref(false);

async function ensurePickers(): Promise<void> {
  if (pickersLoaded.value) return;
  pickersLoaded.value = true;
  try {
    const [skills, exts] = await Promise.all([
      props.bus.request("config.skills.list", {}).catch(() => ({ skills: [] })),
      props.bus.request("config.extensions.list", {}).catch(() => ({ extensions: [] })),
    ]);
    skillOptions.value = (skills?.skills ?? [])
      .filter((s: any) => s.enabled !== false)
      .map((s: any) => ({ name: String(s.name), description: String(s.description ?? "") }));
    extOptions.value = (exts?.extensions ?? []).map((x: any) => ({ name: String(x.name), file: String(x.file ?? "") }));
  } catch {
    // 选择项加载失败不阻塞编辑器，仅列表为空
  }
}

function toggle(list: string[], v: string): void {
  const i = list.indexOf(v);
  if (i >= 0) list.splice(i, 1);
  else list.push(v);
}

function toggleExclude(name: string): void {
  toggle(form.value.exclude_tools, name);
}

// ---------------------------------------------------------------- 头像

const avatarUploading = ref(false);

/**
 * 头像上传：host 读本地图片 → data URL → 前端 canvas 缩到 128×128（cover 裁剪）
 * → 存 data URL（几十 KB，随 experts.json 落盘，免文件管理）。
 */
async function pickAvatar(): Promise<void> {
  if (avatarUploading.value || !pickFile) return;
  let path: string | null = null;
  try {
    path = await pickFile();
  } catch {
    return;
  }
  if (!path) return;
  avatarUploading.value = true;
  try {
    const r = await props.bus.request("experts.read_avatar_file", { srcPath: path });
    const src: string = r?.data_url;
    if (!src) throw new Error("读取结果为空");
    form.value.avatar = await resizeToAvatar(src);
    flash("头像已就绪，保存后生效");
  } catch (e) {
    flash(errText(e), true);
  } finally {
    avatarUploading.value = false;
  }
}

function resizeToAvatar(dataUrl: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const S = 128;
      const canvas = document.createElement("canvas");
      canvas.width = S;
      canvas.height = S;
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        reject(new Error("canvas 不可用"));
        return;
      }
      // cover 裁剪：短边贴满、居中裁掉长边
      const scale = Math.max(S / img.width, S / img.height);
      const w = img.width * scale;
      const h = img.height * scale;
      ctx.drawImage(img, (S - w) / 2, (S - h) / 2, w, h);
      resolve(canvas.toDataURL("image/webp", 0.9));
    };
    img.onerror = () => reject(new Error("图片解析失败"));
    img.src = dataUrl;
  });
}

async function addKbDir(): Promise<void> {
  if (folderPicker) {
    try {
      const p = await folderPicker();
      if (p && !form.value.knowledge_dirs.includes(p)) form.value.knowledge_dirs.push(p);
      return;
    } catch {
      // 选择器失败回退到行内输入
    }
  }
  const p = newKbDir.value.trim();
  if (p && !form.value.knowledge_dirs.includes(p)) form.value.knowledge_dirs.push(p);
  newKbDir.value = "";
}

// ---------------------------------------------------------------- 私有资源

const privates = ref<PrivateResource>({ skills: [], extensions: [] });
const pickFile = inject(FILE_PICKER, null);
const showInstall = ref(false);
const installKind = ref<"skill" | "extension">("skill");
const installMode = ref<"url" | "local">("url");
const installUrl = ref("");
const installSrcPath = ref("");
const installName = ref("");
const installing = ref(false);

async function loadPrivates(id: string): Promise<void> {
  try {
    const r = await props.bus.request("experts.private_list", { expert_id: id });
    privates.value = { skills: r?.skills ?? [], extensions: r?.extensions ?? [] };
  } catch {
    privates.value = { skills: [], extensions: [] };
  }
}

function openInstall(kind: "skill" | "extension"): void {
  installKind.value = kind;
  installMode.value = "url";
  installUrl.value = "";
  installSrcPath.value = "";
  installName.value = "";
  showInstall.value = true;
}

async function pickInstallFile(): Promise<void> {
  if (!pickFile) return;
  try {
    const p = await pickFile();
    if (p) installSrcPath.value = p;
  } catch {
    // 忽略选择器失败
  }
}

async function doInstall(): Promise<void> {
  if (installing.value || !editingId.value) return;
  const url = installUrl.value.trim();
  const srcPath = installSrcPath.value.trim();
  if (!url && !srcPath) {
    flash("请填写网址或选择本地文件", true);
    return;
  }
  installing.value = true;
  try {
    await props.bus.request("experts.install_resource", {
      expert_id: editingId.value,
      kind: installKind.value,
      ...(srcPath ? { srcPath } : { url }),
      ...(installName.value.trim() ? { name: installName.value.trim() } : {}),
    });
    flash("私有资源已安装");
    showInstall.value = false;
    await loadPrivates(editingId.value);
  } catch (e) {
    flash(errText(e), true);
  } finally {
    installing.value = false;
  }
}

async function removePrivate(kind: "skill" | "extension", name: string): Promise<void> {
  if (!editingId.value) return;
  try {
    await props.bus.request("experts.remove_resource", { expert_id: editingId.value, kind, name });
    await loadPrivates(editingId.value);
  } catch (e) {
    flash(errText(e), true);
  }
}
</script>

<template>
  <section class="experts-page">
    <header class="page-head">
      <div class="head-text">
        <h1><Icon name="user-star-line" :size="20" />专家</h1>
        <p>编排可复用的专家智能体：角色提示词、技能与 MCP、知识库。新建任务时雇佣，或用 /expert:名称 以专家身份发言。</p>
      </div>
      <div class="head-actions">
        <button class="btn primary" @click="openCreate"><Icon name="add-line" :size="15" />新建专家</button>
        <button class="icon-btn" title="关闭" @click="emit('close')">
          <Icon name="close-line" :size="17" />
        </button>
      </div>
    </header>

    <div v-if="notice" class="notice" :class="{ err: noticeErr }">{{ notice }}</div>

    <div v-if="loading" class="empty"><Icon name="loader-2-line" :size="18" />加载中…</div>
    <div v-else-if="experts.length === 0" class="empty">
      <Icon name="user-star-line" :size="34" />
      <p>还没有专家。创建第一个专家：给它角色提示词、勾选技能与 MCP，再挂上知识库目录。</p>
      <button class="btn primary" @click="openCreate"><Icon name="add-line" :size="15" />新建专家</button>
    </div>

    <div v-else class="cards">
      <div v-for="e in experts" :key="e.id" class="card">
        <div class="card-head">
          <span
            class="avatar"
            :style="!e.avatar && e.avatar_color ? { background: e.avatar_color, color: '#fff' } : undefined"
          >
            <img v-if="e.avatar" :src="e.avatar" alt="" />
            <Icon v-else :name="e.icon || 'user-star-line'" :size="20" />
          </span>
          <div class="card-id">
            <strong>{{ e.name }}</strong>
            <span v-if="e.description" class="subtitle">{{ e.description }}</span>
          </div>
        </div>
        <p class="card-desc" :title="e.prompt">{{ e.prompt || "（未填写角色提示词）" }}</p>
        <div class="tags">
          <span class="tag" :title="(e.skills ?? []).join('、')">技能 {{ (e.skills ?? []).length || "不限" }}</span>
          <span class="tag" :title="(e.extensions ?? []).join('、')">MCP {{ (e.extensions ?? []).length || "不限" }}</span>
          <span class="tag" v-if="(e.exclude_tools ?? []).length" :title="'禁用 ' + (e.exclude_tools ?? []).join('/')">
            禁 {{ (e.exclude_tools ?? []).join("/") }}
          </span>
          <span class="tag" v-if="(e.knowledge_dirs ?? []).length">知识库 {{ (e.knowledge_dirs ?? []).length }}</span>
        </div>
        <div class="card-actions">
          <button class="btn primary sm" title="新建任务并雇佣该专家" @click="hire(e)">
            <Icon name="user-star-line" :size="14" />雇佣
          </button>
          <button class="icon-btn sm" title="编辑" @click="openEdit(e)">
            <Icon name="edit-2-line" :size="15" />
          </button>
          <button class="icon-btn sm danger" title="删除" @click="remove(e)">
            <Icon name="delete-bin-line" :size="15" />
          </button>
        </div>
      </div>
    </div>

    <!-- 编辑对话框：分组标签页（基础信息 / 技能 / MCP 工具 / 知识库） -->
    <div v-if="showEditor" class="dialog-mask">
      <div class="dialog">
        <div class="d-head">
          <h2>{{ editingId ? "编辑专家" : "新建专家" }}</h2>
          <button class="icon-btn" title="关闭" @click="showEditor = false">
            <Icon name="close-line" :size="17" />
          </button>
        </div>
        <p class="d-sub">专家配置在<b>会话创建时</b>生效：角色提示词追加在系统提示词之后，白名单约束该会话可用资源。</p>

        <div class="tabs">
          <button
            v-for="t in TABS"
            :key="t.id"
            class="tab"
            :class="{ on: activeTab === t.id }"
            @click="activeTab = t.id"
          >
            {{ t.label }}
          </button>
        </div>

        <!-- 基础信息 -->
        <div v-if="activeTab === 'basic'" class="tab-panel">
          <div class="grid2">
            <div class="field">
              <label>名称 <i>*</i></label>
              <input v-model="form.name" placeholder="如 code-reviewer（也是 /expert: 的调用名，不含空格）" />
            </div>
            <div class="field">
              <label>简介</label>
              <input v-model="form.description" placeholder="一句话说明这个专家擅长什么" />
            </div>
          </div>
        <div class="field">
          <label>头像</label>
          <div class="avatar-edit">
            <span
              class="avatar lg"
              :style="!form.avatar && form.avatar_color ? { background: form.avatar_color, color: '#fff' } : undefined"
            >
              <img v-if="form.avatar" :src="form.avatar" alt="头像预览" />
              <Icon v-else :name="form.icon" :size="26" />
            </span>
            <div class="avatar-ops">
              <button class="btn sm" :disabled="avatarUploading" @click="pickAvatar">
                <Icon name="image-add-line" :size="14" />{{ avatarUploading ? "处理中…" : "上传图片" }}
              </button>
              <button v-if="form.avatar" class="btn sm" @click="form.avatar = ''">移除图片</button>
              <span class="avatar-hint">支持 png / jpg / webp / gif，自动裁成 128×128 圆形</span>
            </div>
          </div>
          <p class="sub-label">或用图标 + 底色（未上传图片时生效）</p>
          <div class="icon-row">
            <button
              v-for="ic in ICON_CHOICES"
              :key="ic"
              class="icon-pick"
              :class="{ on: !form.avatar && form.icon === ic }"
              @click="form.icon = ic"
            >
              <Icon :name="ic" :size="17" />
            </button>
          </div>
          <div class="color-row">
            <button
              v-for="c in AVATAR_COLORS"
              :key="c || 'default'"
              class="color-pick"
              :class="{ on: form.avatar_color === c }"
              :style="c ? { background: c } : undefined"
              :title="c || '默认底色'"
              @click="form.avatar_color = c"
            >
              <Icon v-if="form.avatar_color === c" name="check-line" :size="13" />
            </button>
          </div>
        </div>
          <div class="field">
            <label>角色定位提示词</label>
            <textarea
              v-model="form.prompt"
              rows="7"
              placeholder="你是……。你的职责是……。约束：……（追加在系统提示词之后，不影响基础编码能力）"
            ></textarea>
          </div>
        </div>

        <!-- 技能 -->
        <div v-else-if="activeTab === 'skills'" class="tab-panel">
          <div class="field">
            <label>全局技能白名单（不选 = 全部可用）</label>
            <div class="opt-list tall">
              <label v-for="s in skillOptions" :key="s.name" class="opt" :title="s.description">
                <input type="checkbox" :checked="form.skills.includes(s.name)" @change="toggle(form.skills, s.name)" />
                <span>{{ s.name }}</span>
                <em v-if="s.description" class="opt-desc">{{ s.description }}</em>
              </label>
              <p v-if="!skillOptions.length" class="opt-empty">未安装全局技能</p>
            </div>
          </div>
          <div v-if="editingId" class="field">
            <label>私有技能（仅该专家可用；随专家删除）</label>
            <div class="priv-col">
              <div class="priv-head">
                <span>{{ privates.skills.length }} 个已安装</span>
                <button class="btn sm" @click="openInstall('skill')"><Icon name="download-cloud-2-line" :size="13" />安装</button>
              </div>
              <div v-for="s in privates.skills" :key="s.path" class="priv-item" :title="s.description || s.path">
                <Icon name="magic-line" :size="14" />
                <span>{{ s.name }}</span>
                <button class="icon-btn sm" title="移除" @click="removePrivate('skill', s.name)">
                  <Icon name="close-line" :size="13" />
                </button>
              </div>
              <p v-if="!privates.skills.length" class="opt-empty">暂无私有技能</p>
            </div>
          </div>
          <p v-else class="d-sub">保存后可为该专家安装私有技能（zip/tgz，仅该专家可见）。</p>
        </div>

        <!-- MCP 工具 -->
        <div v-else-if="activeTab === 'mcp'" class="tab-panel">
          <div class="field">
            <label>全局插件 / MCP 白名单（不选 = 全部可用）</label>
            <div class="opt-list tall">
              <label v-for="x in extOptions" :key="x.file" class="opt" :title="x.file">
                <input type="checkbox" :checked="form.extensions.includes(x.name)" @change="toggle(form.extensions, x.name)" />
                <span>{{ x.name }}</span>
              </label>
              <p v-if="!extOptions.length" class="opt-empty">未安装全局插件（MCP 也是插件）</p>
            </div>
          </div>
          <div class="field">
            <label>禁用的内置工具（只读专家可禁掉全部修改类工具）</label>
            <div class="check-row">
              <label v-for="t in EXCLUDE_TOOLS" :key="t.name" class="opt">
                <input type="checkbox" :checked="form.exclude_tools.includes(t.name)" @change="toggleExclude(t.name)" />
                <span>{{ t.name }} · {{ t.label }}</span>
              </label>
            </div>
          </div>
          <div v-if="editingId" class="field">
            <label>私有插件 / MCP（仅该专家可用；随专家删除）</label>
            <div class="priv-col">
              <div class="priv-head">
                <span>{{ privates.extensions.length }} 个已安装</span>
                <button class="btn sm" @click="openInstall('extension')"><Icon name="download-cloud-2-line" :size="13" />安装</button>
              </div>
              <div v-for="x in privates.extensions" :key="x.path" class="priv-item" :title="x.path">
                <Icon name="plug-line" :size="14" />
                <span>{{ x.name }}</span>
                <button class="icon-btn sm" title="移除" @click="removePrivate('extension', x.name)">
                  <Icon name="close-line" :size="13" />
                </button>
              </div>
              <p v-if="!privates.extensions.length" class="opt-empty">暂无私有插件</p>
            </div>
          </div>
          <p v-else class="d-sub">保存后可为该专家安装私有插件 / MCP（单个 .ts/.js 文件）。</p>
        </div>

        <!-- 知识库 -->
        <div v-else class="tab-panel">
          <div class="field">
            <label>知识库目录（会话内注入文件清单，模型按需读取）</label>
            <div class="kb-list">
              <div v-for="(d, i) in form.knowledge_dirs" :key="d" class="kb-item">
                <Icon name="folder-line" :size="15" />
                <span :title="d">{{ d }}</span>
                <button class="icon-btn sm" title="移除" @click="form.knowledge_dirs.splice(i, 1)">
                  <Icon name="close-line" :size="13" />
                </button>
              </div>
              <div class="kb-add">
                <input v-model="newKbDir" placeholder="输入目录绝对路径" @keydown.enter.prevent="addKbDir" />
                <button class="btn sm" @click="addKbDir"><Icon name="folder-add-line" :size="14" />添加</button>
              </div>
              <p class="opt-empty">只注入文件路径清单，不读取内容——模型按相关性用 read 工具自取，避免撑爆上下文。</p>
            </div>
          </div>
        </div>

        <div class="d-foot">
          <button class="btn" @click="showEditor = false">取消</button>
          <button class="btn primary" :disabled="saving" @click="save">
            {{ saving ? "保存中…" : editingId ? "保存" : "创建" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 私有资源安装对话框 -->
    <div v-if="showInstall" class="dialog-mask">
      <div class="dialog narrow">
        <div class="d-head">
          <h2>安装私有{{ installKind === "skill" ? "技能" : "插件" }}</h2>
          <button class="icon-btn" title="关闭" @click="showInstall = false">
            <Icon name="close-line" :size="17" />
          </button>
        </div>
        <p class="d-sub">
          {{
            installKind === "skill"
              ? "支持 zip / .tgz 压缩包（须含 SKILL.md），安装到该专家的私有技能目录。"
              : "支持单个 .ts / .js 源文件（MCP 也是插件），安装到该专家的私有插件目录。"
          }}
        </p>
        <div class="seg">
          <button class="seg-btn" :class="{ active: installMode === 'url' }" @click="installMode = 'url'">网址</button>
          <button class="seg-btn" :class="{ active: installMode === 'local' }" @click="installMode = 'local'">本地</button>
        </div>
        <div v-if="installMode === 'url'" class="field">
          <label>资源网址</label>
          <input v-model="installUrl" placeholder="https://…" />
        </div>
        <div v-else class="field">
          <label>本地文件</label>
          <div class="pick-row">
            <input v-model="installSrcPath" readonly placeholder="点击右侧按钮选择文件" />
            <button class="btn sm" @click="pickInstallFile">选择文件</button>
          </div>
        </div>
        <div class="field">
          <label>名称（可选，缺省从来源推导）</label>
          <input v-model="installName" placeholder="如 my-mcp" />
        </div>
        <div class="d-foot">
          <button class="btn" @click="showInstall = false">取消</button>
          <button class="btn primary" :disabled="installing" @click="doInstall">
            {{ installing ? "安装中…" : "安装" }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.experts-page {
  position: relative;
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 26px 30px;
  background: var(--pd-bg);
}
.page-head {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  max-width: 1080px;
  width: 100%;
  margin: 0 auto 20px;
}
.head-text { flex: 1; }
.head-text h1 {
  display: flex;
  align-items: center;
  gap: 9px;
  margin: 0;
  font-size: 19px;
  color: var(--pd-text);
}
.head-text p {
  margin: 6px 0 0;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.6;
}
.head-actions { display: flex; align-items: center; gap: 8px; }

.notice {
  max-width: 1080px;
  width: 100%;
  margin: 0 auto 12px;
  padding: 8px 12px;
  border-radius: 9px;
  font-size: 12.5px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
}
.notice.err { color: var(--pd-red); border-color: var(--pd-red); }

.empty {
  margin: 60px auto 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  color: var(--pd-text-4);
  font-size: 13px;
  text-align: center;
  max-width: 460px;
  line-height: 1.7;
}

/* 卡片：参考市场页布局——圆形头像 + 名称/副标题 + 描述段落 + 标签胶囊 */
.cards {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 16px;
  max-width: 1080px;
  width: 100%;
  margin: 0 auto;
}
.card {
  display: flex;
  flex-direction: column;
  gap: 11px;
  padding: 18px;
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  background: var(--pd-bg-raised);
}
.card:hover { border-color: var(--pd-border-strong, var(--pd-border)); }
.card-head { display: flex; align-items: center; gap: 12px; }
.avatar {
  width: 42px;
  height: 42px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  background: var(--pd-bg-hover);
  color: var(--pd-accent);
  flex: none;
  overflow: hidden;
}
.avatar img { width: 100%; height: 100%; object-fit: cover; }
.avatar.lg { width: 64px; height: 64px; }
.avatar-edit { display: flex; align-items: center; gap: 14px; }
.avatar-ops { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.avatar-hint { font-size: 11.5px; color: var(--pd-text-4); }
.sub-label { margin: 12px 0 0; font-size: 12px; color: var(--pd-text-4); }
.color-row { display: flex; flex-wrap: wrap; gap: 7px; margin-top: 8px; }
.color-pick {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  border: 2px solid transparent;
  background: var(--pd-bg-hover);
  color: #fff;
  cursor: pointer;
  display: grid;
  place-items: center;
  padding: 0;
}
.color-pick.on { border-color: var(--pd-text); }
.card-id { min-width: 0; }
.card-id strong { display: block; color: var(--pd-text); font-size: 15px; }
.card-id .subtitle {
  display: block;
  font-size: 12px;
  color: var(--pd-text-3);
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-desc {
  margin: 0;
  font-size: 12.5px;
  color: var(--pd-text-3);
  line-height: 1.65;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  min-height: 2.6em;
}
.tags { display: flex; flex-wrap: wrap; gap: 6px; }
.tag {
  font-size: 11px;
  padding: 3px 9px;
  border-radius: 20px;
  background: var(--pd-bg-hover);
  color: var(--pd-text-3);
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.card-actions { display: flex; align-items: center; gap: 4px; margin-top: 2px; }
.card-actions .icon-btn:first-of-type { margin-left: auto; }

.btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  border-radius: 9px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg-raised);
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
}
.btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.btn.primary {
  background: var(--pd-accent);
  border-color: var(--pd-accent);
  color: #fff;
}
.btn.primary:hover { opacity: 0.92; }
.btn.sm { padding: 5px 10px; font-size: 12px; border-radius: 8px; }
.icon-btn {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 8px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.icon-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.icon-btn.sm { width: 28px; height: 28px; }
.icon-btn.danger:hover { color: var(--pd-red); }

.dialog-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: grid;
  place-items: center;
  z-index: 120;
}
.dialog {
  width: min(860px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow-y: auto;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  box-shadow: var(--pd-shadow);
  padding: 20px 22px 18px;
}
.dialog.narrow { width: min(560px, calc(100vw - 48px)); }
.d-head { display: flex; align-items: center; gap: 12px; }
.d-head h2 { margin: 0; font-size: 17px; font-weight: 700; color: var(--pd-text); flex: 1; }
.d-sub { margin: 6px 0 0; font-size: 12.5px; color: var(--pd-text-3); line-height: 1.6; }

/* 对话框分组标签 */
.tabs {
  display: flex;
  gap: 4px;
  margin-top: 16px;
  border-bottom: 1px solid var(--pd-border);
}
.tab {
  padding: 8px 14px 9px;
  font-size: 13px;
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
  color: var(--pd-text-3);
  cursor: pointer;
}
.tab:hover { color: var(--pd-text); }
.tab.on { color: var(--pd-accent); border-bottom-color: var(--pd-accent); }
.tab-panel { padding-top: 4px; }

.field { margin-top: 15px; }
.field label {
  display: block;
  font-size: 13px;
  color: var(--pd-text-2);
  margin-bottom: 7px;
}
.field label i { color: var(--pd-red); font-style: normal; }
.field input, .field textarea, .field select {
  width: 100%;
  box-sizing: border-box;
  padding: 8px 11px;
  border-radius: 9px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg);
  color: var(--pd-text);
  font-size: 13px;
  font-family: inherit;
}
.field input[type="checkbox"] {
  width: auto;
  padding: 0;
  accent-color: var(--pd-accent);
}
.field textarea { resize: vertical; line-height: 1.6; }
.field input:focus, .field textarea:focus, .field select:focus {
  outline: none;
  border-color: var(--pd-accent);
}
.grid2 { display: grid; grid-template-columns: 1fr 1fr; gap: 0 14px; }

.icon-row { display: flex; flex-wrap: wrap; gap: 7px; }
.icon-pick {
  width: 34px;
  height: 34px;
  display: grid;
  place-items: center;
  border-radius: 9px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg);
  color: var(--pd-text-3);
  cursor: pointer;
}
.icon-pick.on {
  border-color: var(--pd-accent);
  color: var(--pd-accent);
  background: var(--pd-bg-hover);
}

.opt-list {
  overflow-y: auto;
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.opt-list.tall { max-height: 300px; }
/* label.opt 提升元素选择器：scoped 下 .field label[data-v] (0,2,1) 会压过 .opt[data-v] (0,2,0) */
label.opt {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 8px;
  border-radius: 6px;
  font-size: 12.5px;
  color: var(--pd-text-2);
  cursor: pointer;
}
label.opt:hover { background: var(--pd-bg-hover); }
.opt .opt-desc {
  flex: 1;
  min-width: 0;
  font-style: normal;
  font-size: 11.5px;
  color: var(--pd-text-4);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: right;
}
.opt-empty { margin: 4px 6px; font-size: 12px; color: var(--pd-text-4); }
.check-row { display: flex; gap: 18px; }

.kb-list { display: flex; flex-direction: column; gap: 7px; }
.kb-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--pd-text-2);
  padding: 7px 10px;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
}
.kb-item span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.kb-add { display: flex; gap: 8px; }
.kb-add input {
  flex: 1;
  padding: 7px 10px;
  border-radius: 8px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg);
  color: var(--pd-text);
  font-size: 12.5px;
}

.priv-col {
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.priv-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12.5px;
  color: var(--pd-text-2);
}
.priv-item {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12.5px;
  color: var(--pd-text-2);
  padding: 5px 6px;
  border-radius: 6px;
}
.priv-item:hover { background: var(--pd-bg-hover); }
.priv-item span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.d-foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 20px;
}
.seg { display: inline-flex; gap: 0; margin-top: 14px; border: 1px solid var(--pd-border); border-radius: 9px; overflow: hidden; }
.seg-btn {
  padding: 7px 16px;
  font-size: 12.5px;
  border: none;
  background: none;
  color: var(--pd-text-3);
  cursor: pointer;
}
.seg-btn.active { background: var(--pd-accent); color: #fff; }
.pick-row { display: flex; gap: 8px; }
.pick-row input { flex: 1; }
</style>
