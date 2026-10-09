<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { basename, fmtBytes } from "../utils/time.js";
import { FOLDER_PICKER, GIT_API } from "../databus.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

const props = withDefaults(
  defineProps<{
    busy: boolean;
    disabled?: boolean;
    disabledHint?: string;
    model?: string;
    placeholder?: string;
    centered?: boolean;
    preset?: string;
    /** 可选的项目目录（首页发送时用于新建会话） */
    projects?: string[];
    /** 预选的项目目录（如从侧栏项目分组进入）；cwd 每次携带新 seq 以支持重复点击重新应用 */
    presetCwd?: { cwd: string; seq: number } | null;
    /** 权限模式（完全访问/自动编辑/变更前确认/计划模式） */
    permissionMode?: string;
    /** 思考级别（off/minimal/low/medium/high/xhigh/max） */
    thinkingLevel?: string;
    /** 当前模型可用的思考档位（session.thinking_info；null/缺省 = 未知，全部展示） */
    thinkingAvailable?: string[] | null;
    /** prompts 模板列表加载器（/ 弹层命令段用，来自 store） */
    promptsLoader?: () => Promise<Array<{ name: string; description: string; argument_hint: string }>>;
    /** 可切换的模型名（来自本工作区的会话） */
    models?: string[];
    /** @ 提及的基准目录（会话 cwd 或主目录）；首页未选项目时兜底 */
    mentionCwd?: string;
    /** @ 提及的文件列表加载器（来自 store，走 host/中继） */
    mentionLoader?: (cwd: string) => Promise<Array<{ path: string; name: string; dir: boolean }>>;
    /** 技能列表加载器（+ 菜单 $ 选择技能用，来自 store） */
    skillsLoader?: () => Promise<Array<{ name: string; description: string }>>;
    /** 专家列表加载器（/ 弹层专家段用，来自 store） */
    expertsLoader?: () => Promise<Array<{ id: string; name: string; description?: string; icon?: string; avatar?: string }>>;
    /** 已雇佣的专家（新任务模式绑定到将创建的会话；由父级管理） */
    hiredExpert?: { id: string; name: string } | null;
    /** 当前模型是否支持图片输入（false = 显示预警；undefined = 未知不预警） */
    modelImagesOk?: boolean | null;
    /** 草稿模式（表单内嵌）：send 只上报不清空，Enter/发送由父级决定语义（如保存） */
    draft?: boolean;
  }>(),
  { placeholder: "输入消息，Enter 发送，Shift+Enter 换行" },
);
const emit = defineEmits<{
  send: [
    text: string,
    cwd?: string | null,
    images?: Array<{ data: string; mime_type: string }>,
    files?: Array<{ name: string; mime_type: string; size: number; data: string }>,
  ];
  abort: [];
  setPermissionMode: [mode: string];
  setThinkingLevel: [level: string];
  setModel: [model: string];
  openProviders: [];
  /** 雇佣专家（新任务模式下从 / 弹层选定，绑定到将创建的会话） */
  hire: [expert: { id: string; name: string }];
  /** 取消雇佣 */
  unhire: [];
}>();

const I = {
  folder: "folder-line",
  folderAdd: "folder-add-line",
  branch: "git-branch-line",
  chevD: "arrow-down-s-line",
  x: "close-line",
  check: "check-line",
  search: "search-line",
  chat: "chat-1-line",
  plus: "add-line",
  up: "arrow-up-line",
  stop: "stop-line",
  bulb: "lightbulb-line",
  hand: "chat-check-line",
  shieldCheck: "shield-check-line",
  shield: "shield-flash-line",
  gear: "settings-3-line",
  gauge: "dashboard-2-line",
  img: "image-add-line",
  fileAdd: "file-add-line",
  magic: "magic-line",
};

const text = ref("");
const menuOpen = ref(false);
const menuQuery = ref("");
/** 当前选中的项目目录；null = 不在项目中工作（新建时默认不选项目） */
const selected = ref<string | null>(null);
const adding = ref(false);
const newPath = ref("");
const rootEl = ref<HTMLElement | null>(null);
/** 桌面端注入系统目录选择器；web / 未注入时回退到行内路径输入 */
const folderPicker = inject(FOLDER_PICKER, null);
/** git 分支信息与切换（CoreCtx 命令面）；未注入（理论上不会）时不显示分支 chip */
const gitApi = inject(GIT_API, null);

/** 底栏下拉状态 */
const plusOpen = ref(false);
const permOpen = ref(false);
const thinkOpen = ref(false);
const modelOpen = ref(false);

// ---- + 菜单：图片附件 / 文档附件 / 文件提及 / 技能 ----
interface PendingImage {
  data: string; // base64，不带 data: 前缀
  mime_type: string;
  name: string;
}
const pendingImages = ref<PendingImage[]>([]);
const fileInput = ref<HTMLInputElement | null>(null);
const skills = ref<Array<{ name: string; description: string }>>([]);
const skillsLoading = ref(false);
const MAX_IMAGES = 6;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;

/** 文档附件（txt/pdf/office 等）：整文件 base64 上报，host 提取文本注入 prompt */
interface PendingFile {
  name: string;
  mime_type: string;
  size: number;
  data: string; // base64，不带 data: 前缀
}
const pendingFiles = ref<PendingFile[]>([]);
const docInput = ref<HTMLInputElement | null>(null);
const docWarn = ref("");
const MAX_FILES = 6;
const MAX_FILE_BYTES = 20 * 1024 * 1024;

function ensureSkills(): void {
  if (!skillsLoading.value && skills.value.length === 0 && props.skillsLoader) {
    skillsLoading.value = true;
    props
      .skillsLoader()
      .then((list) => (skills.value = list))
      .catch(() => (skills.value = []))
      .finally(() => (skillsLoading.value = false));
  }
}

function togglePlus(): void {
  plusOpen.value = !plusOpen.value;
  permOpen.value = false;
  thinkOpen.value = false;
  modelOpen.value = false;
  mention.value = null; // + 菜单与提及弹层互斥
  slash.value = null; // 与 / 技能弹层互斥
  if (plusOpen.value) ensureSkills();
}

function pickImages(): void {
  fileInput.value?.click();
  plusOpen.value = false;
}

function onFilesPicked(e: Event): void {
  const input = e.target as HTMLInputElement;
  const slots = Math.max(0, MAX_IMAGES - pendingImages.value.length);
  for (const f of [...(input.files ?? [])].slice(0, slots)) {
    if (!f.type.startsWith("image/") || f.size > MAX_IMAGE_BYTES) continue;
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result ?? "");
      const base64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
      if (base64) {
        pendingImages.value.push({ data: base64, mime_type: f.type || "image/png", name: f.name });
      }
    };
    reader.readAsDataURL(f);
  }
  input.value = "";
}

