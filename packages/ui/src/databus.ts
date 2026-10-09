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
  /** 进入/退出全屏（macOS 绿灯默认行为；Windows 无调用方） */
  toggleFullscreen(): void;
  /** 平铺到屏幕工作区左/右半边（macOS 绿灯悬停菜单的 Move & Resize） */
  tile(side: "left" | "right"): void;
  close(): void;
  /** 当前是否最大化（最大化/还原图标切换） */
  isMax: Ref<boolean>;
  /** 当前是否全屏（macOS 绿灯标题提示） */
  isFullscreen: Ref<boolean>;
}
export const WINDOW_CONTROLS: InjectionKey<WindowControls> = Symbol("pidock.windowControls");

let dblclickGuardInstalled = false;
/**
 * 安装全局守卫：标题栏（data-tauri-drag-region="deep"）上的原生多次点击（双击/三击）
 * 不再交给 tauri 内建脚本处理（内建路径在 macOS 依赖 mouseup 像素级同位、且首次按下
 * 已进入原生拖拽会话，触控板轻点也不产生 detail=2，双击经常不触发），统一由标题栏的
 * @dblclick → onTitlebarDblclick 处理；拦截时同时阻止默认行为——tauri 内建脚本只在
 * 非 macOS 路径做 preventDefault，macOS 的第二次按下没有拦截，WKWebView 会从那里
 * 启动双击选词/拖选，把标题栏下方的主界面内容选亮。桌面端 App.vue 挂载时调用一次。
 */
export function installTauriDblclickGuard(): void {
  if (dblclickGuardInstalled || typeof document === "undefined") return;
  dblclickGuardInstalled = true;
  document.addEventListener(
    "mousedown",
    (e) => {
      if (e.detail >= 2 && (e.target as HTMLElement | null)?.closest?.('[data-tauri-drag-region="deep"]')) {
        e.preventDefault(); // 阻止双击选词/三击选段等默认行为
        e.stopPropagation(); // capture 阶段拦下，tauri 的 document 冒泡监听不再收到
      }
    },
    true,
  );
}

/**
 * 标题栏 @dblclick 处理：点在可交互元素（按钮/链接/输入类，或显式 data-tauri-drag-region="false"）
 * 上不触发最大化，其余区域（含状态胶囊、空白）等同原生标题栏双击缩放；
 * 并清掉可能已产生的选区（按下位置漂移落进内容区时浏览器已开始选择）。
 */
export function onTitlebarDblclick(e: MouseEvent, toggle: () => void): void {
  const el = e.target as HTMLElement | null;
  if (el?.closest("button, a, input, select, textarea, [data-tauri-drag-region='false']")) return;
  e.preventDefault();
  window.getSelection()?.removeAllRanges();
  toggle();
}

/**
 * 运行平台是否 macOS。用于窗口控制按钮按平台切换风格（macOS 红绿灯 / Windows 方块按钮），
 * 只在 Tauri 壳里生效；WKWebView 的 navigator.platform 为 "MacIntel"（iPad 无桌面壳，不涉及）。
 */
export const IS_MAC = typeof navigator !== "undefined" && /^Mac/i.test(navigator.platform || navigator.userAgent);

export interface ReplayEvent {
  seq: number;
  ts: string;
  kind: string;
  payload: any;
}
