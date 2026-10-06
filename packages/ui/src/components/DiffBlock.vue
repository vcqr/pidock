<script setup lang="ts">
/** edit/write tool args rendered as a colorized diff (best-effort). */
import { computed } from "vue";

const props = defineProps<{
  toolName: string;
  args: string;
}>();

interface DiffLine {
  type: "add" | "del" | "ctx" | "hunk";
  text: string;
}

const parsed = computed<{ file?: string; lines: DiffLine[] } | null>(() => {
  let args: any;
  try {
    args = JSON.parse(props.args);
  } catch {
    return null; // truncated/attachment-ized args cannot be rendered as diff
  }
  if (!args || typeof args !== "object") return null;
  const file: string | undefined =
    args.file_path ?? args.path ?? args.filename ?? undefined;

  const lines: DiffLine[] = [];
  const pushBlock = (text: string | undefined, type: "add" | "del") => {
    if (typeof text !== "string") return;
    for (const l of text.split("\n")) lines.push({ type, text: l });
  };

  if (props.toolName === "edit" || props.toolName === "multiedit") {
    const edits: Array<{ old_string?: string; new_string?: string }> =
      Array.isArray(args.edits)
        ? args.edits
        : [{ old_string: args.old_string, new_string: args.new_string }];
    for (const e of edits) {
      pushBlock(e.old_string, "del");
      pushBlock(e.new_string, "add");
    }
  } else if (props.toolName === "write") {
    pushBlock(args.content ?? args.new_string, "add");
  } else {
    return null;
  }
  if (!lines.length) return null;
  return { file, lines };
});

const stats = computed(() => {
  if (!parsed.value) return null;
  const add = parsed.value.lines.filter((l) => l.type === "add").length;
  const del = parsed.value.lines.filter((l) => l.type === "del").length;
  return { add, del };
});
</script>

<template>
  <div v-if="parsed" class="diff">
    <div class="diff-head">
      <span class="file">{{ parsed.file || toolName }}</span>
      <span v-if="stats" class="stats">
        <span class="add">+{{ stats.add }}</span>
        <span class="del">-{{ stats.del }}</span>
      </span>
    </div>
    <pre class="diff-body"><code><span
      v-for="(l, i) in parsed.lines"
      :key="i"
      class="line"
      :class="l.type"
    >{{ l.type === 'add' ? '+' : l.type === 'del' ? '-' : ' ' }} {{ l.text }}
</span></code></pre>
  </div>
</template>

<style scoped>
.diff {
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--pd-code-bg);
  font-family: var(--pd-mono);
}
.diff-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 5px 10px;
  border-bottom: 1px solid var(--pd-border);
  font-size: calc(11.5px * var(--pd-font-scale));
}
.file { color: var(--pd-accent-text); font-family: inherit; }
.stats { display: flex; gap: 8px; font-size: calc(11px * var(--pd-font-scale)); }
.add { color: var(--pd-diff-add-text); }
.del { color: var(--pd-diff-del-text); }
.diff-body {
  margin: 0;
  padding: 6px 0;
  overflow-x: auto;
  font-size: calc(11.5px * var(--pd-font-scale));
  line-height: 1.55;
}
.line { display: block; padding: 0 10px; white-space: pre-wrap; word-break: break-all; }
.line.add { background: var(--pd-diff-add); color: var(--pd-diff-add-text); }
.line.del { background: var(--pd-diff-del); color: var(--pd-diff-del-text); }
.line.hunk { color: var(--pd-text-3); }
</style>