function removeImage(i: number): void {
  pendingImages.value.splice(i, 1);
}

function pickFiles(): void {
  docWarn.value = "";
  docInput.value?.click();
  plusOpen.value = false;
}

function onDocsPicked(e: Event): void {
  const input = e.target as HTMLInputElement;
  const slots = Math.max(0, MAX_FILES - pendingFiles.value.length);
  let picked = 0;
  for (const f of input.files ?? []) {
    if (picked >= slots) {
      docWarn.value = `最多 ${MAX_FILES} 个附件，多余文件已忽略`;
      break;
    }
    if (f.size > MAX_FILE_BYTES) {
      docWarn.value = `${f.name} 超过 20 MB，已跳过`;
      continue;
    }
    picked++;
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result ?? "");
      const base64 = dataUrl.slice(dataUrl.indexOf(",") + 1);
      if (base64) {
        pendingFiles.value.push({ name: f.name, mime_type: f.type || "", size: f.size, data: base64 });
      }
    };
    reader.readAsDataURL(f);
  }
  input.value = "";
}

function removeFile(i: number): void {
  pendingFiles.value.splice(i, 1);
}

/** 在光标处插入文本并移动光标 */
function insertAtCaret(s: string): void {
  const el = ta.value;
  const caret = el ? (el.selectionStart ?? text.value.length) : text.value.length;
  text.value = text.value.slice(0, caret) + s + text.value.slice(caret);
  void nextTick(() => {
    const t = ta.value;
    if (t) {
      t.focus();
      const pos = caret + s.length;
      t.setSelectionRange(pos, pos);
    }
  });
}

/** 插入 @ 并打开文件提及弹层 */
function addFileMention(): void {
  plusOpen.value = false;
  slash.value = null;
  if (!mentionBase.value) return;
  const el = ta.value;
  const caret = el ? (el.selectionStart ?? text.value.length) : text.value.length;
  text.value = text.value.slice(0, caret) + "@" + text.value.slice(caret);
  mention.value = { start: caret, query: "", active: 0 };
  void ensureMentionFiles();
  void nextTick(() => ta.value?.focus());
}

/** 选择技能：插入 pi 的技能命令（/skill:name args） */
function pickSkill(name: string): void {
  plusOpen.value = false;
  insertAtCaret(`/skill:${name} `);
}

