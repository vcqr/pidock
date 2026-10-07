<script setup lang="ts">
import { computed } from "vue";
import { Icon } from "@pidock/ui";
import { osMeta, relTime, type MachineUi } from "../machine.js";

const props = defineProps<{ machine: MachineUi; sessionCount: number | null }>();
defineEmits<{ enter: []; delete: [] }>();

const meta = computed(() => osMeta(props.machine.os));
const metaLine = computed(() => {
  const parts = [meta.value.name];
  if (props.machine.version) parts.push(`v${props.machine.version}`);
  parts.push(props.sessionCount == null ? "…" : `${props.sessionCount} 个会话`);
  return parts.join(" · ");
});
</script>

<template>
  <article class="mcard" :class="{ offline: !machine.online }" @click="$emit('enter')">
    <svg class="mark" viewBox="0 0 24 24" fill="currentColor" :style="{ color: meta.color }" aria-hidden="true">
      <path :d="meta.path" />
    </svg>
    <div class="row">
      <span class="os-chip" :style="{ color: meta.color, background: meta.color + '1f' }">
        <svg viewBox="0 0 24 24" fill="currentColor"><path :d="meta.path" /></svg>
      </span>
      <span class="status" :class="{ on: machine.online }"><i class="dot" />{{ machine.online ? "在线" : "离线" }}</span>
      <button v-if="!machine.online" class="del" title="删除该节点及其同步数据" @click.stop="$emit('delete')">✕</button>
    </div>
    <h3 class="name">{{ machine.hostname || machine.machine_id.slice(0, 8) }}</h3>
    <div class="meta">{{ metaLine }}</div>
    <div class="foot">
      <span>最后活跃 {{ relTime(machine.last_seen) }}</span>
      <span class="enter">进入<Icon name="arrow-right-line" :size="13" /></span>
    </div>
  </article>
</template>

<style scoped>
.mcard {
  position: relative;
  overflow: hidden;
  background: var(--pd-bg-panel);
  border: 1px solid var(--pd-border);
  border-radius: var(--pd-radius-lg);
  padding: 16px 18px 12px;
  cursor: pointer;
  transition: transform 0.15s ease, border-color 0.15s ease, box-shadow 0.15s ease;
}
.mcard:hover {
  transform: translateY(-2px);
  border-color: var(--pd-accent);
  box-shadow: var(--pd-shadow);
}
.mcard.offline { opacity: 0.72; }
.mcard.offline:hover { opacity: 1; }
.mark {
  position: absolute;
  right: -24px;
  bottom: -30px;
  width: 170px;
  height: 170px;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.25s ease, transform 0.25s ease, filter 0.25s ease;
}
.mcard:hover .mark {
  opacity: 0.26;
  transform: scale(1.05);
  filter: drop-shadow(0 0 14px currentColor);
}
.mcard.offline:hover .mark {
  opacity: 0.16;
  transform: scale(1.05);
}
.row,
.name,
.meta,
.foot { position: relative; }
.row { display: flex; align-items: center; gap: 8px; }
.os-chip {
  width: 34px;
  height: 34px;
  border-radius: 9px;
  display: grid;
  place-items: center;
  flex: none;
}
.os-chip svg { width: 19px; height: 19px; }
.status {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-3);
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--pd-text-4);
}
.status.on .dot { background: var(--pd-green); }
.status.on { color: var(--pd-green-text); }
.del {
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  cursor: pointer;
  font-size: 13px;
  padding: 2px 6px;
  border-radius: 6px;
  opacity: 0;
  transition: opacity 0.12s ease;
}
.mcard:hover .del { opacity: 1; }
.del:hover { color: var(--pd-red); background: var(--pd-red-soft); }
.name {
  margin: 11px 0 3px;
  font-size: calc(15px * var(--pd-font-scale));
  font-weight: 700;
  color: var(--pd-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.meta { font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-3); }
.foot {
  margin-top: 12px;
  padding-top: 9px;
  border-top: 1px solid var(--pd-border-soft);
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
}
.enter {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--pd-accent-text);
  opacity: 0;
  transition: opacity 0.12s ease;
}
.mcard:hover .enter { opacity: 1; }
</style>
