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

import type { InjectionKey, Ref } from "vue";
/** optional provide/inject handle for ToolCard full-text viewing */
export const ATTACHMENT_LOADER: InjectionKey<(id: string) => Promise<string>> = Symbol("pidock.attachmentLoader");
/** optional provide/inject handle for the native folder picker (desktop only); resolves null on cancel */
export const FOLDER_PICKER: InjectionKey<() => Promise<string | null>> = Symbol("pidock.folderPicker");
/** optional provide/inject handle for the native single-file picker (desktop only); resolves null on cancel */
export const FILE_PICKER: InjectionKey<(kind?: "install" | "image") => Promise<string | null>> = Symbol("pidock.filePicker");
/** optional provide/inject handle for revealing a path in the OS file manager (desktop only) */
export const REVEAL_PATH: InjectionKey<(path: string) => Promise<void>> = Symbol("pidock.revealPath");

/** git 分支信息与切换（CoreCtx 命令面，桌面与 webhost 都提供）；项目 chip 的分支选择器用 */
export interface GitApi {
  /** 仓库检测 + 当前分支（非仓库时 is_repo = false） */
  info(cwd: string): Promise<{ is_repo: boolean; branch: string | null; root: string | null; detached: boolean }>;
  /** 本地分支列表（当前分支排最前） */
  branches(cwd: string): Promise<{ current: string | null; branches: Array<{ name: string; current: boolean }> }>;
  /** 检出分支；create = 创建并检出 */
  checkout(cwd: string, branch: string, create?: boolean): Promise<{ ok: boolean; branch: string }>;
}
export const GIT_API: InjectionKey<GitApi> = Symbol("pidock.gitApi");

/** 服务端目录列举（fs_list）：「打开文件夹」与文件选择弹层的数据源，桌面与 webhost 都提供 */
export interface FsListing {
  path: string;
  parent: string | null;
  /** 目录在前；dir 标志区分目录与文件 */
  entries: Array<{ name: string; path: string; dir: boolean }>;
  /** 常用目录快捷入口（桌面/下载/图片/文档），不存在时为 null */
  specials?: Partial<Record<"desktop" | "downloads" | "pictures" | "documents", string | null>>;
}
export const FS_LIST: InjectionKey<(path?: string) => Promise<FsListing>> = Symbol("pidock.fsList");

/** 无边框窗口控制（桌面端提供）；全屏页面（如设置中心）盖住标题栏时用它补齐窗口按钮 */
export interface WindowControls {
  minimize(): void;
  toggleMaximize(): void;
  close(): void;
  /** 当前是否最大化（最大化/还原图标切换） */
  isMax: Ref<boolean>;
}
export const WINDOW_CONTROLS: InjectionKey<WindowControls> = Symbol("pidock.windowControls");

export interface ReplayEvent {
  seq: number;
  ts: string;
  kind: string;
  payload: any;
}
