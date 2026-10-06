<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";

/**
 * 主题化确认对话框（替代浏览器原生 window.confirm）。
 * 一般不直接使用：从 "../confirm.js" 导入 appConfirm()（Promise 化、discrete 挂载）。
 */
defineProps<{
  /** 标题（如「移除项目 xxx？」）；可省略只显示正文 */
  title?: string;
  /** 正文，保留换行 */
  message?: string;
  confirmText?: string;
  cancelText?: string;
  /** 危险操作：确认按钮红色 */
  danger?: boolean;
}>();
const emit = defineEmits<{ confirm: []; cancel: [] }>();

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    emit("cancel");
  } else if (e.key === "Enter") {
    e.preventDefault();
    emit("confirm");
  }
}
onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="confirm-mask" @click.self="emit('cancel')">
    <div class="confirm-box" role="alertdialog" aria-modal="true">
      <div v-if="title" class="confirm-title">{{ title }}</div>
      <div v-if="message" class="confirm-msg">{{ message }}</div>
      <div class="confirm-btns">
        <button class="c-btn cancel" @click="emit('cancel')">{{ cancelText ?? "取消" }}</button>
        <button class="c-btn ok" :class="{ danger }" @click="emit('confirm')">{{ confirmText ?? "确定" }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.confirm-mask {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: rgba(0, 0, 0, 0.5);
  display: grid;
  place-items: center;
}
.confirm-box {
  width: 420px;
  max-width: 90vw;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  box-shadow: var(--pd-shadow);
  padding: 18px 20px 16px;
}
.confirm-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--pd-text);
  margin-bottom: 8px;
}
.confirm-msg {
  font-size: 12.5px;
  line-height: 1.65;
  color: var(--pd-text-2);
  white-space: pre-line;
  word-break: break-word;
}
.confirm-btns {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.c-btn {
  padding: 6px 18px;
  border-radius: 8px;
  font-size: 12.5px;
  cursor: pointer;
  border: 1px solid var(--pd-border);
  background: none;
  color: var(--pd-text-2);
}
.c-btn.cancel:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.c-btn.ok {
  background: var(--pd-accent);
  border-color: var(--pd-accent);
  color: #1a1a1a;
  font-weight: 600;
}
.c-btn.ok:hover { background: var(--pd-accent-hover); }
.c-btn.ok.danger {
  background: var(--pd-red);
  border-color: var(--pd-red);
  color: #fff;
}
.c-btn.ok.danger:hover { filter: brightness(1.1); }
</style>
