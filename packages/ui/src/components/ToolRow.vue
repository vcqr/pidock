<script setup lang="ts">
import { computed, ref } from "vue";
import DiffView from "./DiffView.vue";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 紧凑工具活动行：图标 + 动词 + 目标 + diff 统计 + 状态（✓/失败/进行中）。
 * 用于会话活动流（编辑/终端/查阅/搜索…），点击可展开输出。
 */
const props = withDefaults(
  defineProps<{
    toolName: string;
    args: string;
    status?: "running" | "done" | "error";
    output?: string;
  }>(),
  { status: "done" },
);

const expanded = ref(false);


function base(p: string): string {
  const norm = p.replace(/\\/g, "/");
  return norm.slice(norm.lastIndexOf("/") + 1) || p;
}
function dirOf(p: string): string {
  const norm = p.replace(/\\/g, "/");
  const segs = norm.split("/").filter(Boolean);
  segs.pop();
  if (!segs.length) return "";
  return segs.slice(-2).join("/") + "/";
}

const parsed = computed<Record<string, any>>(() => {
  try {
    const v = JSON.parse(props.args);
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
});

interface Info {
  verb: string;
  icon: string;
  target: string;
  dir?: string;
  /** 目标文件完整路径；存在时前置图标显示文件类型图标 */
  file?: string;
  /** 目标为多行命令：完整展示并按原始换行渲染 */
  wrap?: boolean;
  added?: number;
  removed?: number;
}

const info = computed<Info>(() => {
  const a = parsed.value;
  switch (props.toolName) {
    case "edit": {
      const p = String(a.path ?? "");
      const oldText = String(a.oldText ?? "");
      const newText = String(a.newText ?? "");
      const oldLines = oldText ? oldText.split("\n") : [];
      const newLines = newText ? newText.split("\n") : [];
      const oldSet = new Set(oldLines);
      const newSet = new Set(newLines);
      return {
        verb: "编辑",
        icon: "quill-pen-line",
        target: base(p),
        dir: dirOf(p),
        file: p,
        added: newLines.filter((l) => !oldSet.has(l)).length,
        removed: oldLines.filter((l) => !newSet.has(l)).length,
      };
    }
    case "write": {
      const p = String(a.path ?? "");
      const content = String(a.content ?? a.newText ?? "");
      return { verb: "写入", icon: "file-add-line", target: base(p), dir: dirOf(p), file: p, added: content ? content.split("\n").length : 0 };
    }
    case "bash":
      return { verb: "终端", icon: "terminal-line", target: String(a.command ?? a.cmd ?? ""), wrap: true };
    case "read": {
      const p = String(a.path ?? "");
      return { verb: "查阅", icon: "eye-line", target: base(p), dir: dirOf(p), file: p };
    }
    case "grep":
      return { verb: "搜索", icon: "search-line", target: String(a.pattern ?? "") };
    case "find":
      return { verb: "查找", icon: "file-search-line", target: String(a.pattern ?? a.glob ?? "") };
    case "ls": {
      const p = String(a.path ?? "");
      return { verb: "列目录", icon: "folder-line", target: base(p) || "/" };
    }
    default:
      return { verb: props.toolName, icon: "tools-line", target: props.args.replace(/\s+/g, " ").slice(0, 120) };
  }
});

const fullTarget = computed(() => {
  const a = parsed.value;
  return String(a.path ?? a.command ?? props.args);
});

/** edit/write 展开为 diff（参数里的 oldText/newText / content） */
const hasDiff = computed(() => {
  if (props.toolName === "edit") return !!(parsed.value.oldText ?? parsed.value.newText);
  if (props.toolName === "write") return !!parsed.value.content;
  return false;
});
const diffOld = computed(() => String(parsed.value.oldText ?? ""));
const diffNew = computed(() => {
  if (props.toolName === "write") return String(parsed.value.content ?? "");
  return String(parsed.value.newText ?? "");
});
const hasOutput = computed(() => !!props.output);
const expandable = computed(() => hasDiff.value || hasOutput.value);
</script>

<template>
  <div class="tool-wrap">
    <div class="tool-row" :class="{ clickable: expandable, wrapped: info.wrap }" @click="expandable && (expanded = !expanded)">
      <span class="t-icon"><Icon :name="info.icon" :size="15" /></span>
      <span class="t-verb">{{ info.verb }}</span>
      <span v-if="info.file" class="t-file"><FileIcon :path="info.file" :size="14" /></span>
      <span class="t-target" :class="{ wrap: info.wrap }" :title="fullTarget">{{ info.target }}</span>
      <span v-if="info.dir" class="t-dir">{{ info.dir }}</span>
      <span v-if="info.added" class="t-add">+{{ info.added }}</span>
      <span v-if="info.removed" class="t-del">−{{ info.removed }}</span>
      <span class="flex-sp"></span>
      <span v-if="status === 'error'" class="t-fail">执行失败</span>
      <span v-else-if="status === 'done'" class="t-ok"><Icon name="check-line" :size="12" /></span>
      <span v-else-if="status === 'running'" class="t-spin"></span>
    </div>
    <DiffView
      v-if="expanded && hasDiff"
      :old-text="diffOld"
      :new-text="diffNew"
      :file="fullTarget"
    />
    <pre v-else-if="expanded && hasOutput" class="t-output">{{ output!.slice(-4000) }}</pre>
  </div>
</template>

<style scoped>
.tool-wrap { min-width: 0; }
.tool-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 0;
  font-size: 13px;
  color: var(--pd-text-2);
  min-width: 0;
}
.tool-row.clickable { cursor: pointer; }
.tool-row.wrapped { align-items: flex-start; }
.t-icon { color: var(--pd-text-3); flex: none; display: grid; place-items: center; }
.t-file { flex: none; display: grid; place-items: center; margin-left: -3px; }
.t-verb { color: var(--pd-text-2); flex: none; }
.t-target {
  color: var(--pd-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 420px;
}
/* 多行命令：完整展示，保留原始换行 */
.t-target.wrap {
  flex: 1;
  min-width: 0;
  max-width: none;
  overflow: visible;
  text-overflow: clip;
  white-space: pre-wrap;
  word-break: break-word;
}
.t-dir {
  color: var(--pd-text-4);
  font-size: 12px;
  flex: none;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
}
.t-add { color: var(--pd-green); font-size: 12px; flex: none; }
.t-del { color: var(--pd-red); font-size: 12px; flex: none; }
.flex-sp { flex: 1; }
.t-ok { color: var(--pd-green); flex: none; display: grid; place-items: center; }
.t-fail { color: var(--pd-red); font-size: 12px; flex: none; }
.t-spin {
  width: 11px;
  height: 11px;
  flex: none;
  border: 2px solid var(--pd-text-4);
  border-top-color: transparent;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}
.t-output {
  margin: 2px 0 8px 23px;
  padding: 8px 10px;
  background: var(--pd-code-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  font-family: Consolas, monospace;
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--pd-text-3);
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 220px;
  overflow-y: auto;
}
</style>
