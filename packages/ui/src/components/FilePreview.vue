<script setup lang="ts">
import { computed } from "vue";
import { highlightCode } from "../fileHighlight.js";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 文件预览浮层（编辑器式）：多标签页 + 面包屑 + 行号 + 语法高亮。
 * 覆盖在主对话区上方（定位由使用方的 class 决定），临时查看用；
 * 标签状态由 ChatView 持有（文件树点击 → 打开/激活），本组件纯展示。
 */
export interface PreviewFileTab {
  path: string;
  text?: string;
  truncated?: boolean;
  binary?: boolean;
  size?: number;
  /** 读取失败时的错误信息 */
  error?: string;
}

const props = defineProps<{
  tabs: PreviewFileTab[];
  /** 当前激活的标签路径 */
  active: string | null;
  /** 面包屑根段（项目显示名） */
  projectName: string;
}>();
const emit = defineEmits<{
  activate: [path: string];
  /** 关闭标签（标签 ×） */
  close: [path: string];
  /** 关闭全部标签（浮层右上 ×，恢复对话） */
  closeAll: [];
}>();

const activeTab = computed(() => props.tabs.find((t) => t.path === props.active) ?? null);

const highlighted = computed(() => {
  const t = activeTab.value;
  if (!t || t.binary || t.error) return "";
  return highlightCode(t.path, t.text ?? "");
});
const gutterText = computed(() => {
  const t = activeTab.value;
  if (!t || t.binary || t.error) return "";
  return Array.from({ length: (t.text ?? "").split("\n").length }, (_, i) => i + 1).join("\n");
});
const crumbSegments = computed(() => (activeTab.value ? activeTab.value.path.split("/") : []));
</script>

<template>
  <div class="file-preview">
    <!-- 标签栏（行末：关闭全部，恢复对话） -->
    <div class="pv-tabs">
      <div
        v-for="t in tabs"
        :key="t.path"
        class="pv-tab"
        :class="{ on: t.path === active }"
        :title="t.path"
        @click="emit('activate', t.path)"
      >
        <FileIcon :path="t.path" :size="15" />
        <span class="tab-name">{{ t.path.split("/").pop() }}</span>
        <button class="tab-close" title="关闭标签" @click.stop="emit('close', t.path)">
          <Icon name="close-line" :size="12" />
        </button>
      </div>
      <span class="tabs-sp"></span>
      <button class="pv-close-all" title="关闭预览" @click="emit('closeAll')">
        <Icon name="close-line" :size="16" />
      </button>
    </div>

    <template v-if="activeTab">
      <!-- 面包屑：项目 › 路径段（末段带图标） -->
      <div class="pv-crumb">
        <span class="crumb-root">{{ projectName }}</span>
        <template v-for="(seg, i) in crumbSegments" :key="i">
          <span class="crumb-sep">›</span>
          <span v-if="i === crumbSegments.length - 1" class="crumb-last">
            <FileIcon :path="activeTab.path" :size="14" />{{ seg }}
          </span>
          <span v-else class="crumb-seg">{{ seg }}</span>
        </template>
      </div>
      <!-- 代码区：行号列（sticky）+ 高亮代码，随容器双向滚动 -->
      <div v-if="activeTab.error" class="pv-note err">
        <Icon name="error-warning-line" :size="14" />{{ activeTab.error }}
      </div>
      <div v-else-if="activeTab.binary" class="pv-note">二进制文件，无法预览文本内容</div>
      <div v-else class="pv-scroll">
        <pre class="pv-gutter">{{ gutterText }}</pre>
        <pre class="pv-code" v-html="highlighted"></pre>
      </div>
      <div v-if="!activeTab.error && !activeTab.binary && activeTab.truncated" class="pv-note pv-foot">
        文件过大，仅显示前 256 KB
      </div>
    </template>
  </div>
</template>

<style scoped>
.file-preview {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--pd-bg-panel);
}

/* ---- 标签栏 ---- */
.pv-tabs {
  display: flex;
  align-items: flex-end;
  gap: 3px;
  padding: 6px 8px 0 10px;
  overflow-x: auto;
  flex: none;
  scrollbar-width: none;
}
.pv-tabs::-webkit-scrollbar { display: none; }
.pv-tab {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 5px 5px 5px 9px;
  border: 1px solid transparent;
  border-bottom: none;
  border-radius: 8px 8px 0 0;
  color: var(--pd-text-3);
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
  white-space: nowrap;
  user-select: none;
}
.pv-tab:hover { color: var(--pd-text-2); background: var(--pd-bg-hover); }
.pv-tab.on {
  background: var(--pd-bg);
  border-color: var(--pd-border-soft);
  color: var(--pd-text);
}
.tab-name {
  max-width: 110px;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tab-close {
  width: 16px;
  height: 16px;
  border-radius: 4px;
  display: grid;
  place-items: center;
  color: var(--pd-text-4);
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  flex: none;
}
.tab-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.tabs-sp { flex: 1; }
.pv-close-all {
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
  margin-bottom: 3px;
}
.pv-close-all:hover { background: var(--pd-bg-hover); color: var(--pd-text); }

/* ---- 面包屑 ---- */
.pv-crumb {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 6px 12px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-3);
  border-bottom: 1px solid var(--pd-border-soft);
  white-space: nowrap;
  overflow: hidden;
  flex: none;
}
.crumb-root { color: var(--pd-text-2); font-weight: 600; flex: none; }
.crumb-sep { color: var(--pd-text-4); flex: none; }
.crumb-seg { overflow: hidden; text-overflow: ellipsis; }
.crumb-last {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--pd-text);
  flex: none;
}

/* ---- 代码区 ---- */
.pv-scroll {
  flex: 1;
  overflow: auto;
  display: flex;
  align-items: flex-start;
  background: var(--pd-bg);
  min-height: 0;
}
.pv-scroll::-webkit-scrollbar { width: 8px; height: 8px; }
.pv-scroll::-webkit-scrollbar-thumb { background: var(--pd-scrollbar); border-radius: 4px; }
.pv-gutter, .pv-code {
  margin: 0;
  font-family: var(--pd-mono, ui-monospace, Consolas, monospace);
  font-size: calc(12.5px * var(--pd-font-scale));
  line-height: 1.6;
}
.pv-gutter {
  position: sticky;
  left: 0;
  z-index: 1;
  flex: none;
  min-width: 40px;
  padding: 10px 8px 14px 12px;
  text-align: right;
  color: var(--pd-text-4);
  background: var(--pd-bg);
  border-right: 1px solid var(--pd-border-soft);
  user-select: none;
}
.pv-code {
  flex: none;
  padding: 10px 16px 14px 10px;
  color: var(--pd-text-2);
  white-space: pre;
}
.pv-note {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 14px;
  color: var(--pd-text-4);
  font-size: calc(12.5px * var(--pd-font-scale));
}
.pv-note.err { color: var(--pd-red); }
.pv-foot { flex: none; border-top: 1px solid var(--pd-border-soft); }
</style>