const PERMISSION_ITEMS = [
  { value: "plan", label: "计划模式", desc: "编辑前先出计划。", icon: I.bulb },
  { value: "confirm", label: "变更前确认", desc: "改文件前先问我。", icon: I.hand },
  { value: "edit-auto", label: "自动编辑", desc: "自动编辑文件。", icon: I.shieldCheck },
  { value: "full", label: "完全访问", desc: "减少确认次数。", icon: I.shield },
];
const THINKING_ITEMS = [
  { value: "off", label: "关闭" },
  { value: "minimal", label: "最低" },
  { value: "low", label: "低" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
  { value: "xhigh", label: "更高" },
  { value: "max", label: "最高" },
];
/** 按模型能力过滤后的档位（能力未知时全部展示；xhigh/max 仅在模型声明支持时出现） */
const thinkItems = computed(() => {
  const avail = props.thinkingAvailable;
  if (!avail || !avail.length) return THINKING_ITEMS;
  return THINKING_ITEMS.filter((i) => avail.includes(i.value));
});
const permLabel = computed(
  () => PERMISSION_ITEMS.find((i) => i.value === props.permissionMode)?.label ?? "计划模式",
);
const permIcon = computed(
  () => PERMISSION_ITEMS.find((i) => i.value === props.permissionMode)?.icon ?? I.bulb,
);
const thinkLabel = computed(() => {
  const hit = THINKING_ITEMS.find((i) => i.value === props.thinkingLevel);
  return hit?.label ?? (props.thinkingLevel ? String(props.thinkingLevel) : "中");
});

function selectPerm(mode: string): void {
  permOpen.value = false;
  emit("setPermissionMode", mode);
}
function selectThink(level: string): void {
  thinkOpen.value = false;
  emit("setThinkingLevel", level);
}
function toggleModelMenu(): void {
  modelOpen.value = !modelOpen.value;
  permOpen.value = false;
  thinkOpen.value = false;
  modelQuery.value = "";
}
function selectModel(model: string): void {
  modelOpen.value = false;
  modelQuery.value = "";
  emit("setModel", model);
}

/** 模型菜单：按 provider 分组（provider/id → 组标题 + 模型 id），支持过滤 */
const modelQuery = ref("");
const modelGroups = computed(() => {
  const q = modelQuery.value.trim().toLowerCase();
  const groups = new Map<string, Array<{ id: string; full: string }>>();
  for (const m of props.models ?? []) {
    if (q && !m.toLowerCase().includes(q)) continue;
    const slash = m.indexOf("/");
    const provider = slash > 0 ? m.slice(0, slash) : "其他";
    const id = slash > 0 ? m.slice(slash + 1) : m;
    const list = groups.get(provider) ?? [];
    list.push({ id, full: m });
    groups.set(provider, list);
  }
  return [...groups.entries()].map(([provider, items]) => ({ provider, items }));
});
function selectFirstModel(): void {
  const first = modelGroups.value[0]?.items[0];
  if (first) selectModel(first.full);
}

// ---- @ 文件提及 ----
interface MentionFile {
  path: string;
  name: string;
  dir: boolean;
}
const ta = ref<HTMLTextAreaElement | null>(null);
/** 弹层状态：start = 触发的 @ 在文本中的下标 */
const mention = ref<{ start: number; query: string; active: number } | null>(null);
const mentionFiles = ref<MentionFile[]>([]);
const mentionLoadedFor = ref<string | null>(null);
const mentionLoading = ref(false);
/** 基准目录：首页优先用所选项目，否则用会话 cwd / 主目录兜底 */
const mentionBase = computed(() => (props.centered ? selected.value : null) ?? props.mentionCwd ?? null);

const mentionMatches = computed(() => {
  if (!mention.value) return [];
  const q = mention.value.query.toLowerCase();
  return mentionFiles.value
    .filter((f) => f.path.toLowerCase().includes(q))
    .sort((a, b) => (a.dir === b.dir ? a.path.localeCompare(b.path) : a.dir ? -1 : 1))
    .slice(0, 20);
});

watch(mentionBase, () => {
  // 目录切换后强制重取列表
  mentionLoadedFor.value = null;
  mentionFiles.value = [];
});

async function ensureMentionFiles(): Promise<void> {
  const base = mentionBase.value;
  if (!base || !props.mentionLoader || mentionLoadedFor.value === base || mentionLoading.value) return;
  mentionLoading.value = true;
  try {
    mentionFiles.value = await props.mentionLoader(base);
    mentionLoadedFor.value = base;
  } catch {
    mentionFiles.value = [];
  } finally {
    mentionLoading.value = false;
  }
}

/** 检测光标前的 @ 触发式样：行首或空白后的 @query；同时识别 / 技能命令触发 */
function onTextInput(e: Event): void {
  const el = e.target as HTMLTextAreaElement;
  const caret = el.selectionStart ?? 0;
  const before = text.value.slice(0, caret);
  const m = before.match(/(^|\s)@([^\s@]*)$/);
  if (m && mentionBase.value) {
    mention.value = { start: caret - m[2]!.length - 1, query: m[2]!, active: 0 };
    slash.value = null;
    void ensureMentionFiles();
    return;
  }
  if (mention.value) {
    mention.value = null;
  }
  // / 技能命令：/ 与 /skill:、/expert: 三种写法都触发，过滤时忽略 skill:/expert: 前缀
  const sm = before.match(/(^|\s)\/([^\s/]*)$/);
  if (sm && (props.skillsLoader || props.expertsLoader || props.promptsLoader)) {
    slash.value = { start: caret - sm[2]!.length - 1, raw: sm[2]!, active: 0 };
    ensureSkills();
    ensureExperts();
    ensurePrompts();
  } else if (slash.value) {
    slash.value = null;
  }
}

/** 把选中路径替换进文本（替换 @query 段），光标落在 token 之后 */
function applyMention(f: MentionFile): void {
  if (!mention.value) return;
  const start = mention.value.start;
  const end = start + 1 + mention.value.query.length;
  const token = `@${f.path} `;
  text.value = text.value.slice(0, start) + token + text.value.slice(end);
  mention.value = null;
  void nextTick(() => {
    const el = ta.value;
    if (el) {
      el.focus();
      const pos = start + token.length;
      el.setSelectionRange(pos, pos);
    }
  });
}

/** 键盘导航时保持活动项可见 */
watch(
  () => mention.value?.active,
  async () => {
    await nextTick();
    document.querySelector(".mention-item.active")?.scrollIntoView({ block: "nearest" });
  },
);

// ---- / 技能命令弹层（与 @ 提及同款交互） ----
const slash = ref<{ start: number; raw: string; active: number } | null>(null);
const slashQuery = computed(() => (slash.value?.raw ?? "").replace(/^(skill|expert):/i, ""));
/** 弹层条目：技能插入 /skill: 命令；专家在首页/新任务 hiring，会话中插入 /expert: 咨询命令；prompts 插入 /name（host 端展开模板） */
type SlashItem =
  | { kind: "skill"; name: string; description: string }
  | { kind: "prompt"; name: string; description: string }
  | { kind: "expert"; id: string; name: string; description: string; avatar?: string };
const experts = ref<Array<{ id: string; name: string; description?: string; icon?: string; avatar?: string }>>([]);
const expertsLoading = ref(false);
const prompts = ref<Array<{ name: string; description: string; argument_hint: string }>>([]);
const promptsLoading = ref(false);

function ensureExperts(): void {
  if (!expertsLoading.value && experts.value.length === 0 && props.expertsLoader) {
    expertsLoading.value = true;
    props
      .expertsLoader()
      .then((list) => (experts.value = list))
      .catch(() => {})
      .finally(() => (expertsLoading.value = false));
  }
}

function ensurePrompts(): void {
  if (!promptsLoading.value && prompts.value.length === 0 && props.promptsLoader) {
    promptsLoading.value = true;
    props
      .promptsLoader()
      .then((list) => (prompts.value = list))
      .catch(() => {})
      .finally(() => (promptsLoading.value = false));
  }
}

const slashMatches = computed<SlashItem[]>(() => {
  if (!slash.value) return [];
  const q = slashQuery.value.toLowerCase();
  const skillHits: SlashItem[] = skills.value
    .filter((s) => !q || s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q))
    .slice(0, 20)
    .map((s) => ({ kind: "skill", name: s.name, description: s.description }));
  const promptHits: SlashItem[] = prompts.value
    .filter((p) => !q || p.name.toLowerCase().includes(q) || p.description.toLowerCase().includes(q))
    .slice(0, 20)
    .map((p) => ({ kind: "prompt", name: p.name, description: p.description }));
  const expertHits: SlashItem[] = experts.value
    .filter((e) => !q || e.name.toLowerCase().includes(q) || (e.description ?? "").toLowerCase().includes(q))
    .slice(0, 10)
    .map((e) => ({ kind: "expert", id: e.id, name: e.name, description: e.description ?? "", avatar: e.avatar }));
  return [...skillHits, ...promptHits, ...expertHits];
});

/** 把 /query 段替换为完整的技能/模板命令，光标落在其后 */
function applySlash(s: SlashItem): void {
  if (!slash.value) return;
  const start = slash.value.start;
  const end = start + 1 + slash.value.raw.length;
  if (s.kind === "expert") {
    // 首页/新任务 = 雇佣（绑定到将创建的会话，token 移除、chip 展示）；
    // 会话中 = 咨询（插入 /expert:name 命令，host 按需注入角色提示词）。
    if (props.centered) {
      text.value = text.value.slice(0, start) + text.value.slice(end);
      slash.value = null;
      emit("hire", { id: s.id, name: s.name });
      void nextTick(() => ta.value?.focus());
      return;
    }
    const token = `/expert:${s.name} `;
    text.value = text.value.slice(0, start) + token + text.value.slice(end);
    slash.value = null;
    void nextTick(() => {
      const el = ta.value;
      if (el) {
        el.focus();
        const pos = start + token.length;
        el.setSelectionRange(pos, pos);
      }
    });
    return;
  }
  // prompts 模板：插入 /name（host prompt() 默认展开模板，与 pi /命令一致）
  const token = s.kind === "skill" ? `/skill:${s.name} ` : `/${s.name} `;
  text.value = text.value.slice(0, start) + token + text.value.slice(end);
  slash.value = null;
  void nextTick(() => {
    const el = ta.value;
    if (el) {
      el.focus();
      const pos = start + token.length;
      el.setSelectionRange(pos, pos);
    }
  });
}

watch(
  () => slash.value?.active,
  async () => {
    await nextTick();
    document.querySelector(".mention-item.active")?.scrollIntoView({ block: "nearest" });
  },
);

const CUSTOM_KEY = "pidock.customProjects";
const customProjects = ref<string[]>(readCustom());
function readCustom(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(CUSTOM_KEY) ?? "[]");
    return Array.isArray(raw) ? raw.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

