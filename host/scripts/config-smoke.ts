/**
 * M3 config smoke: exercises the full config.* surface against an isolated
 * agent dir (no real LLM call). Verifies settings round-trips, provider key
 * storage, model default, extension/skill discovery + toggle, mcp.json.
 *
 * Run: pnpm --filter @pidock/host smoke:config
 */
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "../target/pidock-host.exe";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "").split(" ").filter(Boolean);

async function main(): Promise<number> {
  const agentDir = mkdtempSync(join(tmpdir(), "pidock-cfg-agent-"));
  const workDir = mkdtempSync(join(tmpdir(), "pidock-cfg-work-"));
  const failures: string[] = [];
  const check = (name: string, cond: boolean, detail = "") => {
    console.log(`${cond ? "  ✔" : "  ✘"} ${name}${detail ? ` — ${detail}` : ""}`);
    if (!cond) failures.push(name);
  };

  // plant: one extension, one skill
  mkdirSync(join(agentDir, "extensions"), { recursive: true });
  writeFileSync(join(agentDir, "extensions", "my-ext.ts"), "export default function () {};\n");
  mkdirSync(join(agentDir, "skills", "test-skill"), { recursive: true });
  writeFileSync(
    join(agentDir, "skills", "test-skill", "SKILL.md"),
    "---\nname: test-skill\ndescription: A skill planted by config smoke\n---\n\nDo nothing.\n",
  );
  mkdirSync(join(agentDir, "skills", "test-skill", "references"), { recursive: true });
  writeFileSync(join(agentDir, "skills", "test-skill", "references", "api.md"), "api docs\n");
  writeFileSync(join(agentDir, "skills", "test-skill", "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x00, 0x00, 0x01]));

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
      const frame = JSON.parse(line);
      if ("event_id" in frame) continue;
      const p = pending.get(frame.id);
      if (!p) continue;
      pending.delete(frame.id);
      if (frame.ok) p.resolve(frame.result);
      else p.reject(new Error(`${frame.error?.code}: ${frame.error?.message}`));
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
    console.log(`== PiDock M3 config smoke (host via ${HOST_BIN}) ==`);

    const g = await request("config.get");
    check("config.get", typeof g.agent_dir === "string");

    await request("config.settings.set", { patch: { defaultProjectTrust: "always", quietStartup: true } });
    const settingsAfter = JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8"));
    check("config.settings.set", settingsAfter.defaultProjectTrust === "always" && settingsAfter.quietStartup === true);

    try {
      await request("config.settings.set", { patch: { notASetting: 1 } });
      check("config.settings.set rejects unknown keys", false);
    } catch (e) {
      check("config.settings.set rejects unknown keys", String(e).includes("unknown_setting"));
    }

    const exts = await request("config.extensions.list", { cwd: workDir });
    const myExt = exts.extensions.find((e: any) => e.name === "my-ext");
    check("extensions.list finds planted ext", myExt !== undefined && myExt.enabled === true);

    await request("config.extensions.toggle", { file: myExt.file, enabled: false });
    const exts2 = await request("config.extensions.list", { cwd: workDir });
    check("extensions.toggle disables", exts2.extensions.find((e: any) => e.name === "my-ext").enabled === false);

    const skills = await request("config.skills.list", { cwd: workDir });
    const mySkill = skills.skills.find((s: any) => s.name === "test-skill");
    check("skills.list finds planted skill", mySkill !== undefined, mySkill?.description);

    await request("config.skills.toggle", { path: mySkill.path, enabled: false });
    const skills2 = await request("config.skills.list", { cwd: workDir });
    check("skills.toggle disables", skills2.skills.find((s: any) => s.name === "test-skill").enabled === false);

    const detail = await request("config.skills.files", { path: mySkill.path });
    check(
      "skills.files lists planted files",
      typeof detail.dir === "string" &&
        detail.files.some((f: any) => f.file === "SKILL.md") &&
        detail.files.some((f: any) => f.file === "references/api.md"),
      detail.files?.map((f: any) => f.file).join(", "),
    );

    const read = await request("config.skills.read", { path: mySkill.path, file: "SKILL.md" });
    check("skills.read returns SKILL.md body", read.text.includes("Do nothing."));

    try {
      await request("config.skills.read", { path: mySkill.path, file: "../escape.md" });
      check("skills.read rejects path escape", false);
    } catch (e) {
      check("skills.read rejects path escape", String(e).includes("bad_request"));
    }

    try {
      await request("config.skills.files", { path: join(agentDir, "settings.json") });
      check("skills.files rejects non-SKILL.md path", false);
    } catch (e) {
      check("skills.files rejects non-SKILL.md path", String(e).includes("bad_request"));
    }

    const detailBin = await request("config.skills.files", { path: mySkill.path });
    check(
      "skills.files marks binary files",
      detailBin.files.find((f: any) => f.file === "logo.png")?.binary === true &&
        detailBin.files.find((f: any) => f.file === "SKILL.md")?.binary === false,
    );

    try {
      await request("config.skills.read", { path: mySkill.path, file: "logo.png" });
      check("skills.read rejects binary content", false);
    } catch (e) {
      check("skills.read rejects binary content", String(e).includes("binary_file"));
    }

    const extRead = await request("config.extensions.read", { file: myExt.file });
    check("extensions.read returns plugin source", extRead.text.includes("export default"));

    try {
      await request("config.extensions.read", { file: join(agentDir, "skills", "test-skill", "logo.png") });
      check("extensions.read rejects non-source files", false);
    } catch (e) {
      check("extensions.read rejects non-source files", String(e).includes("bad_request"));
    }

    await request("config.providers.custom.set", {
      id: "my-proxy",
      entry: {
        baseUrl: "https://proxy.example.com/v1",
        api: "openai-completions",
        models: [{ id: "proxy-mini" }, { id: "proxy-max" }],
      },
    });
    const custom = await request("config.providers.custom.get");
    check(
      "providers.custom.set/get roundtrip",
      custom.config?.providers?.["my-proxy"]?.baseUrl === "https://proxy.example.com/v1" &&
        custom.config?.providers?.["my-proxy"]?.models?.length === 2,
    );
    await request("config.providers.custom.remove", { id: "my-proxy" });
    const customAfter = await request("config.providers.custom.get");
    check("providers.custom.remove", !("my-proxy" in (customAfter.config?.providers ?? {})));

    await request("config.mcp.set", { config: { servers: { demo: { command: "echo", args: ["hi"] } } } });
    const mcp = await request("config.mcp.get");
    check("mcp.set/get roundtrip", mcp.config?.servers?.demo?.command === "echo");

    await request("config.providers.set_key", { provider: "anthropic", key: "sk-ant-test-123" });
    const auth = JSON.parse(readFileSync(join(agentDir, "auth.json"), "utf8"));
    check("provider.set_key writes auth.json", auth.anthropic?.key === "sk-ant-test-123");

    const providers = await request("config.providers.list");
    const anthropic = providers.providers.find((p: any) => p.id === "anthropic");
    check("providers.list shows stored auth", anthropic !== undefined && anthropic.auth !== "missing", anthropic?.auth);

    const models = await request("config.models.list");
    check("models.list non-empty", Array.isArray(models.models) && models.models.length > 0, `${models.models?.length} models`);

    await request("config.models.set_default", { provider: "anthropic", model: "claude-sonnet-4-20250514" });
    const settingsFinal = JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8"));
    check("models.set_default", settingsFinal.defaultProvider === "anthropic");

    // ---- memory (AGENTS.md) ----
    const memBefore = await request("config.agents.read");
    check("agents.read absent file", memBefore.exists === false && memBefore.text === "");
    await request("config.agents.write", { text: "# 记忆\n- smoke test\n" });
    const memAfter = await request("config.agents.read");
    check(
      "agents.write/read roundtrip",
      memAfter.exists === true && memAfter.text.includes("smoke test") && memAfter.path.endsWith("AGENTS.md"),
    );

    // ---- usage stats（植入 registry + 会话 JSONL 后聚合） ----
    const sessDir = join(agentDir, "sessions", "proj");
    mkdirSync(sessDir, { recursive: true });
    const jsonl = [
      JSON.stringify({ type: "session", version: 3, id: "smoke-1", timestamp: "2026-10-03T10:00:00.000Z", cwd: workDir }),
      JSON.stringify({ type: "model_change", id: "a1", parentId: null, timestamp: "2026-10-03T10:00:01.000Z", provider: "volcengine", modelId: "minimax-m3" }),
      JSON.stringify({ type: "message", id: "a2", parentId: "a1", timestamp: "2026-10-03T10:00:02.000Z", message: { role: "user", content: "hi" } }),
      JSON.stringify({ type: "message", id: "a3", parentId: "a2", timestamp: "2026-10-03T10:00:03.000Z", message: { role: "assistant", content: [], provider: "volcengine", model: "minimax-m3", usage: { input: 100, output: 20, cacheRead: 5, cacheWrite: 0, reasoning: 0, totalTokens: 125, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0.5 } } } }),
      JSON.stringify({ type: "message", id: "a4", parentId: "a3", timestamp: "2026-10-04T10:00:04.000Z", message: { role: "assistant", content: [], provider: "volcengine", model: "minimax-m3", usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15 } } }),
    ].join("\n");
    const sessFile = join(sessDir, "2026-10-03T10-00-00-000Z_smoke-1.jsonl");
    writeFileSync(sessFile, jsonl + "\n");
    mkdirSync(join(agentDir, "pidock"), { recursive: true });
    writeFileSync(
      join(agentDir, "pidock", "registry.json"),
      JSON.stringify({
        sessions: [
          {
            session_id: "smoke-1",
            file: sessFile,
            cwd: workDir,
            provider: "volcengine",
            model: "minimax-m3",
            created_at: "2026-10-03T10:00:00.000Z",
          },
        ],
      }),
    );
    const usage = await request("stats.usage", {});
    check(
      "stats.usage totals",
      usage.scanned_sessions === 1 &&
        usage.messages.user === 1 &&
        usage.messages.assistant === 2 &&
        usage.tokens.input === 110 &&
        usage.tokens.output === 25 &&
        usage.tokens.total === 140 &&
        Math.abs(usage.cost - 0.5) < 1e-9,
      JSON.stringify(usage.tokens),
    );
    check(
      "stats.usage by_model/by_day",
      usage.by_model?.[0]?.key === "volcengine/minimax-m3" &&
        usage.by_model?.[0]?.tokens === 140 &&
        usage.by_day?.length === 2,
      usage.by_model?.map((m: any) => m.key).join(","),
    );
    check("stats.usage top_sessions", usage.top_sessions?.[0]?.tokens === 140 && usage.top_sessions?.[0]?.session_id === "smoke-1");

    // ---- pidock 自有设置（代理） ----
    const appBefore = await request("pidock.settings.get");
    check("pidock.settings.get empty default", typeof appBefore.path === "string" && !appBefore.settings?.proxy);
    try {
      await request("pidock.settings.set", { proxy: { mode: "http", url: "not-a-url" } });
      check("pidock.settings.set rejects bad url", false);
    } catch (e) {
      check("pidock.settings.set rejects bad url", String(e).includes("bad_request"));
    }
    await request("pidock.settings.set", {
      proxy: { mode: "http", url: "http://127.0.0.1:7890", noProxy: "localhost,127.0.0.1,.internal", caPath: "C:\\ca.pem" },
    });
    const appAfter = await request("pidock.settings.get");
    check(
      "pidock.settings.set/get roundtrip",
      appAfter.settings?.proxy?.mode === "http" &&
        appAfter.settings?.proxy?.url === "http://127.0.0.1:7890" &&
        appAfter.settings?.proxy?.noProxy === "localhost,127.0.0.1,.internal" &&
        appAfter.settings?.proxy?.caPath === "C:\\ca.pem",
      JSON.stringify(appAfter.settings?.proxy),
    );
    await request("pidock.settings.set", { proxy: { mode: "direct" } });
    const appDirect = await request("pidock.settings.get");
    check(
      "pidock.settings proxy switch to direct drops fields",
      appDirect.settings?.proxy?.mode === "direct" &&
        appDirect.settings?.proxy?.url === undefined,
    );

    // ---- 安装：本地导入（插件单文件 + 技能压缩包） ----
    const importDir = mkdtempSync(join(tmpdir(), "pidock-import-"));
    const pluginSrc = join(importDir, "smoke-plug.ts");
    writeFileSync(pluginSrc, "export default function () {};\n");
    const extInstall = await request("config.extensions.install", { srcPath: pluginSrc });
    check(
      "extensions.install local copy",
      extInstall.file?.endsWith("smoke-plug.ts") &&
        readFileSync(join(agentDir, "extensions", "smoke-plug.ts"), "utf8").includes("export default"),
    );
    try {
      await request("config.extensions.install", { srcPath: pluginSrc });
      check("extensions.install rejects duplicate", false);
    } catch (e) {
      check("extensions.install rejects duplicate", String(e).includes("already_exists"));
    }

    // 打一个 tgz：smoke-skill/SKILL.md（系统自带 bsdtar）
    const skillRoot = join(importDir, "smoke-skill");
    mkdirSync(skillRoot, { recursive: true });
    writeFileSync(
      join(skillRoot, "SKILL.md"),
      "---\nname: smoke-skill\ndescription: installed by config smoke\n---\n\nHi.\n",
    );
    const tgzPath = join(importDir, "smoke-skill.tgz");
    const { execFileSync } = await import("node:child_process");
    // cwd 相对路径：PATH 上的 GNU tar 会把含盘符的 -f 参数当远程主机
    execFileSync("tar", ["-czf", "smoke-skill.tgz", "smoke-skill"], { cwd: importDir });
    const skillInstall = await request("config.skills.install", { srcPath: tgzPath });
    check(
      "skills.install local tgz",
      skillInstall.name === "smoke-skill" &&
        readFileSync(join(agentDir, "skills", "smoke-skill", "SKILL.md"), "utf8").includes("installed by config smoke"),
    );
    const skillsAfter = await request("config.skills.list", { cwd: workDir });
    check(
      "skills.install visible in skills.list",
      skillsAfter.skills.some((s: any) => s.name === "smoke-skill" && s.enabled === true),
    );
    rmSync(importDir, { recursive: true, force: true });

    clearTimeout(timeout);
    child.stdin.end();
    child.kill();
    rmSync(agentDir, { recursive: true, force: true });
    rmSync(workDir, { recursive: true, force: true });
  } catch (err) {
    failures.push(String(err instanceof Error ? err.message : err));
  }

  console.log("== summary ==");
  if (failures.length) {
    console.log(`FAIL (${failures.length}): ${failures.join("; ")}`);
    return 1;
  }
  console.log("PASS — config surface fully green");
  return 0;
}

main().then((code) => process.exit(code));
