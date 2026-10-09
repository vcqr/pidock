<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView, highlightSpecialChars, lineNumbers } from "@codemirror/view";
import {
  bracketMatching,
  defaultHighlightStyle,
  syntaxHighlighting,
  HighlightStyle,
  LanguageDescription,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { unifiedMergeView } from "@codemirror/merge";
import { languages } from "@codemirror/language-data";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";
import { themeMode } from "../theme.js";

/**
 * 「审查」右侧面板：多标签 + CodeMirror 只读 unified merge。
 * 每个被审查的文件一个标签（可切换/单独关闭），编辑器对比文件快照
 * 与当前内容 — 新增行绿底、删除块红底，未变更长段自动折叠。
 */
export interface ReviewTab {
  path: string;
  oldText: string;
  newText: string;
  added: number;
  removed: number;
}

const props = defineProps<{ tabs: ReviewTab[]; active: string }>();
const emit = defineEmits<{
  select: [path: string];
  "close-tab": [path: string];
  close: [];
}>();

const host = ref<HTMLElement | null>(null);
let view: EditorView | null = null;
let builtSig = "";

const activeTab = computed(
  () => props.tabs.find((t) => t.path === props.active) ?? props.tabs[0],
);

function baseName(p: string): string {
  return p.replace(/\\/g, "/").split("/").pop() ?? p;
}
function dirName(p: string): string {
  const segs = p.replace(/\\/g, "/").split("/").filter(Boolean);
  segs.pop();
  return segs.slice(-2).join("/") + "/";
}

/** 主题随 themeMode 实时构建（旧实现模块加载时一次性求值，深→浅切换后面板残留深色样式） */
// 暗色语法高亮：defaultHighlightStyle 是为浅背景设计的深色 token，暗色背景下几乎不可读
const darkHighlight = HighlightStyle.define([
  { tag: t.comment, color: "#7f848e", fontStyle: "italic" },
  { tag: [t.keyword, t.moduleKeyword, t.controlKeyword], color: "#c678dd" },
  { tag: [t.string, t.special(t.string), t.character], color: "#98c379" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "#d19a66" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: "#61afef" },
  { tag: [t.typeName, t.className, t.namespace], color: "#e5c07b" },
  { tag: [t.variableName, t.propertyName, t.definition(t.variableName)], color: "#e06c75" },
  { tag: [t.operator, t.punctuation, t.separator, t.bracket], color: "#abb2bf" },
  { tag: [t.meta, t.processingInstruction], color: "#56b6c2" },
  { tag: [t.heading, t.strong], color: "#e06c75", fontWeight: "bold" },
  { tag: [t.link, t.url], color: "#98c379", textDecoration: "underline" },
  { tag: t.invalid, color: "#f44747" },
]);

function makeReviewTheme() {
  return EditorView.theme(
    {
      "&": {
        height: "100%",
        fontSize: "12.5px",
        backgroundColor: "var(--pd-bg)",
        color: "var(--pd-text-2)",
      },
      ".cm-scroller": {
        fontFamily: "Consolas, 'Courier New', monospace",
        lineHeight: "1.7",
      },
      ".cm-content": { padding: "10px 0 40px" },
      ".cm-gutters": {
        backgroundColor: "var(--pd-bg)",
        color: "var(--pd-text-4)",
        border: "none",
        borderRight: "1px solid var(--pd-border-soft)",
      },
      ".cm-activeLine": { backgroundColor: "transparent" },
      ".cm-activeLineGutter": { backgroundColor: "transparent" },
      "&.cm-focused": { outline: "none" },
      ".cm-changedLine": { backgroundColor: "var(--pd-diff-add)" },
      ".cm-changedText": { color: "var(--pd-diff-add-text)" },
      ".cm-deletedChunk": {
        backgroundColor: "var(--pd-diff-del)",
        color: "var(--pd-diff-del-text)",
      },
      ".cm-deletedChunk .cm-deletedText": { color: "var(--pd-diff-del-text)" },
    },
    { dark: themeMode.value === "dark" },
  );
}

async function loadLanguage(filename: string) {
  const desc = LanguageDescription.matchFilename(languages, filename);
  if (!desc) return null;
  try {
    return await desc.load();
  } catch {
    return null; // 模式加载失败不阻塞审查
  }
}

/** 按「路径+内容长度+主题」签名重建编辑器；标签切换/内容更新/主题切换时才重建 */
async function buildEditor(): Promise<void> {
  const t = activeTab.value;
  if (!t || !host.value) return;
  const sig = `${t.path}#${t.oldText.length}#${t.newText.length}#${themeMode.value}`;
  if (sig === builtSig) return;
  builtSig = sig;
  view?.destroy();
  view = null;
  view = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: t.newText,
      extensions: [
        lineNumbers(),
        highlightSpecialChars(),
        EditorView.lineWrapping,
        bracketMatching(),
        syntaxHighlighting(themeMode.value === "dark" ? darkHighlight : defaultHighlightStyle, { fallback: true }),
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        makeReviewTheme(),
        unifiedMergeView({
          original: t.oldText,
          highlightChanges: true,
          gutter: true,
          mergeControls: false,
          collapseUnchanged: { margin: 3, minSize: 10 },
        }),
      ],
    }),
  });
  const support = await loadLanguage(baseName(t.path));
  if (support && view && builtSig === sig) {
    view.dispatch({ effects: StateEffect.appendConfig.of(support) });
  }
}