const allProjects = computed(() => {
  const set = new Set<string>(customProjects.value);
  for (const p of props.projects ?? []) if (p) set.add(p);
  return [...set];
});
const filteredProjects = computed(() => {
  const s = menuQuery.value.trim().toLowerCase();
  if (!s) return allProjects.value;
  return allProjects.value.filter((p) => `${basename(p)} ${p}`.toLowerCase().includes(s));
});

const chipLabel = computed(() => (selected.value ? basename(selected.value) : "不在项目中工作"));

watch(
  () => props.preset,
  (v) => {
    if (v) text.value = v;
  },
  { immediate: true },
);
// 外部预选项目目录（immediate：进入首页重新挂载时也要应用）
watch(
  () => props.presetCwd,
  (v) => {
    if (v?.cwd) selected.value = v.cwd;
  },
  { immediate: true },
);

function pick(p: string): void {
  selected.value = p;
  menuOpen.value = false;
  menuQuery.value = "";
}
function selectNone(): void {
  selected.value = null;
  menuOpen.value = false;
  menuQuery.value = "";
}
function clearSelected(): void {
  selected.value = null;
}

function addCustom(p: string): void {
  if (!customProjects.value.includes(p)) {
    const next = [...customProjects.value, p];
    customProjects.value = next;
    try {
      localStorage.setItem(CUSTOM_KEY, JSON.stringify(next));
    } catch {
      // localStorage 不可用时仅本次会话内生效
    }
  }
  pick(p);
}

async function onOpenFolder(): Promise<void> {
  if (folderPicker) {
    try {
      const p = await folderPicker();
      if (p) addCustom(p);
    } catch {
      adding.value = true; // 选择器失败时回退到行内输入
    }
    return;
  }
  adding.value = true;
}

function addFolder(): void {
  const p = newPath.value.trim();
  if (!p) return;
  addCustom(p);
  adding.value = false;
  newPath.value = "";
}

// ---- git 分支选择（所选项目目录是 git 仓库时显示分支 chip） ----
const gitBranch = ref<string | null>(null);
const gitIsRepo = ref(false);
const gitDetached = ref(false);
const branchOpen = ref(false);
const branchQuery = ref("");
const branchList = ref<Array<{ name: string; current: boolean }>>([]);
const branchLoading = ref(false);
const branchBusy = ref(false);
const branchErr = ref("");
const creatingBranch = ref(false);
const newBranchName = ref("");

/** 拉取所选目录的 git 信息；非仓库 / git 不可用都按无 git 信息处理 */
async function refreshGitInfo(): Promise<void> {
  const cwd = selected.value;
  if (!cwd || !gitApi || !props.centered) {
    gitIsRepo.value = false;
    gitBranch.value = null;
    return;
  }
  try {
    const info = await gitApi.info(cwd);
    if (selected.value !== cwd) return; // 期间目录已切换
    gitIsRepo.value = info.is_repo;
    gitBranch.value = info.branch;
    gitDetached.value = info.detached;
  } catch {
    if (selected.value === cwd) {
      gitIsRepo.value = false;
      gitBranch.value = null;
    }
  }
}
watch(selected, refreshGitInfo, { immediate: true });

async function refreshBranches(): Promise<void> {
  const cwd = selected.value;
  if (!cwd || !gitApi) return;
  branchLoading.value = true;
  try {
    const res = await gitApi.branches(cwd);
    if (selected.value !== cwd) return;
    branchList.value = res.branches;
  } catch (e) {
    branchErr.value = e instanceof Error ? e.message : String(e);
  } finally {
    branchLoading.value = false;
  }
}

const filteredBranches = computed(() => {
  const q = branchQuery.value.trim().toLowerCase();
  if (!q) return branchList.value;
  return branchList.value.filter((b) => b.name.toLowerCase().includes(q));
});

function toggleProjectMenu(): void {
  menuOpen.value = !menuOpen.value;
  branchOpen.value = false;
}

async function toggleBranchMenu(): Promise<void> {
  branchOpen.value = !branchOpen.value;
  if (!branchOpen.value) return;
  menuOpen.value = false;
  adding.value = false;
  branchQuery.value = "";
  branchErr.value = "";
  creatingBranch.value = false;
  await refreshBranches();
}

/** 检出分支；已在当前分支时仅收起菜单 */
async function checkoutBranch(name: string): Promise<void> {
  const cwd = selected.value;
  if (!cwd || !gitApi || branchBusy.value) return;
  if (branchList.value.find((b) => b.name === name)?.current) {
    branchOpen.value = false;
    return;
  }
  branchBusy.value = true;
  branchErr.value = "";
  try {
    await gitApi.checkout(cwd, name);
    await Promise.all([refreshGitInfo(), refreshBranches()]);
    branchOpen.value = false;
  } catch (e) {
    branchErr.value = e instanceof Error ? e.message : String(e);
  } finally {
    branchBusy.value = false;
  }
}

/** 创建并检出新分支 */
async function createBranch(): Promise<void> {
  const cwd = selected.value;
  const name = newBranchName.value.trim();
  if (!cwd || !gitApi || !name || branchBusy.value) return;
  branchBusy.value = true;
  branchErr.value = "";
  try {
    await gitApi.checkout(cwd, name, true);
    await Promise.all([refreshGitInfo(), refreshBranches()]);
    branchOpen.value = false;
    creatingBranch.value = false;
    newBranchName.value = "";
  } catch (e) {
    branchErr.value = e instanceof Error ? e.message : String(e);
  } finally {
    branchBusy.value = false;
  }
}

function onDocClick(e: MouseEvent): void {
  if (rootEl.value && !rootEl.value.contains(e.target as Node)) {
    menuOpen.value = false;
    adding.value = false;
    menuQuery.value = "";
    permOpen.value = false;
    thinkOpen.value = false;
    modelOpen.value = false;
    plusOpen.value = false;
    modelQuery.value = "";
    branchOpen.value = false;
    creatingBranch.value = false;
    branchErr.value = "";
  }
}
onMounted(() => document.addEventListener("click", onDocClick));
onBeforeUnmount(() => document.removeEventListener("click", onDocClick));

/** 供表单父级（如定时任务弹窗的「创建」按钮）触发草稿提交 */
defineExpose({ submit });

function submit(): boolean {
  const value = text.value.trim();
  if (!value || props.busy || props.disabled) return false;
  const cwd = props.centered ? selected.value : undefined;
  const images = pendingImages.value.map((p) => ({ data: p.data, mime_type: p.mime_type }));
  const files = pendingFiles.value.map((p) => ({
    name: p.name,
    mime_type: p.mime_type,
    size: p.size,
    data: p.data,
  }));
  emit("send", value, cwd === null ? null : cwd || undefined, images.length ? images : undefined, files.length ? files : undefined);
  if (props.draft) return true; // 草稿模式：内容归父级表单所有，成功与否由父级决定
  text.value = "";
  pendingImages.value = [];
  pendingFiles.value = [];
  docWarn.value = "";
  slash.value = null;
  return true;
}

