/**
 * Subagent PARALLEL e2e: spawn pi-host (REAL agent dir — uses your configured
 * model & auth), drive one parent turn that must fire 3 Task calls at once,
 * and verify true concurrency:
 *
 *   1. ≥3 child sessions registered under the same parent
 *   2. each child produced its own marker reply, and the parent's 3 Task
 *      toolResults carry them back
 *   3. OVERLAP: the children's [first-event, last-event] time windows overlap
 *      (span << sum of durations) — hard evidence of parallel execution,
 *      not sequential spawn-and-wait
 *
 * Run: bun run scripts/subagent-parallel-e2e.ts
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
const DEADLINE_MS = 240_000;
const MARKERS = ["甲", "乙", "丙"];

const cwd = mkdtempSync(join(tmpdir(), "pidock-subagent-par-e2e-"));
const child = spawn(HOST_BIN, HOST_ARGS, { stdio: ["pipe", "pipe", "pipe"] });
child.stderr.on("data", (d) => process.stderr.write(`[host:stderr] ${d}`));

let buffer = "";
const pending = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
const events: Array<{ session_id: string; kind: string; ts: string; payload: any }> = [];

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
      events.push({ session_id: frame.session_id, kind: frame.kind, ts: (frame as any).ts ?? "", payload: frame.payload });
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
  check("host ready", !!pong.host_version);

  const created = await request("session.create", { cwd });
  const parentId = created.session_id;

  await request("agent.prompt", {
    session_id: parentId,
    text:
      `在本回合一次性并行调用 3 次 Task 工具（3 个 toolCall 放在同一条消息里同时发起，` +
      `绝对不要等第一个完成再发第二个）。三个子代理的参数：` +
      `第 1 个 label 填 "并行甲"，prompt 填 "直接回复一个字：${MARKERS[0]}。禁止调用任何工具。"；` +
      `第 2 个 label 填 "并行乙"，prompt 填 "直接回复一个字：${MARKERS[1]}。禁止调用任何工具。"；` +
      `第 3 个 label 填 "并行丙"，prompt 填 "直接回复一个字：${MARKERS[2]}。禁止调用任何工具。"。` +
      `收到三份报告后在回复里逐行列出。除此之外不要调用任何工具。`,
  });

  const idleOk = await waitFor("parent idle", () =>
    events.some((e) => e.session_id === parentId && e.kind === "agent_state_changed" && e.payload?.state === "idle"),
  );
  check("父回合结束", idleOk);

  // 1) 同一父会话下的子代理数量
  const childIds = [
    ...new Set(
      events
        .filter((e) => e.kind === "session_meta" && e.payload?.kind === "subagent" && e.payload?.parent_session_id === parentId)
        .map((e) => e.session_id),
    ),
  ];
  check("派出 3 个子代理", childIds.length === 3, `实际 ${childIds.length}`);

  // 2) 各子代理产出各自的标记；父会话 3 份 Task 结果回传
  const finalTextOf = (sid: string): string => {
    const msgs = events.filter((e) => e.session_id === sid && e.kind === "message_complete" && e.payload?.role === "assistant");
    for (let i = msgs.length - 1; i >= 0; i--) {
      const text = (msgs[i]!.payload.blocks ?? [])
        .filter((b: any) => b.type === "text")
        .map((b: any) => b.text ?? "")
        .join("\n")
        .trim();
      if (text) return text;
    }
    return "";
  };
  for (const m of MARKERS) {
    const produced = childIds.some((sid) => finalTextOf(sid).includes(m));
    check(`子代理产出「${m}」`, produced);
  }
  const taskResults = events
    .filter((e) => e.session_id === parentId && e.kind === "message_complete")
    .flatMap((e) => e.payload?.blocks ?? [])
    .filter((b: any) => b.type === "toolResult" && b.toolName === "Task");
  check("父会话收到 3 份 Task 报告", taskResults.length === 3, `实际 ${taskResults.length}`);
  check(
    "三份报告都含标记",
    MARKERS.every((m) => taskResults.some((b: any) => String(b.output ?? "").includes(m))),
  );

  // 3) 并行硬证据：子代理生命周期时间窗重叠（span 远小于时长之和）
  const spans = childIds.map((sid) => {
    const ts = events.filter((e) => e.session_id === sid).map((e) => Date.parse(e.ts)).filter(Number.isFinite);
    return ts.length ? { start: Math.min(...ts), end: Math.max(...ts) } : null;
  });
  const valid = spans.filter((s): s is { start: number; end: number } => !!s);
  if (valid.length === childIds.length && valid.length >= 2) {
    const sorted = [...valid].sort((a, b) => a.start - b.start);
    const overlapWidest = sorted[sorted.length - 1]!.start < sorted[0]!.end;
    const span = Math.max(...valid.map((s) => s.end)) - Math.min(...valid.map((s) => s.start));
    const sum = valid.reduce((n, s) => n + (s.end - s.start), 0);
    const ratio = span / Math.max(1, sum);
    check(
      "时间窗重叠（真并行）",
      overlapWidest && ratio < 0.75,
      `span ${Math.round(span / 100) / 10}s vs Σ ${Math.round(sum / 100) / 10}s（ratio ${ratio.toFixed(2)}，<0.75 判为并行）`,
    );
  } else {
    check("时间窗重叠（真并行）", false, "子代理事件不足");
  }

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
