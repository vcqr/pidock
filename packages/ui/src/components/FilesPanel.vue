<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { REVEAL_PATH } from "../databus.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 项目文件浏览面板（第三栏最右停靠）：workspace.files 建树 + 搜索过滤 +
 * 点击文件预览文本内容。数据经回调注入（store.listWorkspaceFiles / readWorkspaceFile），
 * 组件自身不依赖 store 实例。
 */

interface FileEntry {
  path: string;
  name: string;
  dir: boolean;
}
interface FileReadResult {
  text: string;
  truncated: boolean;
  binary: boolean;
  size: number;
}

interface TreeNode {
  name: string;
  /** 相对 cwd 的 posix 路径（与 workspace.files 一致） */
  path: string;
  dir: boolean;
  children: TreeNode[];
}

const props = defineProps<{
  /** 项目根目录（workspace.files / read_file 的 cwd） */
  cwd: string;
  /** 显示名（缺省取 cwd 末段） */
  name?: string;
  loadFiles: (cwd: string) => Promise<FileEntry[]>;
  readFile: (cwd: string, path: string) => Promise<FileReadResult>;
}>();
const emit = defineEmits<{ close: [] }>();

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
/** 展开的目录（key = 相对路径）；加载后默认展开第一层 */
const expandedDirs = ref<Set<string>>(new Set());

