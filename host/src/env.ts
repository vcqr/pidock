// GUI（macOS Dock/Finder → launchd）拉起的进程只带最小 PATH
// （/usr/bin:/bin:/usr/sbin:/sbin）。pi 会话跑在本进程内，扩展（agent-browser）
// 与 MCP server spawn 子二进制时继承的就是这份 PATH，装在用户 shell PATH 里的
// 工具直接 ENOENT；SDK 只给 shell 工具补了 agent bin（其 utils/shell.js
// getShellEnv），其余子进程无覆盖。启动时从用户登录 shell 重取真实 PATH 合并。
// 放在 host 而非桌面壳层：desktop/webhost/远端节点共用同一个 pidock-host，
// 一处覆盖所有启动方式。

import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

const PROBE_TIMEOUT_MS = 4000;

let ready: Promise<void> | null = null;

/** 幂等；dispatch 处理请求前必须 await（扩展/MCP spawn 时 PATH 必须已就位） */
export function prepareEnv(): Promise<void> {
  ready ??= fixupPath();
  return ready;
}

async function fixupPath(): Promise<void> {
  const pathKey = Object.keys(process.env).find((k) => k.toLowerCase() === "path") ?? "PATH";
  const current = process.env[pathKey] ?? "";
  const entries = current.split(delimiter).filter(Boolean);
  const merged: string[] = [];
  const push = (dir: string): void => {
    if (dir && !merged.includes(dir)) merged.push(dir);
  };
  // pi 自身 bin 目录最优先（与 getShellEnv 行为一致）
  push(join(getAgentDir(), "bin"));
  const home = homedir();
  // launchd 最小 PATH 的特征：条目少且不含用户目录；终端启动的 PATH 已完整，
  // 不必再花一次登录 shell 的开销。Windows GUI 进程从注册表拿全量 PATH，跳过
  const looksMinimal =
    entries.length <= 6 && !entries.some((e) => e === home || e.startsWith(home + "/"));
  if (process.platform !== "win32" && looksMinimal) {
    const shellPath = await readLoginShellPath();
    // 探测输出可能混入噪声（fish 的 $PATH 是空格分隔列表等），只收合法目录
    for (const dir of shellPath.split(delimiter)) {
      if (dir.startsWith("/") && !/\s/.test(dir)) push(dir);
    }
  }
  for (const dir of entries) push(dir);
  const next = merged.join(delimiter);
  if (next !== current) {
    process.env[pathKey] = next;
    process.stderr.write(`[host] PATH fixed up: ${entries.length} -> ${merged.length} entries\n`);
  }
}

function readLoginShellPath(): Promise<string> {
  return new Promise((resolve) => {
    const shell = process.env.SHELL || (process.platform === "darwin" ? "/bin/zsh" : "/bin/bash");
    // -i 才会加载 .zshrc/.bashrc（用户 PATH 多数追加在这里）；交互式输出可能带
    // 插件噪声，用标记定位 printf 写出的 PATH 片段
    const tag = `__pidock_path_${process.pid}__`;
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(shell, ["-ilc", `printf '%s' "${tag}$PATH"`], {
        env: {
          HOME: process.env.HOME ?? homedir(),
          SHELL: shell,
          USER: process.env.USER ?? "",
          LOGNAME: process.env.LOGNAME ?? "",
          TERM: "dumb",
          PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
        },
        stdio: ["ignore", "pipe", "ignore"],
      });
    } catch {
      resolve("");
      return;
    }
    let out = "";
    child.stdout?.on("data", (chunk: Buffer) => {
      out += chunk.toString("utf8");
    });
    const timer = setTimeout(() => child.kill("SIGKILL"), PROBE_TIMEOUT_MS);
    child.on("error", () => {
      clearTimeout(timer);
      resolve("");
    });
    child.on("close", () => {
      clearTimeout(timer);
      const start = out.indexOf(tag);
      const line = start >= 0 ? (out.slice(start + tag.length).split(/\r?\n/, 1)[0] ?? "") : "";
      resolve(line.trim());
    });
  });
}
