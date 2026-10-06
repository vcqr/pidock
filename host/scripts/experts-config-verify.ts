/**
 * 专家配置生效性验证：直连 SDK 按	pool.ts 的方式构建专家会话
 * （appendSystemPrompt + skillsOverride + extensionsOverride + excludeTools），
 * 断言系统提示词包含角色文本、工具集确实排除了被禁工具。
 * 不发 LLM 请求，只看会话构建结果。
 *
 * Run: bun run scripts/experts-config-verify.ts
 */
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createAgentSession,
  DefaultResourceLoader,
  SessionManager,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";

async function main(): Promise<number> {
  const agentDir = mkdtempSync(join(tmpdir(), "pidock-verify-agent-"));
  const cwd = mkdtempSync(join(tmpdir(), "pidock-verify-work-"));
  const failures: string[] = [];
  const check = (name: string, cond: boolean, detail = "") => {
    console.log(`${cond ? "  ✔" : "  ✘"} ${name}${detail ? ` — ${detail}` : ""}`);
    if (!cond) failures.push(name);
  };

  try {
    // 植入全局技能与专家私有技能
    mkdirSync(join(agentDir, "skills", "g-skill"), { recursive: true });
    writeFileSync(join(agentDir, "skills", "g-skill", "SKILL.md"), "---\nname: g-skill\ndescription: global\n---\nx\n");
    const privateSkills = join(agentDir, "expert-skills");
    mkdirSync(join(privateSkills, "p-skill"), { recursive: true });
    writeFileSync(join(privateSkills, "p-skill", "SKILL.md"), "---\nname: p-skill\ndescription: private\n---\nx\n");

    const loader = new DefaultResourceLoader({
      cwd,
      agentDir,
      settingsManager: SettingsManager.create(cwd, agentDir),
      appendSystemPrompt: ["# 专家角色：tester\n\n你是测试专家，必须始终以该身份工作。"],
      additionalSkillPaths: [privateSkills],
      skillsOverride: (base) => ({
        ...base,
        skills: base.skills.filter((s) => s.name === "g-skill" || s.name === "p-skill"),
      }),
    });
    await loader.reload();

    const { session } = await createAgentSession({
      cwd,
      sessionManager: SessionManager.create(cwd),
      resourceLoader: loader,
      excludeTools: ["bash", "edit", "write"],
    });

    const sp = session.systemPrompt;
    check("系统提示词包含角色文本", sp.includes("专家角色：tester") && sp.includes("必须始终以该身份工作"));
    check("appendSystemPrompt 在默认提示词之后（保留了 pi 底座）", sp.length > 500 && sp.indexOf("专家角色：tester") > sp.indexOf("read"));

    const names: string[] = (session as any).getActiveToolNames?.() ?? [];
    check("工具集可读", names.length > 0, `tools=${JSON.stringify(names)}`);
    check("bash/edit/write 已被排除", !names.includes("bash") && !names.includes("edit") && !names.includes("write"), `tools=${names.join(",")}`);

    const skills = loader.getSkills().skills.map((s) => s.name);
    check("技能过滤生效（全局+私有）", skills.includes("g-skill") && skills.includes("p-skill"), `skills=${skills.join(",")}`);
  } finally {
    rmSync(agentDir, { recursive: true, force: true });
    rmSync(cwd, { recursive: true, force: true });
  }

  console.log("== summary ==");
  if (failures.length) {
    console.log(`FAIL (${failures.length}): ${failures.join("; ")}`);
    return 1;
  }
  console.log("PASS — 专家配置在 SDK 层真实生效");
  return 0;
}

main().then((code) => process.exit(code));