function onKeydown(e: KeyboardEvent): void {
  // @ 提及弹层打开时：方向键选择、Enter 确认、Esc 关闭（优先于发送）
  if (mention.value && mentionMatches.value.length) {
    const n = mentionMatches.value.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      mention.value.active = (mention.value.active + 1) % n;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      mention.value.active = (mention.value.active - 1 + n) % n;
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      applyMention(mentionMatches.value[mention.value.active] ?? mentionMatches.value[0]!);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      mention.value = null;
      return;
    }
  }
  // / 技能命令弹层：同款键盘交互
  if (slash.value && slashMatches.value.length) {
    const n = slashMatches.value.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      slash.value.active = (slash.value.active + 1) % n;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      slash.value.active = (slash.value.active - 1 + n) % n;
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      applySlash(slashMatches.value[slash.value.active] ?? slashMatches.value[0]!);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      slash.value = null;
      return;
    }
  }
  // 输入法组词中的 Enter 是"确认候选词"，不是发送/保存
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    submit();
  }
}
</script>

<template>
  <div ref="rootEl" class="composer" :class="{ centered }" :title="disabled ? disabledHint : ''">
    <div v-if="centered" class="c-chips">
      <div class="c-folder" @click="toggleProjectMenu">
        <template v-if="selected">
          <span class="c-clear" title="清除项目选择" @click.stop="clearSelected">
            <Icon :name="I.x" :size="11" />
          </span>
          <Icon :name="I.folder" :size="14" />
          <span class="c-name">{{ chipLabel }}</span>
        </template>
        <template v-else>
          <Icon :name="I.chat" :size="14" />
          <span class="c-name">不在项目中工作</span>
        </template>
        <span class="c-chev" :class="{ open: menuOpen }"><Icon :name="I.chevD" :size="12" /></span>

        <!-- 项目菜单：搜索 / 项目列表 / 打开文件夹 / 不在项目中工作 -->
        <div v-if="menuOpen" class="c-menu" @click.stop>
          <div class="c-search">
            <Icon :name="I.search" :size="14" />
            <input v-model="menuQuery" placeholder="搜索工作区" @keydown.enter="filteredProjects[0] && pick(filteredProjects[0]!)" />
          </div>
          <div class="c-list">
            <button
              v-for="p in filteredProjects"
              :key="p"
              class="c-item"
              :class="{ on: p === selected }"
              :title="p"
              @click="pick(p)"
            >
              <Icon :name="I.folder" :size="14" />
              <span class="c-item-name">{{ basename(p) }}</span>
              <Icon v-if="p === selected" class="c-check" :name="I.check" :size="14" />
            </button>
            <div v-if="!filteredProjects.length" class="c-empty">没有匹配的项目</div>
          </div>
          <div class="c-sep"></div>
          <template v-if="adding">
            <div class="c-addrow">
              <input v-model="newPath" placeholder="文件夹绝对路径，如 D:\project\demo" @keydown.enter="addFolder" />
              <button class="c-addbtn" @click="addFolder">添加</button>
            </div>
          </template>
          <button v-else class="c-item c-action" @click="onOpenFolder">
            <Icon :name="I.folderAdd" :size="14" />
            <span>打开文件夹</span>
          </button>
          <button class="c-item c-action" :class="{ on: !selected }" @click="selectNone">
            <Icon :name="I.chat" :size="14" />
            <span>不在项目中工作</span>
          </button>
        </div>
      </div>

      <!-- 分支 chip：所选目录是 git 仓库时显示（检测 / 列表 / 检出走 CoreCtx git 命令） -->
      <div
        v-if="selected && gitIsRepo && gitApi"
        class="c-branch"
        :title="gitDetached ? 'HEAD 处于游离状态（未在任何分支上）' : ''"
        @click="toggleBranchMenu()"
      >
        <Icon :name="I.branch" :size="14" />
        <span class="c-name">{{ gitBranch ?? "HEAD" }}</span>
        <span class="c-chev" :class="{ open: branchOpen }"><Icon :name="I.chevD" :size="12" /></span>

        <!-- 分支菜单：搜索 / 本地分支列表（当前分支打勾）/ 创建并检出新分支 -->
        <div v-if="branchOpen" class="c-menu c-branch-menu" @click.stop>
          <div class="c-search">
            <Icon :name="I.search" :size="14" />
            <input v-model="branchQuery" placeholder="搜索分支" />
          </div>
          <div class="c-group">分支</div>
          <div class="c-list">
            <button
              v-for="b in filteredBranches"
              :key="b.name"
              class="c-item"
              :class="{ on: b.current }"
              :disabled="branchBusy"
              @click="checkoutBranch(b.name)"
            >
              <Icon :name="I.branch" :size="14" />
              <span class="c-item-name">{{ b.name }}</span>
              <Icon v-if="b.current" class="c-check" :name="I.check" :size="14" />
            </button>
            <div v-if="!filteredBranches.length" class="c-empty">
              {{ branchLoading ? "加载中..." : "没有匹配的分支" }}
            </div>
          </div>
          <div class="c-sep"></div>
          <template v-if="creatingBranch">
            <div class="c-addrow">
              <input v-model="newBranchName" placeholder="新分支名称" @keydown.enter="createBranch" />
              <button class="c-addbtn" :disabled="branchBusy" @click="createBranch">创建</button>
            </div>
          </template>
          <button v-else class="c-item c-action" @click="creatingBranch = true; newBranchName = branchQuery.trim()">
            <Icon :name="I.plus" :size="14" />
            <span>创建并检出新分支...</span>
          </button>
          <div v-if="branchErr" class="c-branch-err">{{ branchErr }}</div>
        </div>
      </div>
    </div>

    <textarea
      ref="ta"
      v-model="text"
      rows="2"
      :disabled="disabled"
      :placeholder="disabled ? disabledHint : placeholder"
      @keydown="onKeydown"
      @input="onTextInput"
    />
    <!-- @ 文件提及弹层 -->
    <div v-if="mention && mentionMatches.length" class="mention-menu">
      <button
        v-for="(f, i) in mentionMatches"
        :key="f.path"
        class="mention-item"
        :class="{ active: i === mention.active }"
        :title="f.path"
        @mousedown.prevent
        @mouseenter="mention.active = i"
        @click="applyMention(f)"
      >
        <span class="m-icon"><FileIcon :path="f.path" :dir="f.dir" :size="16" /></span>
        <span class="m-name">{{ f.name }}</span>
        <span class="m-dir">{{ f.path }}</span>
      </button>
    </div>

    <!-- / 技能命令 + 模板 + 专家弹层 -->
    <div v-if="slash && slashMatches.length" class="mention-menu">
      <template v-for="(s, i) in slashMatches" :key="s.kind === 'skill' ? `s:${s.name}` : s.kind === 'prompt' ? `p:${s.name}` : `e:${s.id}`">
        <!-- 模板段标题：仅紧随技能段（或位于最前）显示一次 -->
        <div
          v-if="s.kind === 'prompt' && (i === 0 || slashMatches[i - 1]?.kind === 'skill')"
          class="mention-group"
        >
          命令模板 · ~/.pi/agent/prompts
        </div>
        <!-- 专家段标题：仅紧随技能/模板段（即第一个专家条目）之前显示一次 -->
        <div
          v-if="s.kind === 'expert' && (i === 0 || ['skill', 'prompt'].includes(slashMatches[i - 1]?.kind ?? ''))"
          class="mention-group"
        >
          专家 · {{ centered ? "雇佣到新会话" : "以该专家身份" }}
        </div>
        <button
          class="mention-item"
          :class="{ active: i === slash.active }"
          :title="s.description"
          @mousedown.prevent
          @mouseenter="slash.active = i"
          @click="applySlash(s)"
        >
          <span class="m-icon">
            <img
              v-if="s.kind === 'expert' && s.avatar"
              :src="s.avatar"
              class="m-avatar"
              alt=""
            />
            <Icon v-else :name="s.kind === 'skill' ? I.magic : s.kind === 'prompt' ? 'terminal-box-line' : 'user-star-line'" :size="16" />
          </span>
          <span class="m-name">{{ s.kind === "skill" ? `/skill:${s.name}` : s.kind === "prompt" ? `/${s.name}` : `/expert:${s.name}` }}</span>
          <span class="m-dir">{{ s.description }}</span>
        </button>
      </template>
    </div>

    <!-- 已雇佣专家 chip（新任务模式：绑定到将创建的会话） -->
    <div v-if="hiredExpert" class="files-row">
      <div class="file-chip expert-hired" :title="`已雇佣专家「${hiredExpert.name}」，将绑定到新会话`">
        <Icon name="user-star-line" :size="15" />
        <span class="file-name">专家 · {{ hiredExpert.name }}</span>
        <button class="file-x" title="取消雇佣" @click="emit('unhire')">
          <Icon :name="I.x" :size="10" />
        </button>
      </div>
    </div>

    <!-- 附件（文档类）chip 行 -->
    <div v-if="pendingFiles.length || docWarn" class="files-row">
      <div v-for="(p, i) in pendingFiles" :key="i" class="file-chip" :title="p.name">
        <FileIcon :path="p.name" :size="15" />
        <span class="file-name">{{ p.name }}</span>
        <span class="file-size">{{ fmtBytes(p.size) }}</span>
        <button class="file-x" title="移除" @click="removeFile(i)">
          <Icon :name="I.x" :size="10" />
        </button>
      </div>
      <div v-if="docWarn" class="img-warn">
        <Icon name="error-warning-line" :size="14" />
        <span>{{ docWarn }}</span>
      </div>
    </div>
    <!-- 图片附件缩略图 -->
    <div v-if="pendingImages.length" class="imgs-row">
      <div v-for="(p, i) in pendingImages" :key="i" class="img-chip" :title="p.name">
        <img :src="'data:' + p.mime_type + ';base64,' + p.data" alt="" />
        <button class="img-x" title="移除" @click="removeImage(i)">
          <Icon :name="I.x" :size="10" />
        </button>
      </div>
      <div v-if="modelImagesOk === false" class="img-warn">
        <Icon name="error-warning-line" :size="14" />
        <span>当前模型可能不支持图片输入，图片可能不会被读取</span>
      </div>
    </div>
    <div class="bar">
      <!-- + 菜单：图片 / 附件 / 文件提及 / 技能 -->
      <div class="dd plus-dd">
        <button class="plus-btn" title="添加图片 / 附件 / 文件 / 技能" @click="togglePlus">
          <Icon :name="I.plus" :size="16" />
        </button>
        <div v-if="plusOpen" class="dd-menu up plus-menu" @click.stop>
          <button class="plus-item" @click="pickImages">
            <Icon :name="I.img" :size="15" />
            <span>添加图片</span>
            <span class="plus-hint">≤ 6 张</span>
          </button>
          <button v-if="!draft" class="plus-item" @click="pickFiles">
            <Icon :name="I.fileAdd" :size="15" />
            <span>上传附件</span>
            <span class="plus-hint">txt / pdf / office</span>
          </button>
          <button class="plus-item" @click="addFileMention">
            <Icon :name="I.folderAdd" :size="15" />
            <span>添加文件</span>
            <span class="plus-hint">@</span>
          </button>
          <div class="plus-sep"></div>
          <div class="plus-title">
            <span>技能</span>
            <span class="plus-hint">$</span>
          </div>
          <div class="plus-skills">
            <button
              v-for="s in skills"
              :key="s.name"
              class="plus-item"
              :title="s.description"
              @click="pickSkill(s.name)"
            >
              <Icon :name="I.magic" :size="15" />
              <span class="plus-name">{{ s.name }}</span>
              <span class="plus-desc">{{ s.description }}</span>
            </button>
            <div v-if="!skills.length" class="plus-empty">{{ skillsLoading ? "加载中…" : "暂无技能" }}</div>
          </div>
        </div>
        <input ref="fileInput" type="file" accept="image/*" multiple hidden @change="onFilesPicked" />
        <input
          ref="docInput"
          type="file"
          accept=".pdf,.docx,.xlsx,.pptx,.txt,.md,.markdown,.csv,.tsv,.log,.json,.yaml,.yml,.toml,.xml,.html,.htm,text/*,application/json,application/pdf,application/vnd.openxmlformats-officedocument.wordprocessingml.document,application/vnd.openxmlformats-officedocument.spreadsheetml.sheet,application/vnd.openxmlformats-officedocument.presentationml.presentation"
          multiple
          hidden
          @change="onDocsPicked"
        />
      </div>

      <!-- 权限模式 -->
      <div class="dd">
        <button
          class="dd-btn dd-btn-boxed"
          :class="{ active: permissionMode != null && permissionMode !== 'full' }"
          @click="permOpen = !permOpen; thinkOpen = false; modelOpen = false; mention = null"
        >
          <Icon class="mode-ico" :name="permIcon" :size="14" />
          <span>{{ permLabel }}</span>
          <span class="c-chev" :class="{ open: permOpen }"><Icon :name="I.chevD" :size="11" /></span>
        </button>
        <div v-if="permOpen" class="dd-menu up">
          <button
            v-for="m in PERMISSION_ITEMS"
            :key="m.value"
            class="perm-item"
            :class="{ on: m.value === permissionMode }"
            @click="selectPerm(m.value)"
          >
            <span class="perm-icon"><Icon :name="m.icon" :size="15" /></span>
            <span class="perm-text">
              <b>{{ m.label }}</b>
              <i>{{ m.desc }}</i>
            </span>
            <Icon v-if="m.value === permissionMode" class="c-check" :name="I.check" :size="14" />
          </button>
        </div>
      </div>

      <span class="flex-sp"></span>

      <!-- 模型 -->
      <div class="dd">
        <button class="dd-btn" @click="toggleModelMenu">
          <span class="ring"></span>
          <span>{{ model ?? "默认模型" }}</span>
          <span class="c-chev" :class="{ open: modelOpen }"><Icon :name="I.chevD" :size="11" /></span>
        </button>
        <div v-if="modelOpen" class="dd-menu up right">
          <div class="c-search">
            <Icon :name="I.search" :size="14" />
            <input v-model="modelQuery" placeholder="搜索模型" @keydown.enter="selectFirstModel" />
          </div>
          <div class="m-list">
            <template v-for="g in modelGroups" :key="g.provider">
              <div class="m-group">{{ g.provider }}</div>
              <button
                v-for="it in g.items"
                :key="it.full"
                class="c-item"
                :class="{ on: it.full === model }"
                :title="it.full"
                @click="selectModel(it.full)"
              >
                <span class="c-item-name">{{ it.id }}</span>
                <Icon v-if="it.full === model" class="c-check" :name="I.check" :size="14" />
              </button>
            </template>
            <div v-if="!modelGroups.length" class="c-empty">
              {{ models?.length ? "没有匹配的模型" : "暂无可用模型，请在供应商页添加" }}
            </div>
          </div>
          <div class="c-sep"></div>
          <button class="c-item c-action" @click="modelOpen = false; emit('openProviders')">
            <Icon :name="I.gear" :size="14" />
            <span class="c-item-name">管理模型供应商…</span>
          </button>
        </div>
      </div>

      <!-- 思考级别 -->
      <div class="dd">
        <button class="dd-btn" @click="thinkOpen = !thinkOpen; permOpen = false; modelOpen = false; mention = null">
          <Icon :name="I.gauge" :size="15" />
          <span>{{ thinkLabel }}</span>
          <span class="c-chev" :class="{ open: thinkOpen }"><Icon :name="I.chevD" :size="11" /></span>
        </button>
        <div v-if="thinkOpen" class="dd-menu up right">
          <button
            v-for="t in thinkItems"
            :key="t.value"
            class="c-item"
            :class="{ on: t.value === thinkingLevel }"
            @click="selectThink(t.value)"
          >
            <span class="c-item-name">{{ t.label }}</span>
            <Icon v-if="t.value === thinkingLevel" class="c-check" :name="I.check" :size="14" />
          </button>
          <div v-if="!thinkItems.length" class="c-empty">当前模型不支持思考</div>
        </div>
      </div>

      <button v-if="busy" class="abort" title="停止" @click="emit('abort')">
        <Icon :name="I.stop" :size="14" />
      </button>
      <!-- 草稿模式：保存动作由父级表单底部的按钮承担，这里不再显示发送键 -->
      <button v-else-if="!draft" class="send" :disabled="disabled || !text.trim()" @click="submit">
        <Icon :name="I.up" :size="15" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.composer {
  /* @ 提及与 / 技能弹层的定位锚点 */
  position: relative;
  border: 1px solid var(--pd-border);
  border-radius: var(--pd-radius-lg);
  background: var(--pd-bg-card);
  padding: 14px 16px 12px;
  transition: border-color 0.15s;
}
.composer:focus-within { border-color: #4a4a4a; }
.composer.centered { box-shadow: var(--pd-shadow); }
.flex-sp { flex: 1; }

/* 手机：工具行只留图标（权限/模型/思考的下拉仍可点开），防止 min-content 撑破窄屏 */
@media (max-width: 480px) {
  .composer { padding: 12px 12px 10px; }
  .bar { gap: 8px; }
  .dd-btn > span:not(.ring):not(.c-chev) { display: none; }
}

/* ---- @ 文件提及弹层 ---- */
.mention-menu {
  position: absolute;
  left: 10px;
  right: 10px;
  bottom: 56px;
  max-height: 272px;
  overflow-y: auto;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 6px;
  box-shadow: var(--pd-shadow);
  z-index: 70;
}
.mention-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  text-align: left;
  padding: 7px 10px;
  border-radius: 8px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.mention-group {
  padding: 6px 10px 2px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  user-select: none;
}
.expert-hired {
  color: var(--pd-accent);
  border-color: var(--pd-accent);
}
.mention-item.active,
.mention-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.m-icon { flex: none; display: inline-grid; place-items: center; }
.m-avatar { width: 18px; height: 18px; border-radius: 50%; object-fit: cover; display: block; }
.m-name { flex: none; font-weight: 500; color: var(--pd-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.m-dir {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
}

.c-chips {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 10px;
}
.c-folder,
.c-branch {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  position: relative;
  width: fit-content;
  user-select: none;
}
.c-folder:hover,
.c-branch:hover { color: var(--pd-text); }
.c-folder svg,
.c-branch svg { color: var(--pd-text-3); }
.c-folder > .c-name,
.c-branch > .c-name { max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.c-clear {
  width: 16px;
  height: 16px;
  display: grid;
  place-items: center;
  border-radius: 5px;
  color: var(--pd-text-4);
}
.c-clear:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.c-chev { display: grid; place-items: center; transition: transform 0.12s; }
.c-chev.open { transform: rotate(180deg); }

.c-menu {
  position: absolute;
  left: 0;
  bottom: calc(100% + 10px);
  width: 360px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 6px;
  box-shadow: var(--pd-shadow);
  z-index: 60;
}
.c-search {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  color: var(--pd-text-4);
  border-bottom: 1px solid var(--pd-border-soft);
  margin-bottom: 4px;
}
.c-search input {
  flex: 1;
  background: none;
  border: none;
  outline: none;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
}
.c-search input::placeholder { color: var(--pd-text-4); }
.c-list { max-height: 220px; overflow-y: auto; }
.c-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  text-align: left;
  padding: 8px 10px;
  border-radius: 8px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.c-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.c-item.on { color: var(--pd-text); }
.c-item-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.c-check { color: var(--pd-accent); flex: none; }
.c-empty { padding: 10px; font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-4); }
.c-sep { height: 1px; background: var(--pd-border-soft); margin: 4px; }
.c-action svg { color: var(--pd-text-3); }
.c-item:disabled { opacity: 0.5; cursor: default; }
.c-item:disabled:hover { background: none; color: var(--pd-text-2); }
/* 分支菜单的分组标题与错误行 */
.c-group {
  padding: 6px 10px 3px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  user-select: none;
}
.c-branch-err {
  padding: 6px 10px 4px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-red-text);
  word-break: break-all;
  user-select: text;
}
.c-addrow {
  display: flex;
  gap: 6px;
  padding: 4px;
}
.c-addrow input {
  flex: 1;
  min-width: 0;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 6px 9px;
}
.c-addrow input:focus { outline: none; border-color: var(--pd-accent); }
.c-addbtn {
  background: var(--pd-accent);
  color: #1a1a1a;
  border: none;
  border-radius: 8px;
  padding: 6px 12px;
  font-size: calc(12px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
}

textarea {
  width: 100%;
  min-height: 56px;
  resize: none;
  background: transparent;
  color: var(--pd-text);
  border: none;
  padding: 2px 2px 6px;
  font-size: calc(14px * var(--pd-font-scale));
  font-family: inherit;
  line-height: 1.7;
}
textarea:focus { outline: none; }
textarea::placeholder { color: var(--pd-text-4); }
textarea:disabled { opacity: 0.45; }

.bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 6px;
}
.plus-btn {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  color: var(--pd-text-2);
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
}
.plus-btn:hover:not(:disabled) { background: var(--pd-bg-hover); color: var(--pd-text); }
.plus-btn:disabled { opacity: 0.45; cursor: default; }

/* ---- + 菜单与附件缩略图 ---- */
.plus-menu { min-width: 252px; }
.plus-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  text-align: left;
  padding: 7px 10px;
  border-radius: 8px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.plus-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.plus-item svg { color: var(--pd-text-3); flex: none; }
.plus-hint { margin-left: auto; font-size: calc(11px * var(--pd-font-scale)); color: var(--pd-text-4); }
.plus-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 7px 10px 3px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  user-select: none;
}
.plus-skills { max-height: 220px; overflow-y: auto; }
.plus-name { font-weight: 500; color: var(--pd-text); flex: none; }
.plus-desc {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
}
.plus-empty { padding: 10px; font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-4); }
.plus-sep { height: 1px; background: var(--pd-border-soft); margin: 4px; }
.files-row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin-top: 8px; }
.file-chip {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 240px;
  padding: 6px 22px 6px 9px;
  border-radius: 8px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg-card);
  font-size: calc(12px * var(--pd-font-scale));
}
.file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--pd-text);
  font-weight: 500;
}
.file-size { flex: none; font-size: calc(11px * var(--pd-font-scale)); color: var(--pd-text-4); }
.file-x {
  position: absolute;
  top: 4px;
  right: 4px;
  width: 14px;
  height: 14px;
  border-radius: 4px;
  border: none;
  background: transparent;
  color: var(--pd-text-4);
  display: grid;
  place-items: center;
  cursor: pointer;
  padding: 0;
}
.file-x:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.imgs-row { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 8px; }
.img-chip {
  position: relative;
  width: 46px;
  height: 46px;
  border-radius: 8px;
  overflow: hidden;
  border: 1px solid var(--pd-border);
}
.img-chip img { width: 100%; height: 100%; object-fit: cover; display: block; }
.img-warn {
  display: flex;
  align-items: center;
  gap: 6px;
  align-self: center;
  padding: 5px 10px;
  border-radius: 8px;
  background: var(--pd-yellow-soft);
  color: var(--pd-yellow-text);
  font-size: calc(12px * var(--pd-font-scale));
}
.img-x {
  position: absolute;
  top: 1px;
  right: 1px;
  width: 15px;
  height: 15px;
  border-radius: 4px;
  border: none;
  background: rgba(0, 0, 0, 0.55);
  color: #fff;
  display: grid;
  place-items: center;
  cursor: pointer;
  padding: 0;
}

