/**
 * MCP 服务器探测：按草稿配置（或 mcp.json 中的既有条目）临时连接一次服务器，
 * 返回 initialize 的 serverInfo 与 tools/list 清单，供设置中心预览工具 / 测试连接。
 * 会话内的正式连接与调用由 pi 的 MCP 扩展负责，这里只读不写、即连即断。
 */
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  McpAuthRequiredError,
  McpClient,
  McpHttpError,
  McpTimeoutError,
  StdioTransport,
  StreamableHttpTransport,
  type InitializeResult,
  type McpTransport,
  type Tool,
} from "@earendil-works/pi-mcp";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

const PROBE_TIMEOUT_MS = 15_000;
const MAX_TOOLS = 500;

export interface ProbeTool {
  name: string;
  description: string;
  inputSchema: unknown;
}

export interface ProbeResult {
  ok: boolean;
  /** 服务器返回 401 且无可用凭证时为 true，UI 提示去配请求头 */
  needsAuth?: boolean;
  serverInfo?: { name: string; version: string };
  tools?: ProbeTool[];
  error?: string;
}

const ENV_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * 与 pi 的 resolveConfigValue 同语义的轻量版（该函数未从 pi 包根导出）：
 * `!cmd` 执行 shell 命令取 stdout，`${VAR}`/`$VAR` 取环境变量，`$$`/`$!` 为字面转义；
 * 命令失败或变量缺失返回 undefined（与 pi 一致，pi 会在此报错，这里降级为空串）。
 */
function resolveConfigValue(raw: string): string | undefined {
  if (raw.startsWith("!")) {
    try {
      const out = execSync(raw.slice(1), {
        encoding: "utf8",
        timeout: 10_000,
        stdio: ["ignore", "pipe", "ignore"],
      });
      return out.trim() || undefined;
    } catch {
      return undefined;
    }
  }
  let out = "";
  let i = 0;
  while (i < raw.length) {
    const ch = raw[i];
    if (ch !== "$") {
      out += ch;
      i++;
      continue;
    }
    const next = raw[i + 1];
    if (next === "$" || next === "!") {
      out += next;
      i += 2;
      continue;
    }
    if (next === "{") {
      const end = raw.indexOf("}", i + 2);
      if (end < 0) {
        out += "$";
        i++;
        continue;
      }
      const name = raw.slice(i + 2, end);
      if (ENV_NAME_RE.test(name)) {
        const v = process.env[name];
        if (v === undefined) return undefined;
        out += v;
        i = end + 1;
        continue;
      }
      out += raw.slice(i, end + 1);
      i = end + 1;
      continue;
    }
    const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(raw.slice(i + 1));
    if (m) {
      const v = process.env[m[0]];
      if (v === undefined) return undefined;
      out += v;
      i += 1 + m[0].length;
      continue;
    }
    out += "$";
    i++;
  }
  return out;
}

function expandHome(value: string): string {
  if (value === "~") return homedir();
  if (value.startsWith("~/") || (process.platform === "win32" && value.startsWith("~\\"))) {
    return join(homedir(), value.slice(2));
  }
  return value;
}

/** 与 pi 的 createDefaultTransport 同构：url 走 streamable HTTP，command 走 stdio */
function buildTransport(entry: Record<string, unknown>): McpTransport {
  if (typeof entry.url === "string") {
    let headers: Record<string, string> | undefined;
    if (entry.headers && typeof entry.headers === "object" && !Array.isArray(entry.headers)) {
      headers = {};
      for (const [k, v] of Object.entries(entry.headers as Record<string, unknown>)) {
        headers[k] = resolveConfigValue(String(v ?? "")) ?? "";
      }
    }
    return new StreamableHttpTransport({ url: entry.url, headers });
  }
  const env: Record<string, string> = {};
  if (entry.env && typeof entry.env === "object" && !Array.isArray(entry.env)) {
    for (const [k, v] of Object.entries(entry.env as Record<string, unknown>)) {
      env[k] = resolveConfigValue(String(v ?? "")) ?? "";
    }
  }
  return new StdioTransport({
    command: expandHome(String(entry.command ?? "")),
    args: Array.isArray(entry.args) ? entry.args.map((a) => expandHome(String(a))) : undefined,
    cwd: typeof entry.cwd === "string" && entry.cwd ? expandHome(entry.cwd) : undefined,
    env,
    stderr: "pipe",
  });
}

function describeError(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err);
  return msg.slice(0, 300) || "未知错误";
}

/** 连接一次 MCP 服务器并列出工具；任何失败都返回结构化结果而不抛出 */
export async function probeServerEntry(entry: unknown): Promise<ProbeResult> {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    return { ok: false, error: "无效的服务器配置" };
  }
  const e = entry as Record<string, unknown>;
  if (typeof e.url !== "string" && typeof e.command !== "string") {
    return { ok: false, error: "配置缺少 url 或 command 字段" };
  }
  let transport: McpTransport;
  try {
    transport = buildTransport(e);
  } catch (err) {
    return { ok: false, error: describeError(err) };
  }
  const client = new McpClient({ name: "pidock", version: "0.5.0", requestTimeoutMs: PROBE_TIMEOUT_MS });
  // requestTimeoutMs 覆盖 initialize/tools/list 单请求；总闸兜底传输层挂起的情况
  const totalTimer = setTimeout(() => void client.close().catch(() => {}), PROBE_TIMEOUT_MS + 2_000);
  try {
    const init: InitializeResult = await client.connect(transport);
    const tools: Tool[] = await client.listTools();
    return {
      ok: true,
      serverInfo: init.serverInfo
        ? {
            name: String(init.serverInfo.name ?? ""),
            version: String(init.serverInfo.version ?? ""),
          }
        : undefined,
      tools: (Array.isArray(tools) ? tools : []).slice(0, MAX_TOOLS).map((t) => ({
        name: String(t?.name ?? ""),
        description: String(t?.description ?? "").slice(0, 300),
        inputSchema: (t as unknown as Record<string, unknown> | undefined)?.inputSchema ?? null,
      })),
    };
  } catch (err) {
    if (err instanceof McpAuthRequiredError) {
      return {
        ok: false,
        needsAuth: true,
        error: "服务器要求认证（401）：请在请求头中配置 Authorization，例如 Authorization=Bearer <token>",
      };
    }
    if (err instanceof McpTimeoutError) {
      return { ok: false, error: "连接超时（15s）" };
    }
    if (err instanceof McpHttpError) {
      return { ok: false, error: `HTTP ${err.status}：${describeError(err)}` };
    }
    const msg = describeError(err);
    if (/not found in \$PATH|ENOENT/i.test(msg)) {
      const spaced = typeof e.command === "string" && e.command.trim().includes(" ");
      return {
        ok: false,
        error: spaced
          ? `${msg}。命令字段只接受单个可执行文件：请把空格后的参数移到「参数」中，或确认该文件确实存在。`
          : `${msg}。请确认该可执行文件已安装并在 PATH 中。`,
      };
    }
    return { ok: false, error: msg };
  } finally {
    clearTimeout(totalTimer);
    try {
      await client.close();
    } catch {
      // 已关闭/未连接时忽略
    }
  }
}

/** mcp.json 的顶层 servers 容器（兼容 mcpServers / servers 两种键名） */
export function readMcpServers(): Record<string, Record<string, unknown>> {
  try {
    const raw = JSON.parse(readFileSync(join(getAgentDir(), "mcp.json"), "utf8"));
    const servers = raw?.mcpServers ?? raw?.servers;
    return servers && typeof servers === "object" && !Array.isArray(servers) ? servers : {};
  } catch {
    return {};
  }
}
