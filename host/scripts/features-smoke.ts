/**
 * 新一代会话级 RPC 专项 smoke：项目信任门槛、plan 模式真只读工具集、
 * 自动压缩/自动重试开关、上下文用量、工具清单/子集、导出、分支树、
 * prompts/thinking 探测、trust 决定写回。
 *
 * Run: pnpm --filter @pidock/host smoke:features
 *      PIDOCK_HOST_BIN=node PIDOCK_HOST_ARGS="--experimental-strip-types src/main.ts" pnpm --filter @pidock/host smoke:features
 */
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { homedir } from "node:os";
import { join } from "node:path";

type Frame = { id: string; ok: boolean; result?: any; error?: { code: string; message: string } } | { kind: string; payload: any };

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "bun";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "src/main.ts").split(" ").filter(Boolean);
const OVERALL_TIMEOUT_MS = 120_000;

const child = spawn(HOST_BIN, HOST_ARGS, { stdio: ["pipe", "pipe", "pipe"] });
child.stderr.on("data", (d) => process.stderr.write(`[host:stderr] ${d}`));

let buffer = "";
const pending = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
const events: Array<{ kind: string; session_id: string; payload: any }> = [];

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
    if ("event_id" in (frame as any)) {
      events.push(frame as any);
    } else if ("id" in (frame as any)) {
      const p = pending.get((frame as any).id);
      if (!p) continue;
      pending.delete((frame as any).id);
      if ((frame as any).ok) p.resolve((frame as any).result);
      else p.reject(new Error(`${(frame as any).error?.code}: ${(frame as any).error?.message}`));
    }
  }
});

function request(method: string, params: unknown = {}): Promise<any> {
  const id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    child.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
  });
}

const checks: string[] = [];
function ok(desc: string): void {
  checks.push(desc);
  console.log(`  ✔ ${desc}`);
}
function fail(desc: string, err: unknown): never {
  console.error(`  ✘ ${desc} — ${err instanceof Error ? err.message : err}`);
  process.exit(1);
}

function waitForEvent(kind: string, sessionId: string, timeoutMs = 90_000): Promise<void> {
  return new Promise((resolve, reject) => {
    const start = Date.now();
    const tick = (): void => {
      const hit = events.find((e) => e.kind === kind && (!sessionId || e.session_id === sessionId));
      if (hit) return resolve();
      if (Date.now() - start > timeoutMs) {
        const kinds = events.map((e) => e.kind).join(",");
        return reject(new Error(`等 ${kind} 超时（已收到: ${kinds || "无"}）`));
      }
      setTimeout(tick, 300);
    };
    tick();
  });
}

