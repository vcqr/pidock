/**
 * Subagent LIVE e2e: spawn pi-host (REAL agent dir — uses your configured
 * model & auth, ~a few k tokens), drive one parent turn that must call the
 * Task tool, and verify the full loop:
 *
 *   1. child session_meta arrives with kind=subagent + parent_session_id
 *   2. the child actually streamed messages (message_complete on child id)
 *   3. the parent's Task toolResult carries the child's final report
 *   4. session.list marks the child kind=subagent / closed / parent linkage
 *
 * Run: bun run scripts/subagent-e2e.ts
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

type Frame =
  | { id: string; ok: boolean; result?: any; error?: { code: string; message: string } }
  | { event_id: string; session_id: string; kind: string; payload: any };

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "bun";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "src/main.ts").split(" ").filter(Boolean);
const DEADLINE_MS = 180_000;
const MARKER = "子代理链路正常";

const cwd = mkdtempSync(join(tmpdir(), "pidock-subagent-e2e-"));
const child = spawn(HOST_BIN, HOST_ARGS, { stdio: ["pipe", "pipe", "pipe"], cwd: "src/.." });
child.stderr.on("data", (d) => process.stderr.write(`[host:stderr] ${d}`));

let buffer = "";
const pending = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
const events: Array<{ session_id: string; kind: string; payload: any }> = [];

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
    if ("event_id" in frame) {
      events.push({ session_id: frame.session_id, kind: frame.kind, payload: frame.payload });
    } else if ("id" in frame) {
      const p = pending.get(frame.id);
      if (!p) continue;
      pending.delete(frame.id);
      if (frame.ok) p.resolve(frame.result);
      else p.reject(new Error(`${frame.error?.code}: ${frame.error?.message}`));
    }
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

/** 轮询事件集直到谓词满足或超时 */
async function waitFor(what: string, pred: () => boolean): Promise<boolean> {
  const deadline = Date.now() + DEADLINE_MS;
  while (Date.now() < deadline) {
    if (pred()) return true;
    await new Promise((r) => setTimeout(r, 300));
  }
  console.log(`(FAIL) 等待超时: ${what}`);
  return false;
}

let failures = 0;
function check(name: string, ok: boolean, detail?: string): void {
  console.log(`${ok ? "(pass)" : "(FAIL)"} ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures++;
}

try {
  const pong = await request("ping");
  check("host ready", !!pong.host_version, `host ${pong.host_version}`);

  const created = await request("session.create", { cwd });
  const parentId = created.session_id;
  check("session.create", !!parentId);

  await request(
    "agent.prompt",
    {
      session_id: parentId,
      text:
        `调用 Task 工具派生一个子代理。参数：label 填 "链路验证"；prompt 填 "直接回复一句话：${MARKER}。不要调用任何其他工具，不要读写文件。" ` +
        `拿到子代理报告后原样转述它。除 Task 外不要调用任何工具。`,
    },
  );

  // 1) 子代理元数据事件
  const metaOk = await waitFor("subagent session_meta", () =>
    events.some((e) => e.kind === "session_meta" && e.payload?.kind === "subagent" && e.payload?.parent_session_id === parentId),
  );
  check("子代理 session_meta（kind/parent 关联）", metaOk);
  const childMeta = events.find((e) => e.kind === "session_meta" && e.payload?.kind === "subagent");
  const childId = childMeta?.session_id ?? "";

  // 2) 父回合结束
  const idleOk = await waitFor("parent idle", () =>
    events.some((e) => e.session_id === parentId && e.kind === "agent_state_changed" && e.payload?.state === "idle"),
  );
  check("父回合结束", idleOk);

  // 3) 子代理有真实消息产出
  check("子代理有消息产出", events.some((e) => e.session_id === childId && e.kind === "message_complete"));

  // 4) 父会话 Task 工具结果带回报告
  const taskResult = events
    .filter((e) => e.session_id === parentId && e.kind === "message_complete")
    .flatMap((e) => e.payload?.blocks ?? [])
    .find((b: any) => b.type === "toolResult" && b.toolName === "Task");
  check("Task 工具结果非空", !!taskResult && String(taskResult.output ?? "").trim().length > 0);
  check(
    "报告回传父会话（含标记句）",
    !!taskResult && String(taskResult.output ?? "").includes(MARKER),
    String(taskResult?.output ?? "").slice(0, 120),
  );

  // 5) 注册表投影：kind/parent/name/关闭
  const list = await request("session.list");
  const summary = (list.sessions ?? []).find((s: any) => s.session_id === childId);
  check("session.list 子代理 kind=subagent", summary?.kind === "subagent");
  check("session.list parent 关联", summary?.parent_session_id === parentId);
  check("session.list 显示名", summary?.name === "链路验证", summary?.name);
  check("子代理已关闭（不占池）", summary?.open === false);

  const totalTokens = events
    .filter((e) => e.kind === "message_complete" && e.payload?.usage)
    .reduce((n, e) => n + (e.payload.usage.totalTokens ?? 0), 0);
  console.log(`[tokens] 本轮消耗 ≈ ${totalTokens}`);
} catch (err) {
  failures++;
  console.log(`(FAIL) e2e aborted: ${err instanceof Error ? err.message : err}`);
} finally {
  child.kill("SIGTERM");
  setTimeout(() => rmSync(cwd, { recursive: true, force: true }), 200);
}

process.exit(failures ? 1 : 0);
