<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { GIT_API, type CommitFileStat, type GitCommit, REVEAL_PATH } from "../databus.js";
import { basename, relTime } from "../utils/time.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 项目文件浏览面板（第三栏停靠，宽度由外层 NSplit 承载可拖拽）：
 * workspace.files 建树 + 搜索过滤；点击文件向主区预览分栏发 open-file。
 * 目录是 git 仓库时头部出现「文件/提交」分段切换，提交视图为 HEAD 历史列表
 * （git_log 走 CoreCtx 命令面），点击一条提交复制短 hash；提交行可展开变更
 * 文件清单，点文件向主区发 open-commit-diff（审查面板开新旧快照 diff）。
 * 已打开/激活状态由 props 注入（openPaths / activePath，随主区预览标签联动）。
 */

/** 提交视图点开变更文件时上报给主区（由 ChatView 拉快照开审查标签） */
export interface CommitDiffPayload {
  hash: string;
  short: string;
  subject: string;
  file: CommitFileStat;
}

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
  /** 提交视图点开变更文件：主区审查面板开该文件的提交 diff 标签 */
  "open-commit-diff": [p: CommitDiffPayload];
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

// ---- git 提交视图（目录是仓库时可用；检测/历史都走 CoreCtx git 命令面） ----
const gitApi = inject(GIT_API, null);
const view = ref<"files" | "commits">("files");
const isRepo = ref(false);
const commits = ref<GitCommit[]>([]);
const commitsLoading = ref(false);
const commitsError = ref<string | null>(null);
/** 服务端单页 100 条；返回满页才显示「加载更多」 */
const PAGE_SIZE = 100;
const hasMore = ref(false);
const copiedShort = ref<string | null>(null);
let copiedTimer: ReturnType<typeof setTimeout> | undefined;

/** 仓库检测：非仓库 / git 不可用都按无 git 处理（不显示切换控件） */
async function detectGit(): Promise<void> {
  if (!gitApi) {
    isRepo.value = false;
    return;
  }
  try {
    isRepo.value = (await gitApi.info(props.cwd)).is_repo;
    if (!isRepo.value && view.value === "commits") view.value = "files";
  } catch {
    isRepo.value = false;
  }
}

async function loadCommits(append = false): Promise<void> {
  if (!gitApi) return;
  commitsLoading.value = true;
  commitsError.value = null;
  try {
    const res = await gitApi.log(props.cwd, PAGE_SIZE, append ? commits.value.length : 0);
    commits.value = append ? [...commits.value, ...res.commits] : res.commits;
    hasMore.value = res.commits.length >= PAGE_SIZE;
  } catch (err) {
    commitsError.value = err instanceof Error ? err.message : String(err);
    if (!append) commits.value = [];
  } finally {
    commitsLoading.value = false;
  }
}

/** 切视图；提交视图每次切入都重取（跟随分支切换后的 HEAD），附带头部刷新按钮 */
function switchView(v: "files" | "commits"): void {
  view.value = v;
  if (v === "commits") void loadCommits();
}

function refresh(): void {
  if (view.value === "commits") void loadCommits();
  else void load();
}

const filteredCommits = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return commits.value;
  return commits.value.filter(
    (c) =>
      c.subject.toLowerCase().includes(q) ||
      c.short.toLowerCase().includes(q) ||
      c.author.toLowerCase().includes(q),
  );
});

/** 修饰引用展示：去掉 HEAD -> / tag: 前缀，HEAD 所指（当前分支）单独高亮 */
function refName(r: string): string {
  return r.startsWith("HEAD -> ") ? r.slice(8) : r.startsWith("tag: ") ? r.slice(5) : r;
}
function isHeadRef(r: string): boolean {
  return r.startsWith("HEAD -> ");
}

function rowTitle(c: GitCommit): string {
  const full = c.time ? new Date(c.time * 1000).toLocaleString() : "";
  return [c.subject, `${c.hash} · ${c.author}${full ? ` · ${full}` : ""}`, c.refs.join("  ")]
    .filter(Boolean)
    .join("\n");
}

function fmtTime(secs: number): string {
  return relTime(new Date(secs * 1000).toISOString());
}

/** 点击提交行：复制短 hash（贴进对话给 AI 引用），行内短暂显示「已复制」 */
function copyHash(c: GitCommit): void {
  void copyText(c.short);
  copiedShort.value = c.short;
  clearTimeout(copiedTimer);
  copiedTimer = setTimeout(() => (copiedShort.value = null), 1200);
}

// ---- 提交的变更文件：行首箭头懒加载展开，点文件行给主区开 diff ----
const expandedCommits = ref<Set<string>>(new Set());
interface CommitFilesEntry {
  loading?: boolean;
  error?: string | null;
  files?: CommitFileStat[];
}
const filesByCommit = ref<Map<string, CommitFilesEntry>>(new Map());

