// 默认工作空间：~/.pidock。桌面壳启动时初始化根目录；host 侧缺省项目的
// 会话落在其下 <当日日期>/<会话id>/，产出物按会话归档。desktop/webhost/
// 远端节点共用同一个 pidock-host，一处定义所有启动方式。

import { homedir } from "node:os";
import { join } from "node:path";

/** 默认工作空间根目录：$HOME/.pidock */
export function defaultWorkspaceRoot(): string {
  return join(homedir(), ".pidock");
}

/** 本地时区的当日日期段，如 2026-10-13 */
function todaySeg(now: Date): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** 会话目录路径：root/<当日日期>/<会话id>（只算路径，不落盘） */
export function sessionDirUnder(root: string, sessionId: string, now: Date = new Date()): string {
  return join(root, todaySeg(now), sessionId);
}

/** cwd 是否为托管会话目录：位于默认工作空间内部（根目录本身是任务工作区，不算） */
export function isManagedSessionDir(cwd: string): boolean {
  const root = defaultWorkspaceRoot();
  return cwd.startsWith(`${root}/`) || cwd.startsWith(`${root}\\`);
}

/**
 * 缺省项目会话的工作目录：~/.pidock/<当日日期>/<会话id>/。
 * 只计算路径不落盘——目录由首个获准执行的工具调用惰性创建
 * （pool.ts 的 tool_call 钩子），纯问答对话不会留下空目录。
 */
export function defaultSessionDir(sessionId: string, now: Date = new Date(), root: string = defaultWorkspaceRoot()): string {
  return sessionDirUnder(root, sessionId, now);
}

/** 多媒体扩展名 → MIME（预览用）；未收录的回落 application/octet-stream */
const MEDIA_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  ico: "image/x-icon",
  bmp: "image/bmp",
  avif: "image/avif",
  tiff: "image/tiff",
  mp4: "video/mp4",
  m4v: "video/x-m4v",
  mov: "video/quicktime",
  webm: "video/webm",
  mkv: "video/x-matroska",
  avi: "video/x-msvideo",
  mp3: "audio/mpeg",
  wav: "audio/wav",
  ogg: "audio/ogg",
  m4a: "audio/mp4",
  flac: "audio/flac",
  aac: "audio/aac",
  opus: "audio/opus",
};

/** 按扩展名推断预览 MIME；非多媒体文件返回 null（走文本预览） */
export function mediaMimeOf(path: string): string | null {
  const base = path.replace(/\\/g, "/").split("/").pop() ?? "";
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return null;
  return MEDIA_MIME[base.slice(dot + 1).toLowerCase()] ?? null;
}
