/**
 * Subagent structural smoke: spawn pi-host with an ISOLATED agent dir
 * (no auth/models → no API calls, no token spend), create a session and
 * verify the Task tool is registered into the model toolset, plus the
 * normal session surfaces no subagent markers.
 *
 * Run: bun run scripts/subagent-smoke.ts
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

type Frame = { id: string; ok: boolean; result?: any; error?: { code: string; message: string } } | { event_id: string; kind: string };

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "bun";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "src/main.ts").split(" ").filter(Boolean);

const agentDir = mkdtempSync(join(tmpdir(), "pidock-subagent-smoke-"));
const child = spawn(HOST_BIN, HOST_ARGS, {
  stdio: ["pipe", "pipe", "pipe"],
  env: { ...process.env, PI_CODING_AGENT_DIR: agentDir },
});
child.stderr.on("data", (d) => process.stderr.write(`[host:stderr] ${d}`));

let buffer = "";
const pending = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
child.stdout!.on("data", (chunk) => {
  buffer += chunk.toString("utf8");
  let idx: number;
  while ((idx = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, idx).trim();
    buffer = buffer.slice(idx + 1);
    if (!line) continue;
    let frame: Frame;
    try {
      frame = JSON.parse(line);
    } catch {
      continue;
    }
    if ("event_id" in frame) continue;
    const p = pending.get(frame.id);
    if (!p) continue;
    pending.delete(frame.id);
    if (frame.ok) p.resolve(frame.result);
    else p.reject(new Error(`${frame.error?.code}: ${frame.error?.message}`));
  }
});

let seq = 0;
function request(method: string, params?: unknown): Promise<any> {
  const id = `s${++seq}`;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    child.stdin!.write(JSON.stringify({ id, method, params: params ?? {} }) + "\n");
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error(`timeout: ${method}`));
      }
    }, 30_000);
  });
}

let failures = 0;
function check(name: string, ok: boolean, detail?: string): void {
  console.log(`${ok ? "(pass)" : "(FAIL)"} ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
}

try {
  const pong = await request("ping");
  check("host ready", !!pong.host_version);

  const created = await request("session.create", { cwd: agentDir });
  check("session.create", !!created.session_id);

  const tools = await request("session.list_tools", { session_id: created.session_id });
  const names: string[] = (tools.tools ?? []).map((t: any) => t.name);
  const task = (tools.tools ?? []).find((t: any) => t.name === "Task");
  check("Task tool registered", !!task, names.join(","));
  check("Task tool active", task?.active === true);

  const list = await request("session.list");
  const summary = (list.sessions ?? []).find((s: any) => s.session_id === created.session_id);
  check("normal session has no kind", !!summary && summary.kind === undefined);
  check("normal session has no parent", !!summary && summary.parent_session_id === undefined);
} catch (err) {
  failures++;
  console.log(`(FAIL) smoke aborted: ${err instanceof Error ? err.message : err}`);
} finally {
  child.kill("SIGTERM");
  setTimeout(() => rmSync(agentDir, { recursive: true, force: true }), 200);
}

process.exit(failures ? 1 : 0);
