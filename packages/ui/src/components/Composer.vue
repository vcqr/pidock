<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { basename } from "../utils/time.js";
import { FOLDER_PICKER } from "../databus.js";
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
    defaultCwd?: string;
    /** 权限模式（完全访问/自动编辑/变更前确认/计划模式） */
    permissionMode?: string;
    /** 思考级别（off/minimal/low/medium/high） */
    thinkingLevel?: string;
    /** 可切换的模型名（来自本工作区的会话） */
    models?: string[];
  }>(),
  { placeholder: "输入消息，Enter 发送，Shift+Enter 换行" },
);
const emit = defineEmits<{
  send: [text: string, cwd?: string | null];
  abort: [];
  setPermissionMode: [mode: string];
  setThinkingLevel: [level: string];
  setModel: [model: string];
  openProviders: [];
}>();

const I = {
  folder: "folder-line",
  folderAdd: "folder-add-line",
  chevD: "arrow-down-s-line",
  x: "close-line",
  check: "check-line",
  search: "search-line",
  cloud: "cloud-line",
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
};

const text = ref("");
const menuOpen = ref(false);
const menuQuery = ref("");
/** 当前选中的项目目录；null = 不在项目中工作 */
const selected = ref<string | null>(null);
const touched = ref(false);
const adding = ref(false);
const newPath = ref("");
const rootEl = ref<HTMLElement | null>(null);
/** 桌面端注入系统目录选择器；web / 未注入时回退到行内路径输入 */
const folderPicker = inject(FOLDER_PICKER, null);

/** 底栏下拉状态 */
const permOpen = ref(false);
const thinkOpen = ref(false);
const modelOpen = ref(false);

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
  { value: "high", label: "最高" },
];
const permLabel = computed(
  () => PERMISSION_ITEMS.find((i) => i.value === props.permissionMode)?.label ?? "完全访问",
);
const permIcon = computed(
  () => PERMISSION_ITEMS.find((i) => i.value === props.permissionMode)?.icon ?? I.shield,
);
const thinkLabel = computed(
  () => THINKING_ITEMS.find((i) => i.value === props.thinkingLevel)?.label ?? "中",
);

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
);
watch(
  () => props.defaultCwd,
  (v) => {
    if (v && !touched.value) selected.value = v;
  },
  { immediate: true },
);
watch(
  () => props.projects,
  (list) => {
    if (selected.value === null && !touched.value && list?.length) selected.value = list[0] ?? null;
  },
  { immediate: true },
);

function pick(p: string): void {
  selected.value = p;
  touched.value = true;
  menuOpen.value = false;
  menuQuery.value = "";
}
function selectNone(): void {
  selected.value = null;
  touched.value = true;
  menuOpen.value = false;
  menuQuery.value = "";
}
function clearSelected(): void {
  selected.value = null;
  touched.value = true;
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

function openRemote(): void {
  menuOpen.value = false;
  window.dispatchEvent(new CustomEvent("pidock:open-sync"));
}

function onDocClick(e: MouseEvent): void {
  if (rootEl.value && !rootEl.value.contains(e.target as Node)) {
    menuOpen.value = false;
    adding.value = false;
    menuQuery.value = "";
    permOpen.value = false;
    thinkOpen.value = false;
    modelOpen.value = false;
    modelQuery.value = "";
  }
}
onMounted(() => document.addEventListener("click", onDocClick));
onBeforeUnmount(() => document.removeEventListener("click", onDocClick));

function submit(): void {
  const value = text.value.trim();
  if (!value || props.busy || props.disabled) return;
  const cwd = props.centered ? selected.value : undefined;
  emit("send", value, cwd === null ? null : cwd || undefined);
  text.value = "";
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    submit();
  }
}
</script>

