<script setup lang="ts">
defineProps<{ state: string }>();

const labels: Record<string, string> = {
  idle: "空闲",
  thinking: "思考中",
  responding: "回复中",
  executing_tool: "执行工具",
  compacting: "压缩上下文",
  retrying: "重试中",
  running: "运行中",
  waiting_approval: "等待审批",
  waiting_ask: "等待回答",
  done: "完成",
  error: "出错",
};
const colors: Record<string, string> = {
  idle: "var(--pd-text-3)",
  thinking: "var(--pd-yellow)",
  responding: "var(--pd-accent)",
  executing_tool: "var(--pd-purple)",
  compacting: "var(--pd-cyan)",
  retrying: "var(--pd-red)",
  waiting_approval: "var(--pd-yellow)",
  waiting_ask: "var(--pd-yellow)",
  done: "var(--pd-green)",
  error: "var(--pd-red)",
};
</script>

<template>
  <span class="state-pill">
    <span class="dot" :class="{ idle: state === 'idle' }" :style="{ background: colors[state] ?? 'var(--pd-text-3)' }" />
    {{ labels[state] ?? state }}
  </span>
</template>

<style scoped>
.state-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-2);
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  animation: pulse 1.6s ease-in-out infinite;
}
.dot.idle { animation: none; }
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}
</style>
