/**
 * PiDock 测试插件 — 覆盖 pi 扩展 API 的主要能力面：
 *
 * 1. 自定义工具   pi.registerTool()  → LLM 可调用 pidock_echo
 * 2. 自定义命令   pi.registerCommand() → /pidock 查看统计
 * 3. 生命周期事件 pi.on("session_start") → 通知 + 会话持久化 (pi.appendEntry)
 * 4. 工具调用拦截 pi.on("tool_call") → 计数 + 权限门示例（拦截 rm -rf / 或 ~）
 *
 * 安装位置（自动发现）：~/.pi/agent/extensions/pidock-test.ts（全局）
 *                      或 <项目>/.pi/extensions/pidock-test.ts（项目级）
 * 桌面端「插件」页可查看并启停（停用写入 settings.json 的排除规则，对新会话生效）。
 *
 * 冒烟验证：设置环境变量 PIDOCK_EXT_MARKER 时，插件加载即写入标记文件；
 * 日常使用不设置该变量，不产生任何额外文件。
 */
import { appendFileSync } from "node:fs";
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

// ---- 冒烟钩子：仅在显式设置标记路径时落盘 ----
if (process.env.PIDOCK_EXT_MARKER) {
  appendFileSync(process.env.PIDOCK_EXT_MARKER, "pidock-test loaded\n");
}

/** 会话内统计（内存态；跨重启的持久化演示见 session_start 里的 appendEntry） */
const stats = { sessionStarts: 0, toolCalls: 0, blocked: 0 };

/** TUI 通知包一层 try/catch：无头 RPC 场景（PiDock host）下 UI 能力可能不可用，测试插件不应干扰会话 */
function notify(ctx: any, message: string): void {
  try {
    ctx?.ui?.notify?.(message, "info");
  } catch {
    // ignore — headless context
  }
}

// ---- 1. 自定义工具：pidock_echo ----
const echoTool = defineTool({
  name: "pidock_echo",
  label: "PiDock Echo",
  description:
    "PiDock 测试插件提供的回显工具：原样返回输入文本（可选转大写），并附带本会话的工具调用统计。",
  parameters: Type.Object({
    text: Type.String({ description: "要回显的文本" }),
    upper: Type.Optional(Type.Boolean({ description: "是否将文本转为大写" })),
  }),
  async execute(_toolCallId, params) {
    const body = params.upper ? params.text.toUpperCase() : params.text;
    const text = `${body}\n\n[pidock-test] 本会话：会话启动 ${stats.sessionStarts} 次 · 工具调用 ${stats.toolCalls} 次 · 拦截 ${stats.blocked} 次`;
    return { content: [{ type: "text", text }], details: {} };
  },
});

export default function (pi: ExtensionAPI) {
  // ---- 3. 生命周期：会话启动 ----
  pi.on("session_start", async (_event, ctx) => {
    stats.sessionStarts++;
    notify(ctx, "PiDock 测试插件已加载，试试 /pidock 或让模型调用 pidock_echo");

    // 会话持久化演示：自定义条目写入 session JSONL，不进入 LLM 上下文，
    // 重载后可通过 ctx.sessionManager.getEntries() 按 customType 恢复
    try {
      pi.appendEntry("pidock-test", {
        startedAt: new Date().toISOString(),
        sessionStarts: stats.sessionStarts,
      });
    } catch {
      // appendEntry 失败不应影响会话启动
    }
  });

  // ---- 4. 工具调用拦截：计数 + 危险命令权限门 ----
  pi.on("tool_call", async (event) => {
    stats.toolCalls++;
    if (event.toolName === "bash") {
      const command = (event.input as { command?: string } | undefined)?.command ?? "";
      if (/rm\s+(-[a-z]*r[a-z]*f|--recursive)\s+[/~]/.test(command)) {
        stats.blocked++;
        // 返回 block 即拒绝本次工具调用，reason 会展示给模型与用户
        return { block: true, reason: "[pidock-test] 已拦截危险的 rm -rf 命令（测试插件的权限门示例）" };
      }
    }
    return undefined;
  });

  // ---- 2. 注册工具与命令 ----
  pi.registerTool(echoTool);

  pi.registerCommand("pidock", {
    description: "显示 PiDock 测试插件的统计信息",
    handler: async (_args, ctx) => {
      notify(
        ctx,
        `pidock-test：会话启动 ${stats.sessionStarts} 次 · 工具调用 ${stats.toolCalls} 次 · 拦截 ${stats.blocked} 次`,
      );
    },
  });
}
