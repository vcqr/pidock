import { createApp, h, ref } from "vue";
import ConfirmDialog from "./components/ConfirmDialog.vue";

/**
 * 主题化确认弹窗（替代浏览器原生 window.confirm）：
 *   if (!(await appConfirm({ title: "删除专家「x」？", danger: true }))) return;
 * discrete 挂载到 body，样式跟随全局 --pd-* 主题变量。
 */
export interface ConfirmOptions {
  /** 标题（如「移除项目 xxx？」）；可省略只显示正文 */
  title?: string;
  /** 正文，保留换行 */
  message?: string;
  confirmText?: string;
  cancelText?: string;
  /** 危险操作：确认按钮红色 */
  danger?: boolean;
}

export function appConfirm(opts: ConfirmOptions | string): Promise<boolean> {
  const o = typeof opts === "string" ? { message: opts } : opts;
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const visible = ref(true);
    let settled = false;
    const close = (v: boolean): void => {
      if (settled) return;
      settled = true;
      visible.value = false;
      resolve(v);
      // 等淡出动画（无动画也保证卸载）
      setTimeout(() => {
        app.unmount();
        host.remove();
      }, 150);
    };
    const app = createApp({
      render: () =>
        h(ConfirmDialog, {
          ...(o.title ? { title: o.title } : {}),
          ...(o.message ? { message: o.message } : {}),
          ...(o.confirmText ? { confirmText: o.confirmText } : {}),
          ...(o.cancelText ? { cancelText: o.cancelText } : {}),
          ...(o.danger ? { danger: true } : {}),
          style: visible.value ? "" : "opacity:0;transition:opacity .15s",
          onConfirm: () => close(true),
          onCancel: () => close(false),
        }),
    });
    app.mount(host);
  });
}
