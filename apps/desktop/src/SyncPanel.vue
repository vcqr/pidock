<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const open = ref(false);
const serverUrl = ref("http://localhost:8080");
const email = ref("");
const password = ref("");
const status = ref<any>(null);
const notice = ref<string | null>(null);

async function refresh(): Promise<void> {
  try {
    status.value = await invoke("sync_status");
    if (status.value.server_url) serverUrl.value = status.value.server_url;
    if (status.value.email) email.value = status.value.email;
  } catch (err) {
    status.value = null;
  }
}
onMounted(() => {
  refresh();
  // 输入卡片项目菜单里的「远程连接」：唤起云同步面板
  window.addEventListener("pidock:open-sync", (() => {
    open.value = true;
    void refresh();
  }) as EventListener);
  setInterval(refresh, 5000);
});

async function save(): Promise<void> {
  notice.value = null;
  try {
    await invoke("sync_configure", {
      body: { server_url: serverUrl.value, email: email.value, password: password.value },
    });
    password.value = "";
    notice.value = "已连接并开启同步";
    await refresh();
  } catch (err) {
    notice.value = String(err);
  }
}

async function disable(): Promise<void> {
  await invoke("sync_disable");
  notice.value = "同步已关闭";
  await refresh();
}

const stateLabel = (s: any): string =>
  !s?.enabled ? "未开启" : s.connected ? "已连接" : `连接中/失败${s.error ? "：" + s.error : ""}`;

const cloudIcon = ["M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"];
</script>

<template>
  <div class="sync-wrap">
    <button
      class="sync-btn"
      :class="{ on: status?.connected }"
      :title="`云同步：${stateLabel(status)}`"
      @click="open = !open"
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
        <path :d="cloudIcon[0]" />
      </svg>
    </button>
    <div v-if="open" class="panel">
      <div class="status">
        <span class="dot" :class="{ on: status?.connected }" />
        {{ stateLabel(status) }}
        <span v-if="status?.machine_id" class="mid">机器: {{ status.machine_id.slice(0, 8) }}…</span>
      </div>
      <label>Server URL<input v-model="serverUrl" placeholder="http://localhost:8080" /></label>
      <label>邮箱<input v-model="email" placeholder="you@example.com" /></label>
      <label>密码<input v-model="password" type="password" placeholder="登录（已保存 token 后可留空）" /></label>
      <div class="actions">
        <button class="primary" @click="save">连接并同步</button>
        <button v-if="status?.enabled" class="danger" @click="disable">关闭同步</button>
      </div>
      <div v-if="notice" class="notice">{{ notice }}</div>
      <p class="hint">开启后：本机所有会话实时上云；Web 端可查看并远程控制（发消息/停止）。</p>
    </div>
  </div>
</template>

<style scoped>
.sync-wrap { position: relative; }
.sync-btn {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  background: none;
  border: none;
  border-radius: 7px;
  color: var(--pd-accent);
  cursor: pointer;
  padding: 0;
}
.sync-btn:hover { background: var(--pd-bg-hover); }
.sync-btn.on { color: var(--pd-green); }
.panel {
  position: absolute;
  right: -6px;
  bottom: calc(100% + 10px);
  width: 300px;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 12px;
  z-index: 80;
  box-shadow: var(--pd-shadow);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.status { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--pd-text-2); }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--pd-text-4); }
.dot.on { background: var(--pd-green); }
.mid { margin-left: auto; font-size: 11px; color: var(--pd-text-4); }
label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--pd-text-2); }
input {
  background: var(--pd-bg-card);
  color: var(--pd-text);
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  padding: 6px 8px;
  font-size: 12.5px;
}
input:focus { outline: none; border-color: var(--pd-accent); }
.actions { display: flex; gap: 6px; }
button.primary {
  background: var(--pd-accent);
  color: #1a1a1a;
  font-weight: 600;
}
button.danger { background: var(--pd-red-soft); color: var(--pd-red-text); }
.panel .actions button {
  border: none;
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 12.5px;
  cursor: pointer;
}
.notice {
  font-size: 12px;
  color: var(--pd-green-text);
  background: var(--pd-green-soft);
  border-radius: 8px;
  padding: 6px 8px;
}
.hint { margin: 0; font-size: 11px; color: var(--pd-text-4); line-height: 1.5; }
</style>
