<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { REVEAL_PATH } from "../databus.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 项目文件浏览面板（第三栏停靠，宽度由外层 NSplit 承载可拖拽）：
 * workspace.files 建树 + 搜索过滤；点击文件向主区预览分栏发 open-file。
 * 已打开/激活状态由 props 注入（openPaths / activePath，随主区预览标签联动）。
 */

interface FileEntry {
  path: string;
  name: string;
  dir: boolean;
}

interface TreeNode {
  name: string;
  /** 相对 cwd 的 posix 路径（与 workspace.files 一致） */
  path: string;
  dir: boolean;
  children: TreeNode[];
}

const props = defineProps<{
  /** 项目根目录（workspace.files 的 cwd） */
  cwd: string;
  /** 显示名（缺省取 cwd 末段） */
  name?: string;
  /** 主区预览已打开的文件路径（树里显示小圆点标记） */
  openPaths?: string[];
  /** 主区预览当前激活的文件路径（树里高亮行） */
  activePath?: string | null;
  loadFiles: (cwd: string) => Promise<FileEntry[]>;
}>();
const emit = defineEmits<{
  /** 点击文件：主区预览打开/激活该标签 */
  "open-file": [path: string];
  /** 头部关闭按钮 */
  close: [];
}>();

const revealPath = inject(REVEAL_PATH, null);

const title = computed(() => props.name || props.cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || props.cwd);
/** cwd 分隔符决定绝对路径拼接符（桌面 Windows 为 \，web 可能为 /） */
const joinAbs = (rel: string): string =>
  props.cwd.replace(/[\\/]+$/, "") + (props.cwd.includes("\\") ? "\\" : "/") + rel;

// ---- 文件列表与树 ----
const entries = ref<FileEntry[]>([]);
const loading = ref(false);
const loadError = ref<string | null>(null);
const query = ref("");
/** 展开的目录（key = 相对路径）；目录默认全部折叠，点展开 */
const expandedDirs = ref<Set<string>>(new Set());

async function load(): Promise<void> {
  loading.value = true;
  loadError.value = null;
  try {
    entries.value = await props.loadFiles(props.cwd);
    if (!entries.value.length) loadError.value = "目录为空";
  } catch (err) {
    entries.value = [];
    loadError.value = err instanceof Error ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

function buildTree(list: FileEntry[]): TreeNode[] {
  const root: TreeNode = { name: "", path: "", dir: true, children: [] };
  for (const e of list) {
    const parts = e.path.split("/");
    let cur = root;
    parts.forEach((part, i) => {
      const isDir = i < parts.length - 1 || e.dir;
      let next = cur.children.find((c) => c.name === part);
      if (!next) {
        next = { name: part, path: parts.slice(0, i + 1).join("/"), dir: isDir, children: [] };
        cur.children.push(next);
      }
      cur = next;
    });
  }
  const sortRec = (n: TreeNode): void => {
    n.children.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
    for (const c of n.children) sortRec(c);
  };
  sortRec(root);
  return root.children;
}

const tree = computed(() => buildTree(entries.value));

/** 树模式下的可见行（展开的目录递归下钻） */
const visibleRows = computed<Array<{ node: TreeNode; depth: number }>>(() => {
  const rows: Array<{ node: TreeNode; depth: number }> = [];
  const walk = (nodes: TreeNode[], depth: number): void => {
    for (const n of nodes) {
      rows.push({ node: n, depth });
      if (n.dir && expandedDirs.value.has(n.path)) walk(n.children, depth + 1);
    }
  };
  walk(tree.value, 0);
  return rows;
});

/** 搜索模式：路径命中的平铺列表 */
const searchRows = computed<FileEntry[]>(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return [];
  return entries.value.filter((e) => e.path.toLowerCase().includes(q)).slice(0, 200);
});

function isOpen(path: string): boolean {
  return !!props.openPaths?.includes(path);
}

function toggleDir(n: TreeNode): void {
  const next = new Set(expandedDirs.value);
  next.has(n.path) ? next.delete(n.path) : next.add(n.path);
  expandedDirs.value = next;
}

/** 是否全部目录都已展开（头部按钮切换依据） */
const allExpanded = computed(() => {
  const dirs = entries.value.filter((e) => e.dir);
  return dirs.length > 0 && dirs.every((d) => expandedDirs.value.has(d.path));
});
/** 展开/收起全部目录 */
function toggleAllDirs(): void {
  expandedDirs.value = allExpanded.value
    ? new Set<string>()
    : new Set(entries.value.filter((e) => e.dir).map((e) => e.path));
}

function onRowClick(n: TreeNode): void {
  if (n.dir) toggleDir(n);
  else emit("open-file", n.path);
}

// ---- 复制路径（WebView 剪贴板失败退回 execCommand） ----
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}

watch(
  () => props.cwd,
  () => {
    query.value = "";
    expandedDirs.value = new Set();
    void load();
  },
);
onMounted(load);
</script>

