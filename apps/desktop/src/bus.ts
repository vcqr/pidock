import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DataBus, ReplayEvent } from "@pidock/ui";
import type { Envelope } from "@pidock/protocol";

/**
 * Desktop DataBus: Tauri IPC -> Rust supervisor -> local pi-host stdio.
 * Delta-level streaming (full typewriter effect).
 */
export function createTauriBus(): DataBus {
  return {
    transport: "本地 pi-host",

    async request(method: string, params?: unknown): Promise<any> {
      // 自动化（定时任务）由 Rust 调度器处理，不经过 pi-host
      if (method.startsWith("automation.")) {
        return await invoke("automation_request", { method, params: params ?? {} });
      }
      return await invoke("host_request", { method, params: params ?? {} });
    },

    async onEvent(handler: (event: Envelope) => void): Promise<() => void> {
      const unlisten = await listen<Envelope>("pidock:event", (ev) => handler(ev.payload));
      return unlisten;
    },

    async loadHistory(sessionId: string, afterSeq?: number): Promise<ReplayEvent[]> {
      const params: Record<string, unknown> = { session_id: sessionId };
      if (afterSeq !== undefined) params.after_seq = afterSeq;
      const result = await invoke<{ events: ReplayEvent[] }>("host_request", {
        method: "session.events",
        params,
      });
      return result.events ?? [];
    },

    async loadAttachment(attachmentId: string): Promise<string> {
      const result = await invoke<{ text: string }>("host_request", {
        method: "attachment.get",
        params: { attachment_id: attachmentId },
      });
      return result.text;
    },
  };
}