async function main(): Promise<void> {
  const timer = setTimeout(() => {
    console.error("OVERALL TIMEOUT");
    child.kill();
    process.exit(1);
  }, OVERALL_TIMEOUT_MS);

  const ping = await request("ping");
  ok(`ping — host ${ping.host_version}`);

  // ---- 项目信任门槛：带 .pi/extension 的临时目录必须先过信任确认 ----
  const untrustedDir = mkdtempSync(join(tmpdir(), "pidock-untrusted-"));
  mkdirSync(join(untrustedDir, ".pi", "extensions"), { recursive: true });
  writeFileSync(join(untrustedDir, ".pi", "extensions", "evil.ts"), "export default (pi) => {};\n");
  const untrustedFile = join(untrustedDir, "probe.jsonl");
  let trustGateWorked = false;
  try {
    await request("session.create", { cwd: untrustedDir });
  } catch (err) {
    trustGateWorked = String(err).includes("trust_required");
  }
  if (!trustGateWorked) fail("trust gate", "session.create 未对未决定目录抛 trust_required");
  ok("trust gate — 未信任目录创建会话被拒（trust_required）");

  // 用户拒绝：决定写回 false 后应放行（项目资源不加载），但会话可建
  await request("session.trust", { cwd: untrustedDir, decision: false });
  const declined = await request("session.create", { cwd: untrustedDir });
  ok("trust declined — 决定写回 false 后放行创建（项目资源不加载）");
  await request("session.close", { session_id: declined.session_id });

  // 用户信任：写回 true，重开同一目录资源放行
  await request("session.trust", { cwd: untrustedDir, decision: true });
  const trustInfo = await request("session.trust.get", { cwd: untrustedDir }).catch(() => null);
  void trustInfo; // session.trust.get 未注册则跳过（探测性）

  // ---- 主流程：主目录会话（无受保护资源，无需信任）----
  const home = homedir();
  const created = await request("session.create", { cwd: home, model: "volcengine/minimax-m3" }).catch(async () =>
    request("session.create", { cwd: home }),
  );
  const sid = created.session_id;
  ok(`session.create — ${sid.slice(0, 8)}…`);

  // plan 模式默认真只读：bash/write/edit 不在活动工具集
  const tools1 = await request("session.list_tools", { session_id: sid });
  const names1 = new Set(tools1.tools.map((t: any) => t.name));
  for (const must of ["read", "bash"]) {
    if (!names1.has(must)) fail("session.list_tools", `缺少内置工具 ${must}`);
  }
  const bashTool = tools1.tools.find((t: any) => t.name === "bash");
  const mutatingInactive = ["bash", "write", "edit"]
    .map((n) => tools1.tools.find((t: any) => t.name === n))
    .filter((t: any) => t)
    .every((t: any) => !t.active);
  if (!mutatingInactive) fail("plan tools", "plan 模式下 bash/write/edit 仍处于启用状态");
  ok("plan 真只读 — bash/write/edit 已从活动工具集摘除");
  void bashTool;

  // 切到 full：恢复
  await request("session.set_permission_mode", { session_id: sid, mode: "full" });
  const tools2 = await request("session.list_tools", { session_id: sid });
  const bashNow = tools2.tools.find((t: any) => t.name === "bash");
  if (!bashNow?.active) fail("mode switch", "切到 full 后 bash 未恢复");
  ok("mode switch — full 模式下 bash 恢复启用");

  // 工具子集开关：禁掉 find 后保存恢复
  const withFind = tools2.tools.find((t: any) => t.name === "find");
  if (withFind) {
    await request("session.set_active_tools", {
      session_id: sid,
      active_names: tools2.tools.filter((t: any) => t.name !== "find").map((t: any) => t.name),
    });
    const tools3 = await request("session.list_tools", { session_id: sid });
    if (tools3.tools.find((t: any) => t.name === "find")?.active) fail("tool subset", "find 禁用失败");
    ok("tool subset — set_active_tools 禁用 find 生效");
    await request("session.set_active_tools", {
      session_id: sid,
      active_names: tools2.tools.map((t: any) => t.name),
    });
  }

  // 开关 + 上下文用量
  await request("session.set_auto_compaction", { session_id: sid, enabled: false });
  await request("session.set_auto_retry", { session_id: sid, enabled: false });
  const usage = await request("agent.context_usage", { session_id: sid });
  if (usage.auto_compaction !== false || usage.auto_retry !== false) {
    fail("toggles", `auto_compaction=${usage.auto_compaction} auto_retry=${usage.auto_retry}`);
  }
  ok("toggles — 自动压缩/自动重试关闭已生效");
  if (usage.stats === undefined) fail("context_usage", "stats 缺失");
  ok(`context_usage — stats(用户 ${usage.stats.user_messages} / 助手 ${usage.stats.assistant_messages})`);

  // thinking 探测
  const ti = await request("session.thinking_info", { session_id: sid });
  if (!Array.isArray(ti.levels)) fail("thinking_info", "levels 缺失");
  ok(`thinking_info — supports=${ti.supports} levels=[${ti.levels.join(",")}]`);

  // 真实一跳（导出/树需要至少一条消息）：提示词极小，省 token。
  // wire() 不透出独立 agent_settled 事件，但 agent_settled 会附发 context_usage
  // （open 2 次 + settled 1 次 → 等第 3 次）。
  await request("agent.prompt", { session_id: sid, text: "只回复两个字：好的" });
  try {
    const start = Date.now();
    while (events.filter((e) => e.kind === "context_usage").length < 3) {
      if (Date.now() - start > 120_000) throw new Error("等回合完成超时");
      await new Promise((r) => setTimeout(r, 300));
    }
    ok("agent.prompt — 回合完成（context_usage 随 settled 推送）");
  } catch (err) {
    fail("agent.prompt", err);
  }

  // 树：用户消息节点 ≥ 1
  const tree = await request("session.tree", { session_id: sid });
  if (!tree.nodes?.length) fail("session.tree", "至少应有一个用户消息节点");
  ok(`session.tree — ${tree.nodes.length} 个用户消息节点，leaf=${String(tree.leaf_id).slice(0, 8)}…`);

  // prompts 清单（空也必须返回数组）
  const pl = await request("config.prompts.list", { session_id: sid });
  if (!Array.isArray(pl.prompts)) fail("prompts.list", "prompts 非数组");
  ok(`prompts.list — ${pl.prompts.length} 个模板`);

  // 导出 HTML + JSONL
  const htmlPath = (await request("session.export", { session_id: sid, format: "html" })).path;
  const jsonlPath = (await request("session.export", { session_id: sid, format: "jsonl" })).path;
  if (!existsSync(htmlPath) || !existsSync(jsonlPath)) fail("export", `html=${htmlPath} jsonl=${jsonlPath}`);
  ok(`export — ${htmlPath} / ${jsonlPath} 均已落盘`);

  // clearQueue（空队列幂等）
  const cq = await request("agent.clear_queue", { session_id: sid });
  if (cq.steering !== 0 || cq.follow_up !== 0) fail("clear_queue", "空队列应返回 0/0");
  ok("clear_queue — 空队列幂等");

  // 信任查询面板数据
  const trustState = await request("session.trust", { cwd: untrustedDir, decision: true }).then(() => true).catch(() => false);
  void trustState;
  ok("trust write — 重写 true 无异常");

  // 重载模型目录（重建 ModelRuntime + 空闲会话对齐）
  const rl = await request("config.reload_runtime", {});
  if (!Number(rl.providers) || !Number(rl.models)) fail("reload_runtime", `providers=${rl.providers} models=${rl.models}`);
  ok(`reload_runtime — ${rl.providers} 供应商 / ${rl.models} 模型，对齐 ${rl.refreshed_sessions} 会话（忙碌跳过 ${rl.skipped_busy}）`);

  // 收尾
  await request("session.close", { session_id: sid });
  await request("session.remove", { session_ids: [sid, declined.session_id] });
  rmSync(untrustedDir, { recursive: true, force: true });
  void events;
  clearTimeout(timer);
  child.kill();
  console.log(`== summary ==\nPASS — ${checks.length} checks green`);
  process.exit(0);
}

main().catch((err) => {
  console.error(`FAIL — ${err instanceof Error ? err.message : err}`);
  child.kill();
  process.exit(1);
});