<template>
  <aside class="files-panel">
    <header class="fp-head">
      <Icon name="folder-open-line" :size="17" />
      <span class="fp-title" :title="cwd">{{ title }}</span>
      <span class="flex-sp"></span>
      <button
        class="icon-btn"
        :title="allExpanded ? '收起全部' : '展开全部'"
        :disabled="loading || !!loadError"
        @click="toggleAllDirs"
      ><Icon :name="allExpanded ? 'collapse-vertical-line' : 'expand-vertical-line'" :size="15" /></button>
      <button class="icon-btn" title="刷新" @click="load"><Icon name="refresh-line" :size="15" /></button>
      <button
        v-if="revealPath"
        class="icon-btn"
        title="在资源管理器中打开"
        @click="revealPath(cwd)"
      ><Icon name="folder-line" :size="15" /></button>
      <button class="icon-btn" title="关闭" @click="emit('close')"><Icon name="close-line" :size="16" /></button>
    </header>

    <div class="fp-search">
      <input v-model="query" placeholder="搜索文件…" @keydown.esc="query = ''" />
    </div>

    <div class="fp-tree">
      <div v-if="loading" class="fp-hint"><Icon name="loader-2-line" :size="16" />加载中…</div>
      <div v-else-if="loadError" class="fp-hint"><Icon name="error-warning-line" :size="16" />{{ loadError }}</div>
      <template v-else-if="query.trim()">
        <div
          v-for="e in searchRows"
          :key="e.path"
          class="row"
          :class="{ current: !e.dir && e.path === activePath }"
          :title="e.path"
          @click="!e.dir && onRowClick({ name: e.name, path: e.path, dir: e.dir, children: [] })"
        >
          <span class="tile"><FileIcon :path="e.path" :dir="e.dir" :size="16" /></span>
          <span class="row-name">{{ e.name }}</span>
          <span class="row-sp"></span>
          <span v-if="isOpen(e.path)" class="open-dot" title="已在预览中打开"></span>
          <button class="row-copy" title="复制路径" @click.stop="copyText(joinAbs(e.path))">
            <Icon name="file-copy-line" :size="14" />
          </button>
        </div>
        <div v-if="!searchRows.length" class="fp-hint">没有匹配的文件</div>
      </template>
      <template v-else>
        <div
          v-for="{ node, depth } in visibleRows"
          :key="node.path"
          class="row"
          :class="{ current: !node.dir && node.path === activePath }"
          :style="{ paddingLeft: 8 + depth * 13 + 'px' }"
          :title="node.path"
          @click="onRowClick(node)"
        >
          <span class="chev" :class="{ fold: !expandedDirs.has(node.path), blank: !node.dir }">
            <Icon name="arrow-down-s-line" :size="13" />
          </span>
          <span class="tile"><FileIcon :path="node.path" :dir="node.dir" :size="16" /></span>
          <span class="row-name">{{ node.name }}</span>
          <span class="row-sp"></span>
          <span v-if="isOpen(node.path)" class="open-dot" title="已在预览中打开"></span>
          <button class="row-copy" title="复制路径" @click.stop="copyText(joinAbs(node.path))">
            <Icon name="file-copy-line" :size="14" />
          </button>
        </div>
        <div v-if="!visibleRows.length" class="fp-hint">目录为空</div>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.files-panel {
  width: 100%;
  height: 100%;
  background: var(--pd-bg-panel);
  border-left: 1px solid var(--pd-border-soft);
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.flex-sp { flex: 1; }

.fp-head {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 10px 10px 8px 12px;
  color: var(--pd-text);
  font-size: calc(13.5px * var(--pd-font-scale));
  font-weight: 600;
}
.fp-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.icon-btn {
  width: 26px;
  height: 26px;
  border-radius: 7px;
  display: grid;
  place-items: center;
  color: var(--pd-text-3);
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  flex: none;
}
.icon-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.icon-btn:disabled { opacity: 0.4; pointer-events: none; }

.fp-search { padding: 0 10px 8px 12px; }
.fp-search input {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  color: var(--pd-text);
  border-radius: 8px;
  padding: 6px 10px;
  font-size: calc(12.5px * var(--pd-font-scale));
}
.fp-search input:focus { outline: none; border-color: var(--pd-accent); }

.fp-tree {
  flex: 1;
  overflow: auto;
  padding: 0 6px 8px;
  min-height: 60px;
}
.fp-tree::-webkit-scrollbar { width: 8px; }
.fp-tree::-webkit-scrollbar-thumb { background: var(--pd-scrollbar); border-radius: 4px; }
.fp-hint {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 6px;
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
}
.row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 27px;
  padding-right: 6px;
  border-radius: 6px;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}
.row:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
/* 当前在主区预览中激活的文件行 */
.row.current { background: var(--pd-bg-active); color: var(--pd-text); }
.row .chev {
  display: grid;
  place-items: center;
  width: 14px;
  flex: none;
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.row .chev.fold { transform: rotate(-90deg); }
.row .chev.blank { visibility: hidden; }
.tile {
  display: grid;
  place-items: center;
  flex: none;
}
.row-name {
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}
/* 已在预览中打开的文件：名称右侧小圆点 */
.row-sp { flex: 1; min-width: 0; }
.open-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex: none;
  background: var(--pd-accent);
}
.row-copy {
  width: 22px;
  height: 22px;
  border-radius: 5px;
  display: grid;
  place-items: center;
  color: var(--pd-text-3);
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  flex: none;
  opacity: 0;
}
.row:hover .row-copy, .row.current .row-copy { opacity: 1; }
.row-copy:hover { background: var(--pd-bg-active); color: var(--pd-text); }
</style>
