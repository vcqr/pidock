/**
 * M1 smoke test: spawn pi-host as a child process, drive it over stdio JSONL,
 * and verify the full local loop (session create -> prompt -> streamed events
 * -> persisted entries -> replay). Uses the model configured in ~/.pi/agent.
 *
 * Run: pnpm --filter @pidock/host smoke           (bun)
 *      PIDOCK_HOST_BIN=node PIDOCK_HOST_ARGS="--experimental-strip-types src/main.ts" pnpm --filter @pidock/host smoke
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

interface Envelope {
  event_id: string;
  session_id: string;
  seq?: number;
  persist: boolean;
  ts: string;
  kind: string;
  payload: any;
}
type Frame = { id: string; ok: boolean; result?: any; error?: { code: string; message: string } } | Envelope;

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "bun";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "src/main.ts").split(" ").filter(Boolean);
const OVERALL_TIMEOUT_MS = 180_000;

const child = spawn(HOST_BIN, HOST_ARGS, { stdio: ["pipe", "pipe", "pipe"] });
child.stderr.on("data", (d) => process.stderr.write(`[host:stderr] ${d}`));
child.on("exit", (code) => console.log(`[host exited: ${code}]`));

let buffer = "";
const pending = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
const events: Envelope[] = [];

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
      console.log(`[unparseable] ${line.slice(0, 120)}`);
      continue;
    }
    if ("event_id" in frame) {
      events.push(frame);
      logEvent(frame as Envelope);
    } else if ("id" in frame) {
      const p = pending.get(frame.id);
      if (!p) continue;
      pending.delete(frame.id);
      if (frame.ok) p.resolve(frame.result);
      else p.reject(new Error(`${frame.error?.code}: ${frame.error?.message}`));
    }
  }
});

function logEvent(e: Envelope): void {
  const brief = JSON.stringify(e.payload)?.slice(0, 100) ?? "";
  console.log(
    `  [evt] ${e.persist ? `persist#${e.seq}` : "eph   "} ${e.kind.padEnd(24)} ${brief}`,
  );
}

function request(method: string, params: unknown = {}): Promise<any> {
  const id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    child.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
  });
}

function waitFor(desc: string, predicate: () => boolean, timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const timer = setInterval(() => {
      if (predicate()) {
        clearInterval(timer);
        resolve();
      } else if (Date.now() - start > timeoutMs) {
        clearInterval(timer);
        reject(new Error(`timeout waiting for: ${desc}`));
      }
    }, 100);
  });
}

const counters = () => {
  const persist = events.filter((e) => e.persist);
  return {
    deltas: events.filter((e) => e.kind === "message_delta").length,
    snapshots: events.filter((e) => e.kind === "message_snapshot").length,
    states: events.filter((e) => e.kind === "agent_state_changed").length,
    persistCount: persist.length,
    assistantComplete: persist.find((e) => e.kind === "message_complete" && e.payload?.role === "assistant"),
  };
};

async function main(): Promise<number> {
  const workDir = mkdtempSync(join(tmpdir(), "pidock-smoke-"));
  const failures: string[] = [];
  const check = (name: string, cond: boolean, detail = "") => {
    console.log(`${cond ? "  ✔" : "  ✘"} ${name}${detail ? ` — ${detail}` : ""}`);
    if (!cond) failures.push(name);
  };

  try {
    console.log(`== PiDock M1 smoke (host via ${HOST_BIN} ${HOST_ARGS.join(" ")}) ==`);

    const pong = await request("ping");
    check("ping", typeof pong?.host_version === "string", `host ${pong?.host_version}`);

    const created = await request("session.create", { cwd: workDir });
    check("session.create", typeof created?.session_id === "string", `id=${created?.session_id?.slice(0, 8)}…`);
    const sessionId: string = created.session_id;

    await waitFor("session_meta event", () => events.some((e) => e.session_id === sessionId && e.kind === "session_meta"));
    check("session_meta event", true);

    await request("agent.prompt", {
      session_id: sessionId,
      text: "请用大约150字介绍一下Rust语言的特点，写完后再单独用一行写：完。",
    });
    check("agent.prompt accepted", true);

    await waitFor("assistant message_complete (persist)", () => counters().assistantComplete !== undefined, 150_000);
    const c = counters();
    check("assistant message_complete", c.assistantComplete !== undefined);
    check("streamed deltas", c.deltas > 0, `${c.deltas} deltas`);
    check("snapshots flowed", c.snapshots > 0, `${c.snapshots} snapshots`);
    check("state transitions", c.states >= 2, `${c.states} states`);
    const text: string = (c.assistantComplete?.payload?.blocks ?? [])
      .filter((b: any) => b.type === "text")
      .map((b: any) => b.text)
      .join("");
    check("answer text non-empty", text.trim().length > 0, `"${text.trim().slice(0, 60)}"`);

    await waitFor("state back to idle", () =>
      events.some((e) => e.kind === "agent_state_changed" && e.payload?.state === "idle"),
    );

    const replay = await request("session.events", { session_id: sessionId });
    const seqs: number[] = replay.events.map((e: any) => e.seq);
    const ordered = seqs.every((s, i) => i === 0 || (seqs[i - 1] !== undefined && s > (seqs[i - 1] as number)));
    check("replay ordered", ordered, `${replay.events.length} events`);
    check("replay includes assistant message", replay.events.some((e: any) => e.kind === "message_complete" && e.payload?.role === "assistant"));

    const listed = await request("session.list");
    check("session.list", listed.sessions.some((s: any) => s.session_id === sessionId));

    const closed = await request("session.close", { session_id: sessionId });
    check("session.close", closed?.closed === true);

    await rmSync(workDir, { recursive: true, force: true });
  } catch (err) {
    failures.push(String(err instanceof Error ? err.message : err));
  }

  console.log("== summary ==");
  if (failures.length) {
    console.log(`FAIL (${failures.length}): ${failures.join("; ")}`);
    child.kill();
    return 1;
  }
  console.log("PASS — all checks green");
  child.stdin.end();
  child.kill();
  return 0;
}

const timer = setTimeout(() => {
  console.log("FAIL: overall timeout");
  child.kill();
  process.exit(1);
}, OVERALL_TIMEOUT_MS);

main()
  .then((code) => {
    clearTimeout(timer);
    process.exit(code);
  })
  .catch((err) => {
    console.error("smoke crashed:", err);
    child.kill();
    process.exit(1);
  });
