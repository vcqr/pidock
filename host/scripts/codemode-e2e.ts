// 端到端 codemode 回归：驱动编译版 host（协议 stdio），先确认会话工具表里有
// codemode，再让模型真实走一次 codemode 脚本（QuickJS worker 在编译产物内执行）。
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "../target/pidock-host.exe";
const child = spawn(HOST_BIN, [], { stdio: ["pipe", "pipe", "pipe"] });
child.stderr.on("data", (d) => process.stderr.write(d));
const workDir = mkdtempSync(join(tmpdir(), "pidock-codemode-e2e-"));

let buf = "";
const frames: any[] = [];
const waiters: Array<{ match: (f: any) => boolean; resolve: (f: any) => void }> = [];
child.stdout.on("data", (d) => {
  buf += d.toString();
  let idx: number;
  while ((idx = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, idx).trim();
    buf = buf.slice(idx + 1);
    if (!line) continue;
    let f: any;
    try {
      f = JSON.parse(line);
    } catch {
      console.log(`FAIL stdout polluted: ${JSON.stringify(line.slice(0, 120))}`);
      process.exit(1);
    }
    frames.push(f);
    if (process.env.PIDOCK_DUMP) require("node:fs").appendFileSync(process.env.PIDOCK_DUMP, line + "\n");
    waiters.forEach((w, i) => {
      if (w.match(f)) {
        waiters.splice(i, 1);
        w.resolve(f);
      }
    });
  }
});
const rpcId = { n: 0 };
function request(method: string, params: unknown = {}): Promise<any> {
  const id = `r${++rpcId.n}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout: ${method}`)), 120_000);
    waiters.push({
      match: (f) => f.id === id,
      resolve: (f) => {
        clearTimeout(timer);
        f.ok ? resolve(f.result) : reject(new Error(`${method}: ${f.error?.message}`));
      },
    });
    child.stdin.write(JSON.stringify({ id, method, params }) + "\n");
  });
}
function waitForEvent(match: (f: any) => boolean, timeoutMs = 120_000): Promise<any> {
  const hit = frames.find(match);
  if (hit) return Promise.resolve(hit);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("event timeout")), timeoutMs);
    waiters.push({
      match: (f) => {
        if (!match(f)) return false;
        clearTimeout(timer);
        return true;
      },
      resolve,
    });
  });
}

try {
  const created = await request("session.create", { cwd: workDir });
  const sessionId: string = created.session_id;
  await waitForEvent((f) => f.session_id === sessionId && f.kind === "session_meta");

  const toolNames: string[] = (await request("session.list_tools", { session_id: sessionId })).tools.map(
    (t: any) => t.name,
  );
  console.log(`session tools: ${toolNames.join(", ")}`);
  if (!toolNames.includes("codemode")) {
    console.log("FAIL codemode not in session tool list");
    process.exit(1);
  }

  // 只留 codemode（+tool_search 供脚本内 searchTools），逼模型走脚本而不是口算
  const active = await request("session.set_active_tools", {
    session_id: sessionId,
    active_names: ["codemode", "tool_search", "TodoWrite", "AskUserQuestion"],
  });
  console.log(`active tools: ${(active.active ?? []).join(", ")}`);

  await request("agent.prompt", {
    session_id: sessionId,
    text: "Use the codemode tool to compute (6*7) with a tiny script and reply with just the number.",
  });
  const codemodeCall = await waitForEvent(
    (f) => f.kind === "tool_execution_start" && f.payload?.tool_name === "codemode",
  ).catch(() => null);
  if (!codemodeCall) {
    console.log(
      "no codemode tool call; events seen:",
      [...new Set(frames.filter((f) => f.session_id === sessionId).map((f) => f.kind))].join(", "),
    );
    const msgs = frames.filter((f) => f.kind === "message_complete");
    console.log("last message:", JSON.stringify(msgs.at(-1)?.payload ?? {}).slice(0, 400));
    process.exit(1);
  }
  console.log(`codemode tool_call observed: args=${JSON.stringify(codemodeCall.payload?.args ?? {}).slice(0, 200)}`);
  const codemodeEnds = frames.filter(
    (f) => f.kind === "tool_execution_end" && f.payload?.tool_name === "codemode",
  );
  for (const end of codemodeEnds) {
    console.log(`codemode end: error=${end.payload?.is_error} output=${JSON.stringify(end.payload?.output ?? "")}`);
  }
  const done = await waitForEvent(
    (f) => f.kind === "agent_state_changed" && f.payload?.state === "idle",
  );
  void done;
  const complete = frames
    .filter((f) => f.kind === "message_complete" && f.payload?.role === "assistant")
    .at(-1);
  const text = JSON.stringify(complete?.payload ?? "");
  console.log(`assistant said: ${text.slice(0, 200)}`);
  console.log(text.includes("42") ? "PASS" : "FAIL answer missing 42");
} finally {
  child.stdin.end();
  setTimeout(() => {
    child.kill();
    rmSync(workDir, { recursive: true, force: true });
    process.exit(0);
  }, 300);
}
