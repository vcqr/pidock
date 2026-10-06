/**
 * Experts smoke: exercises the experts.* surface + expert-bound session
 * creation against an isolated agent dir (no real LLM call). Verifies CRUD,
 * private skill/extension install, registry round-trip on reopen, and the
 * loader override wiring (additionalSkillPaths/extensionsOverride run inside
 * DefaultResourceLoader.reload during session.create/open).
 *
 * Run: pnpm --filter @pidock/host smoke:experts
 */
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HOST_BIN = process.env.PIDOCK_HOST_BIN ?? "../target/pidock-host.exe";
const HOST_ARGS = (process.env.PIDOCK_HOST_ARGS ?? "").split(" ").filter(Boolean);

async function main(): Promise<number> {
  const agentDir = mkdtempSync(join(tmpdir(), "pidock-exp-agent-"));
  const workDir = mkdtempSync(join(tmpdir(), "pidock-exp-work-"));
  const failures: string[] = [];
  const check = (name: string, cond: boolean, detail = "") => {
    console.log(`${cond ? "  ✔" : "  ✘"} ${name}${detail ? ` — ${detail}` : ""}`);
    if (!cond) failures.push(name);
  };

  // plant: global skill + global extension + knowledge dir
  mkdirSync(join(agentDir, "extensions"), { recursive: true });
  writeFileSync(join(agentDir, "extensions", "g-ext.ts"), "export default function () {};\n");
  mkdirSync(join(agentDir, "skills", "g-skill"), { recursive: true });
  writeFileSync(
    join(agentDir, "skills", "g-skill", "SKILL.md"),
    "---\nname: g-skill\ndescription: global skill planted by experts smoke\n---\n\nDo nothing.\n",
  );
  const kbDir = join(agentDir, "kb");
  mkdirSync(join(kbDir, "notes"), { recursive: true });
  writeFileSync(join(kbDir, "notes", "arch.md"), "# 架构\n知识库内容，专家应按需读取。\n");
  writeFileSync(join(kbDir, "binary.dat"), Buffer.from([0x00, 0x01, 0x02]));

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

  const timeout = setTimeout(() => {
    failures.push("timeout");
    child.stdin.end();
    child.kill();
  }, 120_000);

  function request(method: string, params: unknown = {}): Promise<any> {
    const id = Math.random().toString(36).slice(2);
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      child.stdin.write(JSON.stringify({ id, method, params }) + "\n");
    });
  }

  try {
    await request("ping");

    // ---- CRUD
    const saved = await request("experts.save", {
      name: "code-reviewer",
      description: "只读代码评审专家",
      prompt: "你是资深代码评审专家，只分析不修改代码。",
      skills: ["g-skill"],
      exclude_tools: ["bash", "edit", "write"],
      knowledge_dirs: [kbDir],
    });
    const expert = saved.expert;
    check("experts.save", Boolean(expert?.id) && existsSync(saved.private_dir));

    const dup = await request("experts.save", { name: "code-reviewer", prompt: "x" }).then(
      () => false,
      (e: Error) => e.message.startsWith("already_exists"),
    );
    check("experts.save duplicate name rejected", dup === true);

    const got = await request("experts.get", { name: "CODE-REVIEWER" });
    check("experts.get by name (case-insensitive)", got.expert?.id === expert.id);

    const listed = await request("experts.list");
    check("experts.list", listed.experts.some((e: any) => e.id === expert.id));

    // ---- 私有资源：技能 tgz + 插件 srcPath
    const importDir = mkdtempSync(join(tmpdir(), "pidock-exp-import-"));
    mkdirSync(join(importDir, "p-skill"), { recursive: true });
    writeFileSync(
      join(importDir, "p-skill", "SKILL.md"),
      "---\nname: p-skill\ndescription: private skill installed by experts smoke\n---\n\nDo nothing.\n",
    );
    // cwd 相对路径打 tgz（PATH 上的 GNU tar 会把含盘符的 -f 参数当远程主机）
    execFileSync("tar", ["-czf", "p-skill.tgz", "p-skill"], { cwd: importDir });
    const extFile = join(importDir, "p-ext.ts");
    writeFileSync(extFile, "export default function () {};\n");

    const installedSkill = await request("experts.install_resource", {
      expert_id: expert.id,
      kind: "skill",
      srcPath: join(importDir, "p-skill.tgz"),
    });
    check(
      "experts.install_resource skill",
      installedSkill.name === "p-skill" &&
        existsSync(join(saved.private_dir, "skills", "p-skill", "SKILL.md")),
    );
    const installedExt = await request("experts.install_resource", {
      expert_id: expert.id,
      kind: "extension",
      srcPath: extFile,
    });
    check("experts.install_resource extension", installedExt.name === "p-ext");

    const priv = await request("experts.private_list", { expert_id: expert.id });
    check(
      "experts.private_list",
      priv.skills.some((s: any) => s.name === "p-skill" && s.description.includes("private skill")) &&
        priv.extensions.some((e: any) => e.name === "p-ext"),
    );

    // ---- 专家会话：创建 + 重开（loader 覆盖在 reload 中真实执行）
    const created = await request("session.create", { expert_id: expert.id });
    check(
      "session.create(expert) returns expert info",
      created.expert_id === expert.id && created.expert_name === "code-reviewer",
    );
    const sessions = await request("session.list");
    const summary = sessions.sessions.find((s: any) => s.session_id === created.session_id);
    check("session.list carries expert fields", summary?.expert_id === expert.id && summary?.expert_name === "code-reviewer");

    const reopened = await request("session.open", { file: created.file });
    check(
      "session.open replays expert config",
      reopened.expert_id === expert.id && reopened.expert_name === "code-reviewer",
    );

    // 普通会话不受影响
    const plain = await request("session.create", {});
    check("session.create without expert", !plain.expert_id && !plain.expert_name);

    // ---- 删除专家（私有资源随之清除）
    await request("experts.delete", { id: expert.id });
    check("experts.delete removes private dir", !existsSync(saved.private_dir));
    const afterDelete = await request("experts.list");
    check("experts.delete removes entry", !afterDelete.experts.some((e: any) => e.id === expert.id));

    // ---- 头像：read_avatar_file + save 携带 data URL
    const pngPath = join(agentDir, "avatar.png");
    writeFileSync(pngPath, Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64"));
    const avatarRead = await request("experts.read_avatar_file", { srcPath: pngPath });
    check(
      "experts.read_avatar_file returns data URL",
      typeof avatarRead.data_url === "string" && avatarRead.data_url.startsWith("data:image/png;base64,"),
    );
    const badAvatar = await request("experts.read_avatar_file", { srcPath: join(agentDir, "extensions", "g-ext.ts") }).then(
      () => false,
      () => true,
    );
    check("read_avatar_file rejects non-image", badAvatar === true);

    const withAvatar = await request("experts.save", {
      name: "avatar-expert",
      prompt: "头像测试",
      avatar: avatarRead.data_url,
      avatar_color: "#10B981",
    });
    check("experts.save stores avatar + color", Boolean(withAvatar.expert.avatar?.startsWith("data:image/png")) && withAvatar.expert.avatar_color === "#10b981");
    const badSave = await request("experts.save", { name: "bad-avatar", prompt: "x", avatar: "http://not-a-data-url" }).then(
      () => false,
      () => true,
    );
    check("experts.save rejects non-data-url avatar", badSave === true);
    // 缺省 avatar 参数 → 保留既有值；空串 → 清除
    const kept = await request("experts.save", { id: withAvatar.expert.id, name: "avatar-expert", prompt: "改个提示词" });
    check("save without avatar keeps existing", Boolean(kept.expert.avatar));
    const cleared = await request("experts.save", { id: withAvatar.expert.id, name: "avatar-expert", prompt: "改个提示词", avatar: "" });
    check("save with empty avatar clears it", !cleared.expert.avatar);
    await request("experts.delete", { id: withAvatar.expert.id });

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
  console.log("PASS — experts surface fully green");
  return 0;
}

main().then((code) => process.exit(code));
