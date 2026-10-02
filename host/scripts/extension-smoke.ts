/**
 * M1 risk check: do TS extensions (loaded via jiti) work inside the
 * bun-compiled single-file binary?
 *
 * Sets up an isolated agent dir with a marker extension, runs the host
 * (compiled exe by default), and verifies the extension actually executed.
 *
 * Run: pnpm --filter @pidock/host smoke:extension
 */
import { spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "../target/pidock-host.exe";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "").split(" ").filter(Boolean);

const EXTENSION_SOURCE = `
import { appendFileSync } from "node:fs";
appendFileSync(process.env.PIDOCK_MARKER!, "extension-loaded\\n");

import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const helloTool = defineTool({
  name: "hello",
  label: "Hello",
  description: "A simple greeting tool",
  parameters: Type.Object({ name: Type.String({ description: "Name to greet" }) }),
  async execute(_toolCallId, params) {
    return { content: [{ type: "text", text: \`Hello, \${params.name}!\` }], details: {} };
  },
});

export default function (pi: ExtensionAPI) {
  pi.registerTool(helloTool);
}
`;

async function main(): Promise<number> {
  const realAgentDir = getAgentDir();
  const agentDir = mkdtempSync(join(tmpdir(), "pidock-ext-agent-"));
  const workDir = mkdtempSync(join(tmpdir(), "pidock-ext-work-"));
  const marker = join(agentDir, "marker.txt");

  // copy real credentials so the model runtime can initialize
  for (const f of ["auth.json", "models.json", "settings.json", "models-store.json"]) {
    const src = join(realAgentDir, f);
    if (existsSync(src)) cpSync(src, join(agentDir, f));
  }
  mkdirSync(join(agentDir, "extensions"), { recursive: true });
  writeFileSync(join(agentDir, "extensions", "pidock-marker.ts"), EXTENSION_SOURCE);

  const child = spawn(HOST_BIN, HOST_ARGS, {
    stdio: ["pipe", "pipe", "pipe"],
    env: {
      ...process.env,
      PI_CODING_AGENT_DIR: agentDir,
      PIDOCK_MARKER: marker,
    },
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
      const frame = JSON.parse(line);
      if ("event_id" in frame) continue;
      const p = pending.get(frame.id);
      if (!p) continue;
      pending.delete(frame.id);
      frame.ok ? p.resolve(frame.result) : p.reject(new Error(frame.error?.message));
    }
  });

  const request = (method: string, params: unknown = {}): Promise<any> => {
    const id = crypto.randomUUID();
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      child.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
    });
  };

  const timeout = setTimeout(() => {
    console.log("FAIL: timeout");
    child.kill();
    process.exit(1);
  }, 90_000);

  try {
    console.log(`== extension smoke (host via ${HOST_BIN}, agentDir=${agentDir}) ==`);
    await request("ping");
    const created = await request("session.create", { cwd: workDir });
    console.log(`  session created: ${created.session_id.slice(0, 8)}…`);
    await request("session.close", { session_id: created.session_id });

    const loaded = existsSync(marker) && readFileSync(marker, "utf8").includes("extension-loaded");
    console.log(`  ${loaded ? "✔" : "✘"} extension (jiti TS) executed in compiled host`);
    clearTimeout(timeout);
    child.stdin.end();
    child.kill();
    rmSync(agentDir, { recursive: true, force: true });
    rmSync(workDir, { recursive: true, force: true });
    return loaded ? 0 : 1;
  } catch (err) {
    console.log(`FAIL: ${err}`);
    clearTimeout(timeout);
    child.kill();
    return 1;
  }
}

main().then((code) => process.exit(code));
