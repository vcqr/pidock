import type { Envelope } from "@pidock/protocol";

/**
 * Transport-agnostic data source for the shared UI.
 *
 * - Desktop: Tauri IPC -> local pi-host stdio (delta-level streaming).
 * - Web:     HTTP (history) + WebSocket (live, message-level).
 *
 * The UI components only know this interface — that is what makes the web UI
 * "identical" to the desktop one.
 */
export interface DataBus {
  /** RPC to the agent host (desktop) or server relay (web). */
  request(method: string, params?: unknown): Promise<any>;
  /** Live event stream. Returns an unsubscribe function. */
  onEvent(handler: (event: Envelope) => void): Promise<() => void>;
  /** Persisted history replay (entry-anchored events, ordered by seq). */
  loadHistory(sessionId: string, afterSeq?: number): Promise<ReplayEvent[]>;
  /** Full text of a truncated attachment (content-addressed by sha256). */
  loadAttachment(attachmentId: string): Promise<string>;
  /** Human-readable transport label for the status bar. */
  readonly transport: string;
}

import type { InjectionKey } from "vue";
/** optional provide/inject handle for ToolCard full-text viewing */
export const ATTACHMENT_LOADER: InjectionKey<(id: string) => Promise<string>> = Symbol("pidock.attachmentLoader");
/** optional provide/inject handle for the native folder picker (desktop only); resolves null on cancel */
export const FOLDER_PICKER: InjectionKey<() => Promise<string | null>> = Symbol("pidock.folderPicker");
/** optional provide/inject handle for revealing a path in the OS file manager (desktop only) */
export const REVEAL_PATH: InjectionKey<(path: string) => Promise<void>> = Symbol("pidock.revealPath");

export interface ReplayEvent {
  seq: number;
  ts: string;
  kind: string;
  payload: any;
}
