<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * 设置中心「云同步」页（由 App.vue 经 #sync 插槽注入 SettingsView）。
 * 与旧侧栏弹层同一组 Tauri 命令：sync_status / sync_configure / sync_disable。
 */
const serverUrl = ref("http://localhost:8080");
const email = ref("");
const password = ref("");
const accessToken = ref("");
const status = ref<any>(null);
const notice = ref<string | null>(null);
const noticeKind = ref<"ok" | "err">("ok");
const busy = ref(false);

let timer: ReturnType<typeof setInterval> | undefined;

async function refresh(): Promise<void> {
  try {
    status.value = await invoke("sync_status");
    if (status.value.server_url) serverUrl.value = status.value.server_url;
    if (status.value.email) email.value = status.value.email;
  } catch {
    status.value = null;
  }
}
onMounted(() => {
  void refresh();
  timer = setInterval(() => void refresh(), 5000);
});
onUnmounted(() => {
  if (timer) clearInterval(timer);
});

function flash(msg: string, kind: "ok" | "err" = "ok"): void {
  notice.value = msg;
  noticeKind.value = kind;
  setTimeout(() => (notice.value = null), 3000);
}

async function save(): Promise<void> {
  notice.value = null;
  busy.value = true;
  try {
    await invoke("sync_configure", {
      body: {
        server_url: serverUrl.value,
        email: email.value,
        password: password.value,
        token: accessToken.value,
      },
    });
    password.value = "";
    accessToken.value = "";
    flash("已连接并开启同步");
    await refresh();
  } catch (err) {
    flash(String(err), "err");
  } finally {
    busy.value = false;
  }
}

async function disable(): Promise<void> {
  busy.value = true;
  try {
    await invoke("sync_disable");
    flash("同步已关闭");
    await refresh();
  } catch (err) {
    flash(String(err), "err");
  } finally {
    busy.value = false;
  }
}

const stateLabel = (s: any): string =>
  !s?.enabled ? "未开启" : s.connected ? "已连接" : `连接中/失败${s.error ? "：" + s.error : ""}`;
</script>

<template>
  <div class="sync-page">
    <div class="group">
      <div class="row">
        <div class="row-text">
          <b>
            <span class="dot" :class="{ on: status?.connected }"></span>
            {{ stateLabel(status) }}
          </b>
          <span v-if="status?.machine_id" class="mono">本机 ID：{{ status.machine_id }}</span>
          <span v-else>尚未连接同步服务器</span>
        </div>
        <button v-if="status?.enabled" class="danger-btn" :disabled="busy" @click="disable">关闭同步</button>
      </div>
    </div>

    <h3 class="grp-title">账号</h3>
    <div class="group form">
      <div class="field">
        <label>服务器地址</label>
        <input v-model="serverUrl" placeholder="http://localhost:8080" spellcheck="false" />
      </div>
      <div class="field">
        <label>邮箱</label>
        <input v-model="email" placeholder="you@example.com" type="email" autocomplete="username" spellcheck="false" />
      </div>
      <div class="field">
        <label>访问令牌（推荐，免密且不过期）</label>
        <input
          v-model="accessToken"
          placeholder="在 Web 端「访问令牌」页创建，粘贴到这里（pd_ 开头）"
          type="password"
          autocomplete="off"
          spellcheck="false"
        />
      </div>
      <div class="field">
        <label>密码（与令牌二选一）</label>
        <input
          v-model="password"
          placeholder="登录（已保存凭据后可留空）"
          type="password"
          autocomplete="current-password"
        />
      </div>
      <div class="actions">
        <button class="primary-btn" :disabled="busy" @click="save">连接并同步</button>
      </div>
      <div v-if="notice" class="notice" :class="{ err: noticeKind === 'err' }">{{ notice }}</div>
      <p class="hint">开启后：本机所有会话实时上云；Web 端可查看并远程控制（发消息 / 停止）。凭据仅保存在本机 sync.json。</p>
    </div>
  </div>
</template>

<style scoped>
/* 插槽内容拿不到 SettingsView 的 scoped 样式，这里自带一份同款（group/row/grp-title）；
   外层宽度由 SettingsView 的居中内容列约束 */
.sync-page { max-width: 720px; }
.grp-title {
  margin: 26px 0 10px;
  font-size: calc(13.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text-2);
}
.group {
  margin-top: 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border-soft);
  border-radius: 12px;
  padding: 4px 16px;
}
.row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 0;
}
.row-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.row-text b { font-size: calc(13.5px * var(--pd-font-scale)); font-weight: 600; color: var(--pd-text); }
.row-text span { font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-3); line-height: 1.5; word-break: break-all; }
.dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--pd-text-4);
  margin-right: 8px;
  vertical-align: 1px;
}
.dot.on { background: var(--pd-green); }
.mono { font-family: var(--pd-mono); }
.danger-btn {
  background: var(--pd-red-soft);
  color: var(--pd-red-text);
  border: none;
  border-radius: 8px;
  padding: 6px 12px;
  font-size: calc(12.5px * var(--pd-font-scale));
  cursor: pointer;
  flex: none;
}
.danger-btn:hover { filter: brightness(1.1); }
.danger-btn:disabled { opacity: 0.6; cursor: default; }
.form { padding: 16px; }
.field { margin-bottom: 14px; }
.field:last-of-type { margin-bottom: 0; }
.field label {
  display: block;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-text-2);
  margin-bottom: 6px;
}
.field input {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  padding: 8px 12px;
}
.field input:focus { outline: none; border-color: var(--pd-accent); }
.field input::placeholder { color: var(--pd-text-4); }
.actions { margin-top: 16px; }
.primary-btn {
  background: var(--pd-text);
  color: var(--pd-bg);
  border: none;
  border-radius: 9px;
  padding: 8px 16px;
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
}
.primary-btn:hover { background: var(--pd-accent); color: #1a1a1a; }
.primary-btn:disabled { opacity: 0.6; cursor: default; }
.notice {
  margin-top: 12px;
  font-size: calc(12.5px * var(--pd-font-scale));
  color: var(--pd-green-text);
  background: var(--pd-green-soft);
  border-radius: 8px;
  padding: 8px 12px;
}
.notice.err { color: var(--pd-red-text); background: var(--pd-red-soft); }
.hint {
  margin: 12px 0 0;
  font-size: calc(11.5px * var(--pd-font-scale));
  color: var(--pd-text-4);
  line-height: 1.6;
}
</style>
