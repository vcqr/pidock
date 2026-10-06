<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import Icon from "./Icon.vue";
import { formatSpan } from "../utils/time.js";

/**
 * 「思考 · 持续了 N 秒」可折叠行。
 * streaming 时 startedAt 存在且无 durationMs → 内部每秒跳动；结束后 durationMs 固定。
 */
const props = defineProps<{
  text: string;
  startedAt?: number;
  durationMs?: number;
}>();

const expanded = ref(false);
const now = ref(Date.now());
let timer: ReturnType<typeof setInterval> | undefined;

function stopTick(): void {
  if (timer) {
    clearInterval(timer);
    timer = undefined;
  }
}
function startTick(): void {
  if (!timer) timer = setInterval(() => (now.value = Date.now()), 1000);
}

watch(
  () => [props.startedAt, props.durationMs],
  ([started, done]) => {
    if (started && !done) startTick();
    else stopTick();
  },
  { immediate: true },
);
onBeforeUnmount(stopTick);

const duration = computed(() => {
  if (props.durationMs) return formatSpan(props.durationMs);
  if (props.startedAt) return formatSpan(Math.max(0, now.value - props.startedAt));
  return null;
});

</script>

<template>
  <div class="think">
    <button class="think-head" @click="expanded = !expanded">
      <Icon name="brain-line" :size="16" />
      <span class="think-label">思考</span>
      <span v-if="duration" class="think-dur">· 持续了 {{ duration }}</span>
      <span class="chev" :class="{ open: expanded }"><Icon name="arrow-down-s-line" :size="12" /></span>
    </button>
    <pre v-if="expanded && text" class="think-body">{{ text }}</pre>
  </div>
</template>

<style scoped>
.think { min-width: 0; }
.think-head {
  display: flex;
  align-items: center;
  gap: 7px;
  background: none;
  border: none;
  padding: 4px 0;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  cursor: pointer;
}
.think-head:hover { color: var(--pd-text); }
.think-head svg { color: var(--pd-text-3); }
.think-label { color: var(--pd-text-2); }
.think-dur { color: var(--pd-text-4); font-size: calc(12px * var(--pd-font-scale)); }
.chev { display: grid; place-items: center; color: var(--pd-text-4); transition: transform 0.12s; }
.chev.open { transform: rotate(180deg); }
.think-body {
  margin: 2px 0 8px 23px;
  padding: 10px 12px;
  background: var(--pd-bg-card);
  border-left: 2px solid var(--pd-border);
  border-radius: 0 8px 8px 0;
  font-family: inherit;
  font-size: calc(12.5px * var(--pd-font-scale));
  line-height: 1.7;
  color: var(--pd-text-3);
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 260px;
  overflow-y: auto;
}
</style>