async function load(): Promise<void> {
  loading.value = true;
  loadError.value = null;
  preview.value = null;
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

function toggleDir(n: TreeNode): void {
  const next = new Set(expandedDirs.value);
  next.has(n.path) ? next.delete(n.path) : next.add(n.path);
  expandedDirs.value = next;
}

// ---- 预览 ----
const preview = ref<(FileReadResult & { path: string }) | null>(null);
const previewLoading = ref(false);
const previewError = ref<string | null>(null);

async function openPreview(n: TreeNode): Promise<void> {
  previewLoading.value = true;
  previewError.value = null;
  try {
    const r = await props.readFile(props.cwd, n.path);
    preview.value = { ...r, path: n.path };
  } catch (err) {
    preview.value = null;
    previewError.value = err instanceof Error ? err.message : String(err);
  } finally {
    previewLoading.value = false;
  }
}
function closePreview(): void {
  preview.value = null;
  previewError.value = null;
}

function onRowClick(n: TreeNode): void {
  if (n.dir) toggleDir(n);
  else void openPreview(n);
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
      <Icon name="folder-open-line" :size="15" />
      <span class="fp-title" :title="cwd">{{ title }}</span>
      <span class="flex-sp"></span>
      <button class="icon-btn" title="刷新" @click="load"><Icon name="refresh-line" :size="14" /></button>
      <button
        v-if="revealPath"
        class="icon-btn"
        title="在资源管理器中打开"
        @click="revealPath(cwd)"
      ><Icon name="folder-line" :size="14" /></button>
      <button class="icon-btn" title="关闭" @click="emit('close')"><Icon name="close-line" :size="15" /></button>
    </header>

    <div class="fp-search">
      <input v-model="query" placeholder="搜索文件…" @keydown.esc="query = ''" />
    </div>

    <div class="fp-tree">
      <div v-if="loading" class="fp-hint"><Icon name="loader-2-line" :size="15" />加载中…</div>
      <div v-else-if="loadError" class="fp-hint"><Icon name="error-warning-line" :size="15" />{{ loadError }}</div>
      <template v-else-if="query.trim()">
        <div
          v-for="e in searchRows"
          :key="e.path"
          class="row"
          :title="e.path"
          @click="!e.dir && onRowClick({ name: e.name, path: e.path, dir: e.dir, children: [] })"
        >
          <span class="tile"><FileIcon :path="e.path" :dir="e.dir" :size="14" /></span>
          <span class="row-name">{{ e.name }}</span>
          <button class="row-copy" :title="'复制路径'" @click.stop="copyText(joinAbs(e.path))">
            <Icon name="file-copy-line" :size="13" />
          </button>
        </div>
        <div v-if="!searchRows.length" class="fp-hint">没有匹配的文件</div>
      </template>
      <template v-else>
        <div
          v-for="{ node, depth } in visibleRows"
          :key="node.path"
          class="row"
          :style="{ paddingLeft: 8 + depth * 13 + 'px' }"
          :title="node.path"
          @click="onRowClick(node)"
        >
          <span class="chev" :class="{ fold: !expandedDirs.has(node.path), blank: !node.dir }">
            <Icon name="arrow-down-s-line" :size="12" />
          </span>
          <span class="tile"><FileIcon :path="node.path" :dir="node.dir" :size="14" /></span>
          <span class="row-name">{{ node.name }}</span>
          <button class="row-copy" title="复制路径" @click.stop="copyText(joinAbs(node.path))">
            <Icon name="file-copy-line" :size="13" />
          </button>
        </div>
        <div v-if="!visibleRows.length" class="fp-hint">目录为空</div>
      </template>
    </div>

    <!-- 文件预览（下半区） -->
    <div v-if="preview || previewLoading || previewError" class="fp-preview">
      <div class="pv-head">
        <FileIcon v-if="preview" :path="preview.path" :size="14" />
        <span class="pv-name" :title="preview ? joinAbs(preview.path) : ''">{{ preview?.path ?? "预览" }}</span>
        <span class="flex-sp"></span>
        <button
          v-if="preview"
          class="icon-btn"
          title="复制路径"
          @click="copyText(joinAbs(preview.path))"
        ><Icon name="file-copy-line" :size="13" /></button>
        <button class="icon-btn" title="关闭预览" @click="closePreview"><Icon name="close-line" :size="14" /></button>
      </div>
      <div v-if="previewLoading" class="pv-note"><Icon name="loader-2-line" :size="14" />加载中…</div>
      <div v-else-if="previewError" class="pv-note err"><Icon name="error-warning-line" :size="14" />{{ previewError }}</div>
      <template v-else-if="preview">
        <div v-if="preview.binary" class="pv-note">二进制文件，无法预览文本内容</div>
        <template v-else>
          <pre class="pv-body">{{ preview.text }}</pre>
          <div v-if="preview.truncated" class="pv-note">文件过大，仅显示前 256 KB</div>
        </template>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.files-panel {
  width: 300px;
  flex: none;
  background: var(--pd-bg-panel);
  border-left: 1px solid var(--pd-border-soft);
  display: flex;
  flex-direction: column;
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
  font-size: 13px;
  font-weight: 600;
}
.fp-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.icon-btn {
  width: 24px;
  height: 24px;
  border-radius: 6px;
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

.fp-search { padding: 0 10px 8px 12px; }
.fp-search input {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  color: var(--pd-text);
  border-radius: 8px;
  padding: 5px 10px;
  font-size: 12px;
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
  font-size: 12px;
}
.row {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 25px;
  padding-right: 6px;
  border-radius: 6px;
  color: var(--pd-text-2);
  font-size: 12.5px;
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}
.row:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
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
.row-copy {
  margin-left: auto;
  width: 20px;
  height: 20px;
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
.row:hover .row-copy { opacity: 1; }
.row-copy:hover { background: var(--pd-bg-active); color: var(--pd-text); }

.fp-preview {
  flex: 0 0 45%;
  border-top: 1px solid var(--pd-border-soft);
  display: flex;
  flex-direction: column;
  min-height: 100px;
}
.pv-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 8px 7px 12px;
  font-size: 12px;
  color: var(--pd-text-2);
  border-bottom: 1px solid var(--pd-border-soft);
}
.pv-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
.pv-body {
  flex: 1;
  overflow: auto;
  margin: 0;
  padding: 8px 12px;
  font-family: var(--pd-mono, ui-monospace, Consolas, monospace);
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--pd-text-2);
  white-space: pre;
  user-select: text;
}
.pv-body::-webkit-scrollbar { width: 8px; height: 8px; }
.pv-body::-webkit-scrollbar-thumb { background: var(--pd-scrollbar); border-radius: 4px; }
.pv-note {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  color: var(--pd-text-4);
  font-size: 11.5px;
}
.pv-note.err { color: var(--pd-red); }
</style>