<template>
  <div ref="rootEl" class="composer" :class="{ centered }" :title="disabled ? disabledHint : ''">
    <div v-if="centered" class="c-folder" @click="menuOpen = !menuOpen">
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

      <!-- 项目菜单：搜索 / 项目列表 / 打开文件夹 / 远程连接 / 不在项目中工作 -->
      <div v-if="menuOpen" class="c-menu" @click.stop>
        <div class="c-search">
          <Icon :name="I.search" :size="13" />
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
        <button class="c-item c-action" @click="openRemote">
          <Icon :name="I.cloud" :size="14" />
          <span>远程连接</span>
        </button>
        <button class="c-item c-action" :class="{ on: !selected }" @click="selectNone">
          <Icon :name="I.chat" :size="14" />
          <span>不在项目中工作</span>
        </button>
      </div>
    </div>

    <textarea
      v-model="text"
      rows="2"
      :disabled="disabled"
      :placeholder="disabled ? disabledHint : placeholder"
      @keydown="onKeydown"
    />
    <div class="bar">
      <button class="plus-btn" disabled title="附件 / 图片 · 开发中">
        <Icon :name="I.plus" :size="16" />
      </button>

      <!-- 权限模式 -->
      <div class="dd">
        <button
          class="dd-btn dd-btn-boxed"
          :class="{ active: permissionMode != null && permissionMode !== 'full' }"
          @click="permOpen = !permOpen; thinkOpen = false; modelOpen = false"
        >
          <Icon class="mode-ico" :name="permIcon" :size="13" />
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
            <Icon :name="I.search" :size="13" />
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
        <button class="dd-btn" @click="thinkOpen = !thinkOpen; permOpen = false; modelOpen = false">
          <Icon :name="I.gauge" :size="13" />
          <span>{{ thinkLabel }}</span>
          <span class="c-chev" :class="{ open: thinkOpen }"><Icon :name="I.chevD" :size="11" /></span>
        </button>
        <div v-if="thinkOpen" class="dd-menu up right">
          <button
            v-for="t in THINKING_ITEMS"
            :key="t.value"
            class="c-item"
            :class="{ on: t.value === thinkingLevel }"
            @click="selectThink(t.value)"
          >
            <span class="c-item-name">{{ t.label }}</span>
            <Icon v-if="t.value === thinkingLevel" class="c-check" :name="I.check" :size="14" />
          </button>
        </div>
      </div>

      <button v-if="busy" class="abort" title="停止" @click="emit('abort')">
        <Icon :name="I.stop" :size="13" />
      </button>
      <button v-else class="send" :disabled="disabled || !text.trim()" @click="submit">
        <Icon :name="I.up" :size="15" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.composer {
  border: 1px solid var(--pd-border);
  border-radius: var(--pd-radius-lg);
  background: var(--pd-bg-card);
  padding: 14px 16px 12px;
  transition: border-color 0.15s;
}
.composer:focus-within { border-color: #4a4a4a; }
.composer.centered { box-shadow: var(--pd-shadow); }
.flex-sp { flex: 1; }

.c-folder {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--pd-text-2);
  font-size: 13px;
  margin-bottom: 10px;
  cursor: pointer;
  position: relative;
  width: fit-content;
  user-select: none;
}
.c-folder:hover { color: var(--pd-text); }
.c-folder svg { color: var(--pd-text-3); }
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
  font-size: 13px;
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
  font-size: 13px;
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
.c-empty { padding: 10px; font-size: 12px; color: var(--pd-text-4); }
.c-sep { height: 1px; background: var(--pd-border-soft); margin: 4px; }
.c-action svg { color: var(--pd-text-3); }
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
  font-size: 12px;
  padding: 6px 9px;
}
.c-addrow input:focus { outline: none; border-color: var(--pd-accent); }
.c-addbtn {
  background: var(--pd-accent);
  color: #1a1a1a;
  border: none;
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 12px;
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
  font-size: 14px;
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
  font-size: 13px;
  cursor: pointer;
  white-space: nowrap;
}
.dd-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.dd-btn-boxed {
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border-soft);
}
/* 选中的非默认权限模式：点亮为主题色 */
.dd-btn-boxed.active {
  background: var(--pd-accent-soft);
  border-color: var(--pd-accent);
  color: var(--pd-accent-text);
}
.dd-btn-boxed.active .mode-ico { color: var(--pd-accent); }
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
  font-size: 11px;
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
.perm-text b { display: block; font-size: 13.5px; font-weight: 600; color: var(--pd-text); }
.perm-text i {
  display: block;
  font-style: normal;
  font-size: 12px;
  color: var(--pd-text-3);
  margin-top: 2px;
}

.ring {
  width: 9px;
  height: 9px;
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