/* ---- 底栏下拉 ---- */
.dd { position: relative; }
.dd-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 220px;
  background: none;
  border: none;
  border-radius: 8px;
  padding: 6px 8px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
}
.dd-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.dd-btn-boxed {
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border-soft);
}
/* 选中的非默认权限模式：图标与文字用主题色，不加底色填充 */
.dd-btn-boxed.active {
  background: none;
  border-color: transparent;
  color: var(--pd-accent);
}
.dd-btn-boxed.active .mode-ico { color: var(--pd-accent); }
.dd-btn-boxed.active:hover { background: var(--pd-bg-hover); }
.dd-btn .c-chev { color: var(--pd-text-4); }
.dd-btn > span:not(.ring):not(.c-chev) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dd-menu {
  position: absolute;
  bottom: calc(100% + 10px);
  left: 0;
  min-width: 240px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 6px;
  box-shadow: var(--pd-shadow);
  z-index: 60;
}
.dd-menu.right { left: auto; right: 0; }
.m-list { max-height: 280px; overflow-y: auto; }
.m-group {
  padding: 7px 10px 3px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  user-select: none;
}
.m-group:first-child { padding-top: 5px; }
.perm-item {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 280px;
  text-align: left;
  padding: 9px 10px;
  border-radius: 8px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  cursor: pointer;
}
.perm-item:hover { background: var(--pd-bg-hover); }
.perm-item.on { color: var(--pd-text); background: var(--pd-bg-hover); }
.perm-icon { color: var(--pd-text-3); margin-top: 1px; flex: none; }
.perm-item.on .perm-icon { color: var(--pd-accent); }
.perm-text { flex: 1; min-width: 0; }
.perm-text b { display: block; font-size: calc(13.5px * var(--pd-font-scale)); font-weight: 600; color: var(--pd-text); }
.perm-text i {
  display: block;
  font-style: normal;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
  margin-top: 2px;
}

.ring {
  width: 12px;
  height: 12px;
  border: 2px solid var(--pd-text-4);
  border-radius: 50%;
}

button.send,
button.abort {
  width: 32px;
  height: 32px;
  border-radius: 10px;
  display: grid;
  place-items: center;
  border: none;
  cursor: pointer;
  padding: 0;
  flex: none;
}
.send {
  background: #4a4a4a;
  color: #ececec;
}
.send:hover:not(:disabled) { background: #565656; }
.send:disabled { opacity: 0.35; cursor: default; }
.abort {
  background: var(--pd-red-soft);
  color: var(--pd-red-text);
}
</style>
