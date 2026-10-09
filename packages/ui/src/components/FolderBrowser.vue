<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref } from "vue";
import { FS_LIST } from "../databus.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 自绘文件/文件夹选择弹层：服务端目录浏览（fs_list 命令），替代系统原生对话框。
 * 桌面与 webhost 共用——web 模式下系统对话框不可用，改由前端浏览服务端目录。
 * - mode="dir"（默认）：点目录行进入，路径栏可跳转，「选择当前目录」确认；
 * - mode="file"：目录行进入，文件行选中（accept 可按扩展名过滤），「选择此文件」确认。
 */
const props = withDefaults(
  defineProps<{
    /** dir = 选目录（打开项目文件夹）；file = 选文件（头像/技能包/插件导入） */
    mode?: "dir" | "file";
    /** file 模式的扩展名白名单（如 [".png", ".jpg"]）；缺省不过滤 */
    accept?: string[];
  }>(),
  { mode: "dir" },
);
const emit = defineEmits<{ pick: [path: string]; close: [] }>();

const fsList = inject(FS_LIST, null);
const path = ref("");
const parent = ref<string | null>(null);
const entries = ref<Array<{ name: string; path: string; dir: boolean }>>([]);
const loading = ref(false);
const err = ref("");
/** 路径栏与实际所在目录分离：Enter 才跳转 */
const pathInput = ref("");
/** file 模式下已选中的文件 */
const selectedFile = ref("");
/** 常用目录快捷入口（桌面/下载/图片/文档），随 fs_list 返回 */
const specials = ref<Partial<Record<"desktop" | "downloads" | "pictures" | "documents", string | null>>>({});
/** 默认不显示点开头的隐藏文件，避免家目录被系统目录刷屏 */
const showHidden = ref(false);

const SPECIAL_ITEMS = [
  { key: "desktop", label: "桌面", icon: "computer-line" },
  { key: "downloads", label: "下载", icon: "download-cloud-2-line" },
  { key: "pictures", label: "图片", icon: "image-add-line" },
  { key: "documents", label: "文档", icon: "file-line" },
] as const;
const quickLinks = computed(() =>
  SPECIAL_ITEMS.filter((s) => !!specials.value[s.key]).map((s) => ({ ...s, path: specials.value[s.key]! })),
);

/** 展示条目：dir 模式只显示目录；file 模式显示全部（accept 过滤文件扩展名）；默认滤掉隐藏文件 */
const visibleEntries = computed(() => {
  const all = showHidden.value ? entries.value : entries.value.filter((e) => !e.name.startsWith("."));
  if (props.mode === "dir") return all.filter((e) => e.dir);
  const accept = props.accept ?? [];
  if (!accept.length) return all;
  return all.filter(
    (e) => e.dir || accept.some((ext) => e.name.toLowerCase().endsWith(ext.toLowerCase())),
  );
});

async function load(target?: string): Promise<void> {
  if (!fsList) {
    err.value = "当前环境不支持目录浏览";
    return;
  }
  loading.value = true;
  err.value = "";
  selectedFile.value = "";
  try {
    // 网络偶发丢请求（半开连接等）时不能把弹层卡死：15s 超时后可重试
    const res = await Promise.race([
      fsList(target),
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error("加载超时，请重试")), 15000),
      ),
    ]);
    path.value = res.path;
    pathInput.value = res.path;
    parent.value = res.parent;
    entries.value = res.entries;
    if (res.specials) specials.value = res.specials;
  } catch (e) {
    err.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

function go(p: string): void {
  void load(p);
}

function jump(): void {
  const p = pathInput.value.trim();
  if (p && p !== path.value) go(p);
}

/** 目录行进入；文件行选中（file 模式） */
function onEntryClick(e: { name: string; path: string; dir: boolean }): void {
  if (e.dir) {
    go(e.path);
    return;
  }
  if (props.mode === "file") {
    selectedFile.value = selectedFile.value === e.path ? "" : e.path;
  }
}

function confirm(): void {
  if (props.mode === "file") {
    if (selectedFile.value) emit("pick", selectedFile.value);
    return;
  }
  if (path.value) emit("pick", path.value);
}

onMounted(() => void load());
onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    emit("close");
  }
}
</script>

