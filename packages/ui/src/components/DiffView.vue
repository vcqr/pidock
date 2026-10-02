<script setup lang="ts">
import { computed } from "vue";

/**
 * edit/write 参数的 diff 视图：行号 + 红（删除）/绿（新增）行背景，
 * 参考 ZCode 的展开样式。大文件用 LCS（有上限），超限退化为整块删除+新增。
 */
const props = defineProps<{
  oldText: string;
  newText: string;
  file?: string;
}>();

interface Op {
  type: "add" | "del" | "ctx";
  text: string;
}

const MAX_CELLS = 1_000_000;
const MAX_ROWS = 600;

const ops = computed<Op[]>(() => {
  const oldLines = props.oldText ? props.oldText.split("\n") : [];
  const newLines = props.newText ? props.newText.split("\n") : [];
  const n = oldLines.length;
  const m = newLines.length;
  if (n * m > MAX_CELLS) {
    return [
      ...oldLines.map((t) => ({ type: "del" as const, text: t })),
      ...newLines.map((t) => ({ type: "add" as const, text: t })),
    ];
  }
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i]![j] = oldLines[i] === newLines[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
    }
  }
  const out: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      out.push({ type: "ctx", text: oldLines[i]! });
      i++;
      j++;
    } else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) {
      out.push({ type: "del", text: oldLines[i]! });
      i++;
    } else {
      out.push({ type: "add", text: newLines[j]! });
      j++;
    }
  }
  while (i < n) out.push({ type: "del", text: oldLines[i++]! });
  while (j < m) out.push({ type: "add", text: newLines[j++]! });
  return out;
});

const rows = computed(() => {
  if (ops.value.length <= MAX_ROWS) return { lines: ops.value, omitted: 0 };
  return {
    lines: [...ops.value.slice(0, MAX_ROWS), { type: "ctx" as const, text: `…（剩余 ${ops.value.length - MAX_ROWS} 行省略）` }],
    omitted: ops.value.length - MAX_ROWS,
  };
});

const stats = computed(() => {
  const add = ops.value.filter((o) => o.type === "add").length;
  const del = ops.value.filter((o) => o.type === "del").length;
  return { add, del };
});
</script>

<template>
  <div class="diff">
    <div class="diff-head">
      <span v-if="file" class="file">{{ file }}</span>
      <span class="stats">
        <span class="add">+{{ stats.add }}</span>
        <span class="del">−{{ stats.del }}</span>
      </span>
    </div>
    <div class="diff-body">
      <div v-for="(op, i) in rows.lines" :key="i" class="dl" :class="op.type">
        <span class="no">{{ i + 1 }}</span>
        <span class="sign">{{ op.type === "add" ? "+" : op.type === "del" ? "−" : " " }}</span>
        <span class="tx">{{ op.text }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.diff {
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  overflow: hidden;
  margin: 2px 0 8px 21px;
  background: var(--pd-code-bg);
}
.diff-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--pd-border-soft);
  background: var(--pd-bg-hover);
}
.file {
  font-family: Consolas, monospace;
  font-size: 11.5px;
  color: var(--pd-text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.stats { margin-left: auto; display: flex; gap: 8px; font-size: 11.5px; font-family: Consolas, monospace; }
.stats .add { color: var(--pd-green); }
.stats .del { color: var(--pd-red); }
.diff-body {
  overflow-x: auto;
  font-family: Consolas, monospace;
  font-size: 12px;
  line-height: 1.65;
  max-height: 320px;
  overflow-y: auto;
}
.dl { display: flex; align-items: flex-start; min-width: max-content; }
.dl.add { background: rgba(52, 193, 132, 0.14); }
.dl.add .tx, .dl.add .no { color: var(--pd-diff-add-text); }
.dl.del { background: rgba(239, 68, 68, 0.12); }
.dl.del .tx, .dl.del .no { color: var(--pd-diff-del-text); }
.dl.ctx .tx { color: var(--pd-text-2); }
.no {
  width: 34px;
  flex: none;
  text-align: right;
  padding: 0 8px;
  color: var(--pd-text-4);
  user-select: none;
}
.sign { width: 16px; flex: none; text-align: center; color: var(--pd-text-4); }
.tx {
  padding-right: 12px;
  white-space: pre;
  color: var(--pd-text-2);
}
</style>