async function toggleCommitExpand(hash: string): Promise<void> {
  const next = new Set(expandedCommits.value);
  if (next.has(hash)) {
    next.delete(hash);
    expandedCommits.value = next;
    return;
  }
  next.add(hash);
  expandedCommits.value = next;
  if (filesByCommit.value.get(hash)?.files) return;
  filesByCommit.value.set(hash, { loading: true });
  try {
    const res = await gitApi?.commitFiles(props.cwd, hash);
    filesByCommit.value.set(hash, { files: res?.files ?? [] });
  } catch (err) {
    filesByCommit.value.set(hash, { error: err instanceof Error ? err.message : String(err) });
  }
}

/** 状态首字母 → 徽标配色类（A 新增 / D 删除 / M·T 修改 / 其他中性） */
function statusKind(status: string): string {
  const c = status[0] ?? "";
  return c === "A" ? "add" : c === "D" ? "del" : c === "M" || c === "T" ? "mod" : "other";
}

function openCommitDiff(c: GitCommit, f: CommitFileStat): void {
  emit("open-commit-diff", { hash: c.hash, short: c.short, subject: c.subject, file: f });
}

watch(
  () => props.cwd,
  () => {
    query.value = "";
    expandedDirs.value = new Set();
    view.value = "files";
    commits.value = [];
    hasMore.value = false;
    commitsError.value = null;
    expandedCommits.value = new Set();
    filesByCommit.value = new Map();
    void load();
    void detectGit();
  },
);
onMounted(() => {
  void load();
  void detectGit();
});
</script>

<template>
  <aside class="files-panel">
    <header class="fp-head">
      <Icon name="folder-open-line" :size="17" />
      <span class="fp-title" :title="cwd">{{ title }}</span>
      <span class="flex-sp"></span>
      <!-- 仓库目录才出现的视图切换：文件树 / 提交历史 -->
      <div v-if="isRepo" class="seg">
        <button class="seg-btn" :class="{ on: view === 'files' }" @click="switchView('files')">文件</button>
        <button class="seg-btn" :class="{ on: view === 'commits' }" @click="switchView('commits')">提交</button>
      </div>
      <button
        v-if="view === 'files'"
        class="icon-btn"
        :title="allExpanded ? '收起全部' : '展开全部'"
        :disabled="loading || !!loadError"
        @click="toggleAllDirs"
      ><Icon :name="allExpanded ? 'collapse-vertical-line' : 'expand-vertical-line'" :size="15" /></button>
      <button
        class="icon-btn"
        :title="view === 'commits' ? '刷新提交' : '刷新文件'"
        :disabled="loading || commitsLoading"
        @click="refresh"
      ><Icon name="refresh-line" :size="15" /></button>
      <button
        v-if="revealPath"
        class="icon-btn"
        title="在资源管理器中打开"
        @click="revealPath(cwd)"
      ><Icon name="folder-line" :size="15" /></button>
      <button class="icon-btn" title="关闭" @click="emit('close')"><Icon name="close-line" :size="16" /></button>
    </header>

    <div class="fp-search">
      <input
        v-model="query"
        :placeholder="view === 'commits' ? '搜索提交 / hash / 作者…' : '搜索文件…'"
        @keydown.esc="query = ''"
      />
    </div>

    <div class="fp-tree">
      <!-- 提交历史视图：双行行（subject + hash·作者·相对时间），refs 徽标，点击复制短 hash -->
      <template v-if="view === 'commits'">
        <div v-if="commitsLoading && !commits.length" class="fp-hint"><Icon name="loader-2-line" :size="16" />加载中…</div>
        <div v-else-if="commitsError" class="fp-hint"><Icon name="error-warning-line" :size="16" />{{ commitsError }}</div>
        <template v-else>
          <div v-for="c in filteredCommits" :key="c.hash" class="cwrap">
            <div class="crow" :title="rowTitle(c)" @click="copyHash(c)">
              <span
                class="chev"
                :class="{ fold: !expandedCommits.has(c.hash) }"
                :title="expandedCommits.has(c.hash) ? '收起变更文件' : '展开变更文件'"
                @click.stop="toggleCommitExpand(c.hash)"
              ><Icon name="arrow-down-s-line" :size="13" /></span>
              <div class="crow-main">
                <div class="crow-top">
                  <span class="crow-subject">{{ c.subject }}</span>
                  <span
                    v-for="r in c.refs.slice(0, 2)"
                    :key="r"
                    class="crow-ref"
                    :class="{ head: isHeadRef(r) }"
                  >{{ refName(r) }}</span>
                </div>
                <div class="crow-meta">
                  <span class="crow-hash">{{ c.short }}</span>
                  <span class="crow-author">{{ c.author }}</span>
                  <span>{{ fmtTime(c.time) }}</span>
                  <span class="flex-sp"></span>
                  <span v-if="copiedShort === c.short" class="crow-copied">已复制</span>
                  <Icon class="crow-copy" name="file-copy-line" :size="13" />
                </div>
              </div>
            </div>
            <!-- 展开的变更文件清单（懒加载）：点文件行 → 主区审查面板开该提交的 diff -->
            <template v-if="expandedCommits.has(c.hash)">
              <div v-if="filesByCommit.get(c.hash)?.loading" class="fp-hint cf-hint">
                <Icon name="loader-2-line" :size="14" />加载中…
              </div>
              <div v-else-if="filesByCommit.get(c.hash)?.error" class="fp-hint cf-hint">
                <Icon name="error-warning-line" :size="14" />{{ filesByCommit.get(c.hash)?.error }}
              </div>
              <template v-else>
                <div
                  v-for="f in filesByCommit.get(c.hash)?.files ?? []"
                  :key="f.path"
                  class="cfrow"
                  :title="f.path"
                  @click="openCommitDiff(c, f)"
                >
                  <span class="cf-status" :class="'st-' + statusKind(f.status)">{{ f.status[0] }}</span>
                  <span class="cf-name">{{ basename(f.path) }}</span>
                  <span class="row-sp"></span>
                  <span class="cf-stat">
                    <i v-if="f.added > 0" class="plus">+{{ f.added }}</i>
                    <i v-if="f.removed > 0" class="minus">−{{ f.removed }}</i>
                  </span>
                </div>
                <div v-if="!filesByCommit.get(c.hash)?.files?.length" class="fp-hint cf-hint">无变更文件</div>
              </template>
            </template>
          </div>
          <div v-if="!filteredCommits.length" class="fp-hint">
            {{ commits.length ? "没有匹配的提交" : "暂无提交" }}
          </div>
          <button
            v-if="hasMore && !query.trim()"
            class="more-btn"
            :disabled="commitsLoading"
            @click="loadCommits(true)"
          >{{ commitsLoading ? "加载中…" : "加载更多" }}</button>
        </template>
      </template>
      <template v-else>
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

