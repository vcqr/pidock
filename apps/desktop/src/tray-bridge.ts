import { watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AgentStore } from "@pidock/ui";

/**
 * 桌面托盘桥：把会话列表（标题/状态/待处理审批）推给 Rust 侧，
 * 作为托盘菜单、图标角标与 tooltip 的数据源。事件驱动的 watch
 * 不受窗口隐藏时定时器节流影响（关键通知另有 Rust 事件 watcher 兜底）。
 */
export function startTrayBridge(store: AgentStore): void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const push = (): void => {
    if (timer) return;
    timer = setTimeout(() => {
      timer = undefined;
      const sessions = store.sessions.map((s) => ({
        id: s.session_id,
        title: s.name || baseName(s.file) || "未命名会话",
        state: s.state,
        pending: store.pendingBySession[s.session_id]?.kind ?? null,
      }));
      void invoke("tray_update", { sessions }).catch(() => {
        /* 托盘初始化前静默 */
      });
    }, 150);
  };
  watch(() => [store.sessions, store.pendingBySession], push, { deep: true, immediate: true });
}

function baseName(file: string): string {
  return file.split(/[\\/]/).pop() ?? "";
}
