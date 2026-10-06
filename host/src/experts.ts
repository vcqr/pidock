import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { basename, join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { Expert } from "@pidock/protocol";
import { RpcError } from "./pool.js";
import { BINARY_EXTENSIONS, downloadToBuffer, installSkillFromBuffer, looksBinary, sanitizeInstallName } from "./archive.js";

/**
 * 专家（智能体编排）存储与解析。
 *
 * 专家 = 预编排的智能体档案：角色提示词（append 到 pi 默认系统提示词之后）、
 * 全局技能/插件白名单、内置工具增减、知识库目录、默认模型/思考级别/权限模式。
 *
 * 存储布局（PiDock 专属文件区，不与 pi 的配置混放）：
 * - ~/.pi/agent/pidock/experts.json          专家档案数组
 * - ~/.pi/agent/pidock/experts/<id>/skills/  专家私有技能（每技能一个含 SKILL.md 的目录）
 * - ~/.pi/agent/pidock/experts/<id>/extensions/  专家私有插件（.ts/.js 单文件，MCP 也走这里）
 */

function expertsFilePath(): string {
  return join(getAgentDir(), "pidock", "experts.json");
}

/** 专家私有资源根目录 */
export function expertDir(id: string): string {
  return join(getAgentDir(), "pidock", "experts", id);
}

function readExperts(): Expert[] {
  try {
    const raw = JSON.parse(readFileSync(expertsFilePath(), "utf8"));
    return Array.isArray(raw) ? (raw as Expert[]) : [];
  } catch {
    return [];
  }
}

function writeExperts(list: Expert[]): void {
  const file = expertsFilePath();
  mkdirSync(join(file, ".."), { recursive: true });
  writeFileSync(file, JSON.stringify(list, null, 2) + "\n");
}

const KB_MAX_DEPTH = 2;
const KB_MAX_FILES_PER_DIR = 50;
const KB_MAX_TOTAL_LINES = 120;

function binaryExtension(file: string): boolean {
  const base = file.slice(file.lastIndexOf(".") + 1).toLowerCase();
  return Boolean(base) && BINARY_EXTENSIONS.has(base);
}

/**
 * 知识库清单：只列文件路径，不注入内容——模型按需用 read 读取，
 * token 花在刀刃上（与 agentsFilesOverride 全量注入相对的设计取舍）。
 */
export function buildKnowledgeManifest(dirs: string[]): string | null {
  const usable = (dirs ?? []).filter((d) => typeof d === "string" && d.trim() && existsSync(d.trim()) && statSync(d.trim()).isDirectory());
  if (usable.length === 0) return null;
  const lines: string[] = [];
  for (const dir of usable) {
    const root = dir.trim();
    lines.push(`目录 ${root}:`);
    let count = 0;
    const walk = (cur: string, rel: string, depth: number): void => {
      if (count >= KB_MAX_FILES_PER_DIR || lines.length >= KB_MAX_TOTAL_LINES) return;
      let entries;
      try {
        entries = readdirSync(cur, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of entries) {
        if (count >= KB_MAX_FILES_PER_DIR || lines.length >= KB_MAX_TOTAL_LINES) return;
        if (e.name.startsWith(".") || e.name === "node_modules" || e.name === "target") continue;
        const abs = join(cur, e.name);
        const relPath = rel ? `${rel}/${e.name}` : e.name;
        if (e.isDirectory()) {
          if (depth < KB_MAX_DEPTH) walk(abs, relPath, depth + 1);
        } else if (e.isFile() && !binaryExtension(e.name)) {
          lines.push(`- ${abs}`);
          count++;
        }
      }
    };
    walk(root, "", 0);
  }
  if (lines.length === 0) return null;
  return [
    "# 知识库",
    "以下是本专家可用的知识文件清单。按需用 read 工具读取与当前任务相关的文件，不要一次全部读入：",
    ...lines,
  ].join("\n");
}

/** /expert:name 命令的消息注入块（与 pi 的 /skill: 展开同构，整条消息替换并持久化）。
 * 段序固定：专家块 → 用户指令 → 知识库清单（UI 折叠渲染按 "\n\n# 知识库" 切分参数与清单）。 */
export function buildExpertPromptBlock(expert: Expert, args: string | undefined): string {
  const parts: string[] = [];
  parts.push(`<expert name="${expert.name}" location="pidock-experts">\n${expert.prompt || "（该专家未填写角色提示词）"}\n</expert>`);
  const rest = (args ?? "").trim();
  parts.push(rest || "请以该专家的身份与视角处理本次对话。");
  const kb = buildKnowledgeManifest(expert.knowledge_dirs ?? []);
  if (kb) parts.push(kb);
  return parts.join("\n\n");
}

/** 匹配消息开头的 /expert:name [args]；名称解析到空白符为止 */
export function matchExpertCommand(text: string): { name: string; args?: string } | null {
  const m = /^\/expert:(\S+)(?:\s+([\s\S]*))?$/.exec(text.trim());
  if (!m) return null;
  return { name: m[1] ?? "", args: m[2] };
}

/** 会话创建用的专家解析结果（pool.ts 据此构建 DefaultResourceLoader 覆盖项） */
export interface ExpertSessionConfig {
  expert: Expert;
  /** 追加到默认系统提示词之后的段落（角色提示词 + 知识库清单） */
  appendSystemPrompt: string[];
  /** 专家私有技能根目录（可不存在 → 空数组） */
  additionalSkillPaths: string[];
  /** 专家私有插件文件绝对路径 */
  additionalExtensionPaths: string[];
  /** null = 不限制 */
  skillAllowlist: string[] | null;
  extensionAllowlist: string[] | null;
  tools?: string[];
  excludeTools?: string[];
}

export class ExpertsService {
  list(): { experts: Expert[] } {
    return { experts: readExperts().sort((a, b) => a.name.localeCompare(b.name)) };
  }

  get(params: { id?: string; name?: string }): { expert: Expert | null } {
    const list = readExperts();
    const found = params.id
      ? list.find((e) => e.id === params.id)
      : params.name
        ? list.find((e) => e.name.toLowerCase() === params.name!.trim().toLowerCase())
        : undefined;
    return { expert: found ?? null };
  }

  save(params: Partial<Expert> & { name?: string }): { expert: Expert; private_dir: string } {
    const name = (params.name ?? "").trim();
    if (!name || /\s/.test(name)) {
      throw new RpcError("bad_request", "专家名称必填且不能含空白（它也是 /expert:name 的调用名）");
    }
    if (!params.prompt && !(params.tools?.length || params.exclude_tools?.length || params.knowledge_dirs?.length || params.skills?.length || params.extensions?.length)) {
      throw new RpcError("bad_request", "专家至少要有角色提示词或一项资源配置");
    }
    const list = readExperts();
    const dup = list.find((e) => e.name.toLowerCase() === name.toLowerCase() && e.id !== params.id);
    if (dup) throw new RpcError("already_exists", `专家「${name}」已存在`);
    const now = new Date().toISOString();
    const existing = params.id ? list.find((e) => e.id === params.id) : undefined;
    if (params.id && !existing) throw new RpcError("not_found", "expert not found");
    const expert: Expert = {
      id: existing?.id ?? randomUUID(),
      name,
      description: params.description?.trim() || undefined,
      icon: params.icon?.trim() || undefined,
      prompt: typeof params.prompt === "string" ? params.prompt : "",
      skills: Array.isArray(params.skills) ? params.skills.map(String) : [],
      extensions: Array.isArray(params.extensions) ? params.extensions.map(String) : [],
      tools: Array.isArray(params.tools) && params.tools.length ? params.tools.map(String) : undefined,
      exclude_tools: Array.isArray(params.exclude_tools) && params.exclude_tools.length ? params.exclude_tools.map(String) : undefined,
      knowledge_dirs: Array.isArray(params.knowledge_dirs) ? params.knowledge_dirs.map((d) => String(d).trim()).filter(Boolean) : [],
      // model/thinking_level/permission_mode 已从编辑器 UI 移除（归属会话层配置）；
      // 字段保留兼容旧数据，参数缺省时保留既有值不被覆盖
      model: params.model !== undefined ? params.model.trim() || undefined : existing?.model,
      thinking_level: params.thinking_level !== undefined ? params.thinking_level.trim() || undefined : existing?.thinking_level,
      permission_mode: params.permission_mode ?? existing?.permission_mode,
      created_at: existing?.created_at ?? now,
      updated_at: now,
    };
    const next = existing ? list.map((e) => (e.id === expert.id ? expert : e)) : [...list, expert];
    writeExperts(next);
    mkdirSync(expertDir(expert.id), { recursive: true });
    return { expert, private_dir: expertDir(expert.id) };
  }

  delete(params: { id: string }): { ok: true } {
    const list = readExperts();
    const found = list.find((e) => e.id === params.id);
    if (!found) throw new RpcError("not_found", "expert not found");
    writeExperts(list.filter((e) => e.id !== params.id));
    // 私有资源随专家删除（档案已不可达，资源成了孤儿）
    rmSync(expertDir(params.id), { recursive: true, force: true });
    return { ok: true };
  }

  // ------------------------------------------------------------ 私有资源

  async installResource(params: {
    expert_id: string;
    kind: "skill" | "extension";
    url?: string;
    srcPath?: string;
    name?: string;
  }): Promise<{ ok: true; name: string; path: string }> {
    const expert = readExperts().find((e) => e.id === params.expert_id);
    if (!expert) throw new RpcError("not_found", "expert not found");
    if (params.kind === "skill") {
      let buf: Buffer;
      let baseName: string;
      if (params.srcPath) {
        const src = params.srcPath.trim();
        if (!src || !existsSync(src) || !statSync(src).isFile()) throw new RpcError("bad_request", "srcPath 文件不存在");
        buf = readFileSync(src);
        baseName = basename(src) || "skill";
      } else if (params.url) {
        buf = await downloadToBuffer(params.url.trim());
        baseName = new URL(params.url).pathname.split("/").filter(Boolean).pop() || "skill";
      } else {
        throw new RpcError("bad_request", "url 与 srcPath 至少提供一个");
      }
      const root = join(expertDir(expert.id), "skills");
      return { ok: true, ...(await installSkillFromBuffer(buf, baseName, root, params.name)) };
    }
    // extension：单个 .ts/.js 文本文件
    let text: string;
    let baseName: string;
    if (params.srcPath) {
      const src = params.srcPath.trim();
      if (!src || !existsSync(src) || !statSync(src).isFile()) throw new RpcError("bad_request", "srcPath 文件不存在");
      const raw = readFileSync(src);
      if (looksBinary(raw)) throw new RpcError("bad_request", "插件必须是文本文件（.ts/.js）");
      text = raw.toString("utf8");
      baseName = basename(src) || "plugin.ts";
    } else if (params.url) {
      const buf = await downloadToBuffer(params.url.trim());
      if (looksBinary(buf)) throw new RpcError("bad_request", "下载内容不是文本文件（.ts/.js）");
      text = buf.toString("utf8");
      baseName = new URL(params.url).pathname.split("/").filter(Boolean).pop() || "plugin.ts";
    } else {
      throw new RpcError("bad_request", "url 与 srcPath 至少提供一个");
    }
    const filename = sanitizeInstallName(params.name || baseName);
    if (!/\.(ts|js)$/i.test(filename)) throw new RpcError("bad_request", "插件文件必须是 .ts 或 .js");
    if (text.length > 2 * 1024 * 1024) throw new RpcError("too_large", "插件超过 2MB 上限");
    const dir = join(expertDir(expert.id), "extensions");
    mkdirSync(dir, { recursive: true });
    const target = join(dir, filename);
    if (existsSync(target)) throw new RpcError("already_exists", `插件「${filename.replace(/\.(ts|js)$/i, "")}」已存在`);
    writeFileSync(target, text);
    return { ok: true, name: filename.replace(/\.(ts|js)$/i, ""), path: target };
  }

  privateList(params: { expert_id: string }): {
    skills: Array<{ name: string; description: string; path: string }>;
    extensions: Array<{ name: string; path: string }>;
  } {
    const dir = expertDir(params.expert_id);
    const skills: Array<{ name: string; description: string; path: string }> = [];
    const extensions: Array<{ name: string; path: string }> = [];
    const skillsRoot = join(dir, "skills");
    if (existsSync(skillsRoot)) {
      for (const entry of readdirSync(skillsRoot, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const skillFile = join(skillsRoot, entry.name, "SKILL.md");
        if (!existsSync(skillFile)) continue;
        let description = "";
        try {
          const head = readFileSync(skillFile, "utf8").slice(0, 2048);
          description = /^description:\s*(.+)$/m.exec(head)?.[1]?.trim() ?? "";
        } catch {
          // 读不出来的技能照样列出
        }
        skills.push({ name: entry.name, description, path: skillFile });
      }
    }
    const extRoot = join(dir, "extensions");
    if (existsSync(extRoot)) {
      for (const entry of readdirSync(extRoot, { withFileTypes: true })) {
        if (entry.isFile() && /\.(ts|js)$/i.test(entry.name)) {
          extensions.push({ name: entry.name.replace(/\.(ts|js)$/i, ""), path: join(extRoot, entry.name) });
        }
      }
    }
    return { skills, extensions };
  }

  removeResource(params: { expert_id: string; kind: "skill" | "extension"; name: string }): { ok: true } {
    const dir = join(expertDir(params.expert_id), params.kind === "skill" ? "skills" : "extensions");
    const name = sanitizeInstallName(params.name);
    const target = params.kind === "skill" ? join(dir, name) : join(dir, `${name}.ts`);
    const alt = join(dir, `${name}.js`);
    if (existsSync(target)) {
      rmSync(target, { recursive: true, force: true });
    } else if (existsSync(alt)) {
      rmSync(alt, { force: true });
    } else {
      throw new RpcError("not_found", "私有资源不存在");
    }
    return { ok: true };
  }

  // ------------------------------------------------------------ 会话解析

  /** 解析专家为会话创建参数；专家不存在或无有效配置时返回 null（回退普通会话） */
  resolveForSession(id: string | undefined): ExpertSessionConfig | null {
    if (!id) return null;
    const expert = readExperts().find((e) => e.id === id);
    if (!expert) return null;
    const dir = expertDir(expert.id);
    const skillsRoot = join(dir, "skills");
    const extRoot = join(dir, "extensions");
    const skillPaths = existsSync(skillsRoot) ? [skillsRoot] : [];
    const extPaths = existsSync(extRoot)
      ? readdirSync(extRoot, { withFileTypes: true })
          .filter((e) => e.isFile() && /\.(ts|js)$/i.test(e.name))
          .map((e) => join(extRoot, e.name))
      : [];
    const sections: string[] = [];
    if (expert.prompt.trim()) {
      sections.push(`# 专家角色：${expert.name}\n\n${expert.prompt.trim()}`);
    }
    const kb = buildKnowledgeManifest(expert.knowledge_dirs ?? []);
    if (kb) sections.push(kb);
    return {
      expert,
      appendSystemPrompt: sections,
      additionalSkillPaths: skillPaths,
      additionalExtensionPaths: extPaths,
      skillAllowlist: expert.skills.length ? expert.skills : null,
      extensionAllowlist: expert.extensions.length ? expert.extensions : null,
      tools: expert.tools?.length ? expert.tools : undefined,
      excludeTools: expert.exclude_tools?.length ? expert.exclude_tools : undefined,
    };
  }
}