<template>
  <div class="fb-mask" @click.self="emit('close')">
    <div class="fb-box" role="dialog" aria-modal="true">
      <header class="fb-head">
        <span>{{ mode === "file" ? "选择文件" : "选择项目文件夹" }}</span>
        <button class="fb-x" title="关闭" @click="emit('close')">
          <Icon name="close-line" :size="14" />
        </button>
      </header>
      <div class="fb-path">
        <input
          v-model="pathInput"
          spellcheck="false"
          placeholder="目录绝对路径，Enter 跳转"
          @keydown.enter="jump"
        />
        <button class="fb-go" :disabled="loading" title="跳转" @click="jump">
          <Icon name="arrow-right-line" :size="13" />
        </button>
      </div>
      <div class="fb-quick">
        <button
          v-for="q in quickLinks"
          :key="q.key"
          class="fb-quick-btn"
          :class="{ on: path === q.path }"
          @click="go(q.path)"
        >
          <Icon :name="q.icon" :size="13" />
          <span>{{ q.label }}</span>
        </button>
        <span class="fb-sp"></span>
        <button
          class="fb-quick-btn"
          :class="{ on: showHidden }"
          :title="showHidden ? '隐藏点开头的文件' : '显示点开头的隐藏文件'"
          @click="showHidden = !showHidden"
        >
          <Icon :name="showHidden ? 'eye-line' : 'eye-off-line'" :size="13" />
          <span>隐藏文件</span>
        </button>
      </div>
      <div class="fb-list">
        <button v-if="parent" class="fb-item fb-up" @click="go(parent)">
          <Icon name="arrow-up-line" :size="14" />
          <span class="fb-name">..</span>
        </button>
        <button
          v-for="e in visibleEntries"
          :key="e.path"
          class="fb-item"
          :class="{ on: e.path === selectedFile }"
          :title="e.path"
          @click="onEntryClick(e)"
        >
          <FileIcon v-if="!e.dir" :path="e.name" :dir="false" :size="14" />
          <Icon v-else name="folder-line" :size="14" />
          <span class="fb-name">{{ e.name }}</span>
        </button>
        <div v-if="!loading && !visibleEntries.length" class="fb-empty">
          {{
            !entries.length
              ? "此目录为空"
              : visibleEntries.length === 0 && !showHidden && entries.length
                ? "没有可见条目（可点右上角显示隐藏文件）"
                : "没有匹配的文件"
          }}
        </div>
        <div v-if="loading" class="fb-empty">加载中...</div>
      </div>
      <div v-if="err" class="fb-err">{{ err }}</div>
      <footer class="fb-foot">
        <button class="fb-btn ghost" @click="go('')">主目录</button>
        <span class="fb-sp"></span>
        <button class="fb-btn ghost" @click="emit('close')">取消</button>
        <button
          v-if="mode === 'file'"
          class="fb-btn ok"
          :disabled="!selectedFile || loading"
          @click="confirm"
        >
          选择此文件
        </button>
        <button v-else class="fb-btn ok" :disabled="!path || loading" @click="confirm">
          选择当前目录
        </button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.fb-mask {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: rgba(0, 0, 0, 0.5);
  display: grid;
  place-items: center;
}
.fb-box {
  width: 460px;
  max-width: 92vw;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  box-shadow: var(--pd-shadow);
  padding: 14px 16px 14px;
}
.fb-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 14px;
  font-weight: 600;
  color: var(--pd-text);
  margin-bottom: 10px;
}
.fb-x {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-4);
  cursor: pointer;
}
.fb-x:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-path {
  display: flex;
  gap: 6px;
  margin-bottom: 8px;
}
.fb-path input {
  flex: 1;
  min-width: 0;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  color: var(--pd-text);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 6px 9px;
}
.fb-path input:focus { outline: none; border-color: var(--pd-accent); }
.fb-go {
  display: grid;
  place-items: center;
  width: 30px;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  background: none;
  color: var(--pd-text-2);
  cursor: pointer;
}
.fb-go:hover:not(:disabled) { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-go:disabled { opacity: 0.5; cursor: default; }
.fb-quick {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-bottom: 8px;
  flex-wrap: wrap;
}
.fb-sp { flex: 1; }
.fb-quick-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-3);
  font-size: calc(11.5px * var(--pd-font-scale));
  cursor: pointer;
}
.fb-quick-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-quick-btn.on { background: var(--pd-bg-hover); color: var(--pd-accent); }
.fb-list {
  height: 300px;
  overflow-y: auto;
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  padding: 4px;
}
.fb-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  text-align: left;
  padding: 7px 9px;
  border-radius: 7px;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.fb-item:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-item.on { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-item svg { color: var(--pd-text-3); flex: none; }
.fb-up { color: var(--pd-text-3); }
.fb-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.fb-empty {
  padding: 12px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  text-align: center;
}
.fb-err {
  margin-top: 8px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-red-text);
  word-break: break-all;
}
.fb-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
}
.fb-btn {
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 12.5px;
  cursor: pointer;
  border: 1px solid var(--pd-border);
  background: none;
  color: var(--pd-text-2);
}
.fb-btn.ghost:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.fb-btn.ok {
  background: var(--pd-accent);
  border-color: var(--pd-accent);
  color: #1a1a1a;
  font-weight: 600;
}
.fb-btn.ok:hover:not(:disabled) { background: var(--pd-accent-hover); }
.fb-btn.ok:disabled { opacity: 0.5; cursor: default; }
</style>
