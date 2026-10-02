<script setup lang="ts">
import ToolRow from "./ToolRow.vue";

/** 实时工具活动行（tool_execution_* 事件驱动），渲染为紧凑活动行。 */
defineProps<{
  toolName: string;
  status: "running" | "done" | "error";
  args: string;
  partial?: string;
  output?: string;
  dimmed?: boolean;
}>();
</script>

<template>
  <div class="tool-card" :class="{ dimmed }">
    <ToolRow :tool-name="toolName" :args="args" :status="status" :output="output || partial" />
  </div>
</template>

<style scoped>
.tool-card { padding: 3px 0; min-width: 0; }
.dimmed { opacity: 0.55; }
</style>