watch(
  () => [props.active, props.tabs.map((t) => `${t.path}:${t.oldText.length}:${t.newText.length}`).join("|")],
  () => void buildEditor(),
);
watch(themeMode, () => void buildEditor());
onMounted(() => void buildEditor());
onBeforeUnmount(() => {
  view?.destroy();
  view = null;
});
</script>

<template>
  <aside class="review-pane">
    <div class="rv-tabs">
      <button
        v-for="t in tabs"
        :key="t.path"
        class="rv-tab"
        :class="{ on: t.path === active }"
        :title="t.path"
        @click="emit('select', t.path)"
      >
        <FileIcon :path="t.path" :size="13" />
        <span class="rv-tab-name">{{ baseName(t.path) }}</span>
        <span class="rv-tab-x" title="关闭标签" @click.stop="emit('close-tab', t.path)">
          <Icon name="close-line" :size="11" />
        </span>
      </button>
      <span class="flex-sp"></span>
      <button class="rv-close" title="关闭审查" @click="emit('close')">
        <Icon name="close-line" :size="14" />
      </button>
    </div>
    <header v-if="activeTab" class="rv-head">
      <FileIcon :path="activeTab.path" :size="15" />
      <span class="rv-dir">{{ dirName(activeTab.path) }}</span>
      <span class="rv-name">{{ baseName(activeTab.path) }}</span>
      <span class="flex-sp"></span>
      <span v-if="activeTab.added" class="rv-add">+{{ activeTab.added }}</span>
      <span v-if="activeTab.removed" class="rv-del">−{{ activeTab.removed }}</span>
    </header>
    <div ref="host" class="rv-editor"></div>
  </aside>
</template>

<style scoped>
.review-pane {
  /* 宽度由外层 NSplit 的 pane 控制，这里填满即可 */
  width: 100%;
  flex: none;
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-left: 1px solid var(--pd-border-soft);
  background: var(--pd-bg);
}
.rv-tabs {
  display: flex;
  align-items: center;
  gap: 2px;
  height: 38px;
  padding: 0 8px 0 6px;
  border-bottom: 1px solid var(--pd-border-soft);
  background: var(--pd-bg-panel);
  flex: none;
  overflow-x: auto;
}
.rv-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 180px;
  padding: 5px 8px;
  background: none;
  border: none;
  border-radius: 8px;
  color: var(--pd-text-3);
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
  flex: none;
}
.rv-tab:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.rv-tab.on { background: var(--pd-bg-hover); color: var(--pd-text); }
.rv-tab-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.rv-tab-x {
  display: grid;
  place-items: center;
  width: 16px;
  height: 16px;
  border-radius: 5px;
  color: var(--pd-text-4);
  flex: none;
}
.rv-tab-x:hover { color: var(--pd-text); background: var(--pd-border-soft); }
.flex-sp { flex: 1; }
.rv-close {
  background: none;
  border: none;
  color: var(--pd-text-3);
  cursor: pointer;
  border-radius: 7px;
  padding: 5px;
  display: grid;
  place-items: center;
  flex: none;
}
.rv-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.rv-head {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 12px;
  border-bottom: 1px solid var(--pd-border-soft);
  flex: none;
}
.rv-name {
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
}
.rv-dir {
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.rv-add { color: var(--pd-green); font-size: calc(12px * var(--pd-font-scale)); font-family: var(--pd-mono); }
.rv-del { color: var(--pd-red); font-size: calc(12px * var(--pd-font-scale)); font-family: var(--pd-mono); }
.rv-editor {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.rv-editor :deep(.cm-editor) { height: 100%; }
</style>