/* 头部视图分段切换（仓库目录才出现） */
.seg {
  display: flex;
  flex: none;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  padding: 2px;
}
.seg-btn {
  border: none;
  background: none;
  padding: 2px 9px;
  border-radius: 6px;
  color: var(--pd-text-3);
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
}
.seg-btn:hover { color: var(--pd-text-2); }
.seg-btn.on { background: var(--pd-bg-active); color: var(--pd-text); font-weight: 600; }

/* 提交历史行：第一行 subject + refs 徽标，第二行 hash · 作者 · 相对时间；点击复制短 hash，
   行首箭头展开变更文件清单 */
.cwrap { min-width: 0; }
.crow {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 8px 6px 4px;
  border-radius: 6px;
  cursor: pointer;
  user-select: none;
}
.crow:hover { background: var(--pd-bg-hover); }
.crow .chev {
  display: grid;
  place-items: center;
  width: 14px;
  flex: none;
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.crow .chev.fold { transform: rotate(-90deg); }
.crow .chev:hover { color: var(--pd-text-2); }
.crow-main { min-width: 0; flex: 1; }
.crow-top {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.crow-subject {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
}
.crow-ref {
  flex: none;
  padding: 0 5px;
  border-radius: 5px;
  background: var(--pd-bg-active);
  color: var(--pd-text-2);
  font-size: calc(11px * var(--pd-font-scale));
  line-height: 16px;
  white-space: nowrap;
}
.crow-ref.head { color: var(--pd-accent); font-weight: 600; }
.crow-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
  color: var(--pd-text-4);
  font-size: calc(11.5px * var(--pd-font-scale));
}
.crow-hash {
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  color: var(--pd-text-3);
}
.crow-author {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  max-width: 40%;
}
.crow-copied { color: var(--pd-accent); flex: none; }
.crow-copy { flex: none; opacity: 0; color: var(--pd-text-4); }
.crow:hover .crow-copy { opacity: 1; }
.more-btn {
  display: block;
  margin: 6px auto 2px;
  padding: 4px 14px;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  background: none;
  color: var(--pd-text-3);
  font-size: calc(12px * var(--pd-font-scale));
  cursor: pointer;
}
.more-btn:hover:not(:disabled) { background: var(--pd-bg-hover); color: var(--pd-text); }
.more-btn:disabled { opacity: 0.5; pointer-events: none; }

/* 提交展开的变更文件行：状态徽标（A/M/D/T）+ 文件名 + +/- 行数；点击开提交 diff */
.cf-hint { padding: 4px 6px 6px 26px; }
.cfrow {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  margin-left: 22px;
  padding-right: 6px;
  border-radius: 6px;
  color: var(--pd-text-2);
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}
.cfrow:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.cf-status {
  flex: none;
  width: 15px;
  height: 15px;
  border-radius: 4px;
  display: grid;
  place-items: center;
  font-size: calc(10.5px * var(--pd-font-scale));
  font-weight: 700;
  line-height: 1;
}
.cf-status.st-add { color: #4caf50; }
.cf-status.st-del { color: #ef5350; }
.cf-status.st-mod { color: #ffb74d; }
.cf-status.st-other { color: var(--pd-text-3); }
.cf-name {
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}
.cf-stat {
  flex: none;
  display: flex;
  gap: 5px;
  font-size: calc(11px * var(--pd-font-scale));
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
}
.cf-stat .plus { color: #4caf50; font-style: normal; }
.cf-stat .minus { color: #ef5350; font-style: normal; }
</style>
