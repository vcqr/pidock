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
