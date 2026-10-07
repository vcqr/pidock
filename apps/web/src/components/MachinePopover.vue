<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { osMeta, relTime, type MachineUi } from "../machine.js";

const props = defineProps<{
  machine: MachineUi;
  sessionCount: number | null;
  homeDir?: string;
}>();
const emit = defineEmits<{ close: [] }>();

const meta = computed(() => osMeta(props.machine.os));
const root = ref<HTMLElement | null>(null);
const copied = ref(false);
let copyTimer: ReturnType<typeof setTimeout> | undefined;

function onDocMousedown(e: MouseEvent): void {
  if (root.value && !root.value.contains(e.target as Node)) emit("close");
}
onMounted(() => document.addEventListener("mousedown", onDocMousedown));
onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onDocMousedown);
  if (copyTimer) clearTimeout(copyTimer);
});

async function copyId(): Promise<void> {
  const id = props.machine.machine_id;
  try {
    // http 局域网部署下 navigator.clipboard 不可用，退回 execCommand
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(id);
    } else {
      const ta = document.createElement("textarea");
      ta.value = id;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    copied.value = true;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied.value = false), 1500);
  } catch {
    /* 剪贴板不可用时静默 */
  }
}
</script>

<template>
  <div ref="root" class="mpop">
    <div class="head">
      <span class="os-chip" :style="{ color: meta.color, background: meta.color + '1f' }">
        <svg viewBox="0 0 24 24" fill="currentColor"><path :d="meta.path" /></svg>
      </span>
      <b class="host">{{ machine.hostname || machine.machine_id.slice(0, 8) }}</b>
      <span class="status" :class="{ on: machine.online }">
        <i class="dot" />{{ machine.online ? "在线" : "离线" }}
      </span>
    </div>
    <dl>
      <dt>本机 IP</dt>
      <dd class="mono">{{ machine.local_ip || "—" }}</dd>
      <dt>操作系统</dt>
      <dd>{{ meta.name }}</dd>
      <dt>Agent 版本</dt>
      <dd>{{ machine.version ? `v${machine.version}` : "—" }}</dd>
      <dt>主目录</dt>
      <dd class="mono">{{ homeDir || "—" }}</dd>
      <dt>会话数</dt>
      <dd>{{ sessionCount ?? "—" }}</dd>
      <dt>最后活跃</dt>
      <dd>{{ relTime(machine.last_seen) }}</dd>
      <dt>节点 ID</dt>
      <dd class="mono id-row">
        <span class="id" :title="machine.machine_id">{{ machine.machine_id }}</span>
        <button class="copy" @click="copyId">{{ copied ? "已复制" : "复制" }}</button>
      </dd>
    </dl>
  </div>
</template>

<style scoped>
.mpop {
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  z-index: 60;
  width: 330px;
  padding: 14px 16px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  box-shadow: var(--pd-shadow);
}
.head {
  display: flex;
  align-items: center;
  gap: 9px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--pd-border-soft);
}
.os-chip {
  width: 30px;
  height: 30px;
  border-radius: 8px;
  display: grid;
  place-items: center;
  flex: none;
}
.os-chip svg { width: 17px; height: 17px; }
.host {
  color: var(--pd-text);
  font-size: calc(13.5px * var(--pd-font-scale));
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-3);
  flex: none;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--pd-text-4);
}
.status.on .dot { background: var(--pd-green); }
dl {
  margin: 10px 0 0;
  display: grid;
  grid-template-columns: 72px 1fr;
  row-gap: 8px;
  column-gap: 10px;
  font-size: calc(12px * var(--pd-font-scale));
}
dt { color: var(--pd-text-4); }
dd {
  margin: 0;
  color: var(--pd-text-2);
  overflow-wrap: anywhere;
}
.mono {
  font-family: var(--pd-mono, ui-monospace, monospace);
  font-size: calc(11px * var(--pd-font-scale));
}
.id-row { display: flex; align-items: center; gap: 6px; }
.id { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.copy {
  flex: none;
  background: transparent;
  border: 1px solid var(--pd-border);
  color: var(--pd-text-2);
  border-radius: 6px;
  font-size: calc(11px * var(--pd-font-scale));
  padding: 1px 7px;
  cursor: pointer;
}
.copy:hover {
  border-color: var(--pd-accent);
  color: var(--pd-accent-text);
}
</style>
