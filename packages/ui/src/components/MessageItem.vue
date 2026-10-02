<script setup lang="ts">
import { computed, ref } from "vue";
import type { UiMessageItem } from "../store.js";
import { relTime } from "../utils/time.js";
import MdContent from "./MdContent.vue";
import ToolCard from "./ToolCard.vue";

const props = defineProps<{ item: UiMessageItem; ts?: string }>();

const copied = ref(false);
async function copyMessage(): Promise<void> {
  const text =
    props.item.text ||
    (props.item.blocks ?? [])
      .map((b) => b.text ?? b.output ?? "")
      .join("\n");
  await navigator.clipboard.writeText(text);
  copied.value = true;
  setTimeout(() => (copied.value = false), 1500);
}

function toolBlocks(item: UiMessageItem) {
  return (item.blocks ?? []).filter((b) => b.type === "toolCall" || b.type === "toolResult");
}
function textBlocks(item: UiMessageItem) {
  return (item.blocks ?? []).filter((b) => b.type === "text");
}
function thinkingText(item: UiMessageItem): string {
  if (item.thinking) return item.thinking;
  return (item.blocks ?? [])
    .filter((b) => b.type === "thinking")
    .map((b) => b.thinking ?? "")
    .join("\n");
}
</script>

<template>
  <!-- user message -->
  <div v-if="item.role === 'user'" class="row user">
    <div class="bubble user-bubble" :class="{ pending: item.pending }">
      <span class="content">{{ item.text }}</span>
      <span v-if="item.pending" class="pending-mark">· 发送中</span>
    </div>
  </div>

  <!-- assistant message -->
  <div v-else-if="item.role === 'assistant'" class="row">
    <div class="bubble assistant-bubble">
      <details v-if="thinkingText(item)" class="thinking">
        <summary>💭 思考过程</summary>
        <pre class="thinking-body">{{ thinkingText(item) }}</pre>
      </details>
      <template v-if="item.streaming">
        <MdContent :source="item.text" live />
        <span v-if="!item.text" class="cursor">▍</span>
      </template>
      <template v-else>
        <MdContent
          v-for="(b, i) in textBlocks(item)"
          :key="i"
          :source="b.text || ''"
        />
        <template v-for="(b, i) in toolBlocks(item).filter((b) => b.type === 'toolCall')" :key="'c' + i">
          <ToolCard
            v-if="!['edit', 'write', 'multiedit'].includes(b.toolName || '')"
            :tool-name="b.toolName || ''"
            :args="b.args"
            :attachment="b.attachment"
            status="done"
          />
        </template>
      </template>
    </div>
    <button v-if="!item.streaming" class="copy" :title="copied ? '已复制' : '复制'" @click="copyMessage">
      {{ copied ? "✓" : "⧉" }}
    </button>
  </div>

  <!-- toolResult canonical card -->
  <div v-else class="row">
    <div class="tool-results">
      <ToolCard
        v-for="(b, i) in toolBlocks(item).filter((b) => b.type === 'toolResult')"
        :key="i"
        :tool-name="b.toolName || ''"
        :output="b.output"
        :attachment="b.attachment"
        :status="b.isError ? 'error' : 'done'"
      />
    </div>
  </div>
</template>

<style scoped>
.row { display: flex; margin: 10px 0; gap: 8px; align-items: flex-start; }
.row.user { justify-content: flex-end; }
.bubble {
  max-width: 86%;
  border-radius: var(--pd-radius);
  padding: 9px 13px;
  font-size: 13.5px;
  min-width: 0;
}
.user-bubble {
  background: var(--pd-accent);
  color: #1a1a1a;
  white-space: pre-wrap;
  word-break: break-word;
  border-bottom-right-radius: 4px;
}
[data-theme="light"] .user-bubble { color: #fff; }
.user-bubble.pending { opacity: 0.65; }
.pending-mark { font-size: 11px; opacity: 0.75; margin-left: 6px; }
.assistant-bubble {
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  flex: 1;
  border-bottom-left-radius: 4px;
}
.copy {
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  font-size: 12px;
  cursor: pointer;
  padding: 2px 4px;
  opacity: 0;
  transition: opacity 0.15s;
}
.row:hover .copy { opacity: 1; }
.copy:hover { color: var(--pd-accent); }
.cursor { animation: blink 1s step-start infinite; color: var(--pd-accent); font-weight: 700; }
@keyframes blink { 50% { opacity: 0; } }
.thinking { margin: 2px 0 10px; }
.thinking summary {
  cursor: pointer;
  font-size: 12px;
  color: var(--pd-text-3);
  user-select: none;
}
.thinking-body {
  margin: 6px 0 0;
  padding: 8px 10px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  font-size: 12px;
  color: var(--pd-text-2);
  white-space: pre-wrap;
  max-height: 220px;
  overflow: auto;
}
.tool-results { flex: 1; min-width: 0; }
</style>
