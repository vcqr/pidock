<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { ATTACHMENT_LOADER } from "../databus.js";
import DiffBlock from "./DiffBlock.vue";

const props = defineProps<{
  toolName: string;
  status: "running" | "done" | "error";
  args?: string;
  partial?: string;
  output?: string;
  attachment?: { attachment_id: string; size: number; truncated: boolean };
}>();

const open = ref(false);
const loader = inject(ATTACHMENT_LOADER, null);
const full = ref<string | null>(null);
const loading = ref(false);

const isDiffTool = computed(() => ["edit", "write", "multiedit"].includes(props.toolName));
const body = computed(() => full.value ?? (props.output || props.partial || props.args || ""));
const statusLabel = computed(() =>
  props.status === "running" ? "运行中…" : props.status === "error" ? "失败" : "完成",
);
const statusIcon = computed(() =>
  props.status === "running" ? "◐" : props.status === "error" ? "✕" : "✓",
);

async function loadFull(): Promise<void> {
  if (!props.attachment || !loader || loading.value) return;
  loading.value = true;
  try {
    full.value = await loader(props.attachment.attachment_id);
  } catch (err) {
    full.value = `加载失败：${String(err)}`;
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="tool-card" :class="status" @click="open = !open">
    <div class="head">
      <span class="status-icon" :class="status">{{ statusIcon }}</span>
      <span class="name">{{ toolName }}</span>
      <span v-if="attachment?.truncated && !full" class="truncated">已截断</span>
      <span class="status">{{ statusLabel }}</span>
      <span class="chevron">{{ open ? "▾" : "▸" }}</span>
    </div>
    <template v-if="open">
      <DiffBlock
        v-if="isDiffTool && !full"
        :tool-name="toolName"
        :args="args || ''"
      />
      <pre v-else-if="body" class="body">{{ body }}</pre>
      <button
        v-if="attachment?.truncated && !full && loader"
        class="load-full"
        :disabled="loading"
        @click.stop="loadFull"
      >
        {{ loading ? "加载中…" : "查看全文（已存对象存储）" }}
      </button>
    </template>
  </div>
</template>

<style scoped>
.tool-card {
  border: 1px solid var(--pd-border);
  border-radius: var(--pd-radius);
  margin: 6px 0;
  overflow: hidden;
  cursor: pointer;
  background: var(--pd-bg-card);
}
.tool-card.running { border-color: var(--pd-purple); }
.tool-card.error { border-color: var(--pd-red); }
.head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  font-size: 12px;
}
.status-icon { font-size: 12px; color: var(--pd-text-3); }
.status-icon.running { color: var(--pd-purple); animation: spin 1.2s linear infinite; display: inline-block; }
.status-icon.error { color: var(--pd-red); }
.status-icon.done { color: var(--pd-green); }
@keyframes spin { to { transform: rotate(360deg); } }
.name {
  color: var(--pd-text);
  font-weight: 600;
  font-family: Consolas, "JetBrains Mono", monospace;
  font-size: 12px;
}
.truncated {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--pd-yellow-soft);
  color: var(--pd-yellow-text);
}
.status { color: var(--pd-text-3); margin-left: auto; }
.chevron { color: var(--pd-text-4); }
.body {
  margin: 0;
  padding: 8px 10px;
  border-top: 1px solid var(--pd-border);
  max-height: 260px;
  overflow: auto;
  font-size: 11.5px;
  color: var(--pd-text-2);
  white-space: pre-wrap;
  word-break: break-all;
}
.load-full {
  width: 100%;
  border: none;
  border-top: 1px solid var(--pd-border);
  background: var(--pd-bg-raised);
  color: var(--pd-accent);
  font-size: 12px;
  padding: 7px;
  cursor: pointer;
}
.load-full:disabled { opacity: 0.5; }
</style>
