import type { DataBus, ReplayEvent } from "@pidock/ui";
import type { Envelope } from "@pidock/protocol";
import { ipc, onPidockEvent, isTauri } from "./ipc";

/**
 * Desktop DataBus: 统一 IPC（Tauri IPC 或 webhost WS）-> Rust core -> pi-host。
 * 两种传输共用同一命令面（host_request / automation_request），消息层完全一致。
 * Delta 级流式（完整打字机效果）。
 */
export function createBus(): DataBus {
  return {
    transport: isTauri ? "本地 pi-host" : "远程 pi-host（Web 接入）",

    async request(method: string, params?: unknown): Promise<any> {
      // 自动化（定时任务）由 Rust 调度器处理，不经过 pi-host
      if (method.startsWith("automation.")) {
        return await ipc("automation_request", { method, params: params ?? {} });
      }
      return await ipc("host_request", { method, params: params ?? {} });
    },

    async onEvent(handler: (event: Envelope) => void): Promise<() => void> {
      return onPidockEvent(handler);
    },

    async loadHistory(sessionId: string, afterSeq?: number): Promise<ReplayEvent[]> {
      const params: Record<string, unknown> = { session_id: sessionId };
      if (afterSeq !== undefined) params.after_seq = afterSeq;
      const result = await ipc<{ events: ReplayEvent[] }>("host_request", {
        method: "session.events",
        params,
      });
      return result.events ?? [];
    },

    async loadAttachment(attachmentId: string): Promise<string> {
      const result = await ipc<{ text: string }>("host_request", {
        method: "attachment.get",
        params: { attachment_id: attachmentId },
      });
      return result.text;
    },
  };
}
