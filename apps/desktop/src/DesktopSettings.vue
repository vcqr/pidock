<script setup lang="ts">
import { onMounted, ref } from "vue";
import { NSwitch } from "naive-ui";
import { invoke } from "@tauri-apps/api/core";

/**
 * 设置中心「桌面」页（由 App.vue 经 #pane-desktop 插槽注入 SettingsView）。
 * 对应 Rust 侧 desktop_config_get/set（%APPDATA%/app.pidock.desktop/desktop.json），
 * 与 host 的 settings.json 互不相干。
 */
const closeHide = ref(true);
const notifications = ref(true);
const autostart = ref(false);
const notice = ref<string | null>(null);
const noticeKind = ref<"ok" | "err">("ok");

let noticeTimer: ReturnType<typeof setTimeout> | undefined;
function flash(msg: string, kind: "ok" | "err" = "ok"): void {
  notice.value = msg;
  noticeKind.value = kind;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (notice.value = null), 2500);
}

onMounted(async () => {
  try {
    const cfg = await invoke<{ close_action: string; notifications: boolean; autostart: boolean }>(
      "desktop_config_get",
    );
    closeHide.value = cfg.close_action === "hide";
    notifications.value = cfg.notifications;
    autostart.value = cfg.autostart;
  } catch (err) {
    flash(String(err), "err");
  }
});

async function save(): Promise<void> {
  try {
    await invoke("desktop_config_set", {
      config: {
        close_action: closeHide.value ? "hide" : "exit",
        notifications: notifications.value,
        autostart: autostart.value,
      },
    });
  } catch (err) {
    flash(String(err), "err");
  }
}
</script>

<template>
  <div class="desktop-page">
    <div class="group">
      <div class="row">
        <div class="row-text">
          <b>关闭窗口时隐藏到托盘</b>
          <span>点 × 后 PiDock 收进系统托盘，会话继续在后台运行；退出走托盘菜单「退出 PiDock」。</span>
        </div>
        <n-switch v-model:value="closeHide" size="small" @update:value="save" />
      </div>
      <div class="row">
        <div class="row-text">
          <b>系统通知</b>
          <span>窗口隐藏或失焦时，会话等待确认/提问、定时任务结束会发系统通知；托盘菜单里也有同一开关。</span>
        </div>
        <n-switch v-model:value="notifications" size="small" @update:value="save" />
      </div>
      <div class="row">
        <div class="row-text">
          <b>开机自动启动</b>
          <span>登录系统后自动启动 PiDock 并静默收进托盘（任务计划/注册表 Run 项）。</span>
        </div>
        <n-switch v-model:value="autostart" size="small" @update:value="save" />
      </div>
    </div>
    <div v-if="notice" class="notice" :class="{ err: noticeKind === 'err' }">{{ notice }}</div>
    <p class="hint">托盘：左键单击显示/隐藏窗口，右键打开菜单（会话速览、开关与退出）。</p>
  </div>
</template>

<style scoped>
/* 插槽内容拿不到 SettingsView 的 scoped 样式，这里自带一份同款（group/row） */
.desktop-page { max-width: 720px; }
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
.row + .row { border-top: 1px solid var(--pd-border-soft); }
.row-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.row-text b { font-size: calc(13.5px * var(--pd-font-scale)); font-weight: 600; color: var(--pd-text); }
.row-text span { font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-3); line-height: 1.5; }
.notice { margin-top: 10px; font-size: calc(12.5px * var(--pd-font-scale)); color: var(--pd-green); }
.notice.err { color: var(--pd-red); }
.hint { margin: 14px 2px 0; font-size: calc(12px * var(--pd-font-scale)); color: var(--pd-text-4); }
</style>
