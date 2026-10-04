import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { getAgentDir, loadSkillsFromDir } from "@earendil-works/pi-coding-agent";
import { RpcError, type SessionPool } from "./pool.js";

/**
 * config.* command implementations — the desktop settings center backend.
 *
 * Everything here operates on pi's own config files so the desktop app and
 * the pi CLI stay interchangeable:
 * - ~/.pi/agent/settings.json  (settings, packages, extension/skill globs)
 * - ~/.pi/agent/auth.json      (credentials, {providerId: Credential})
 * - ~/.pi/agent/models.json    (custom providers/models)
 * - ~/.pi/agent/mcp.json       (MCP extension config; MCP itself is
 *                               extension-provided in pi)
 */

type Json = Record<string, any>;

function readJson(path: string): Json | null {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
}

function writeJson(path: string, data: unknown): void {
  writeFileSync(path, JSON.stringify(data, null, 2) + "\n");
}

/** keys the desktop settings center may write into settings.json */
const SETTING_KEYS = new Set([
  "defaultProvider",
  "defaultModel",
  "defaultThinkingLevel",
  "theme",
  "defaultProjectTrust",
  "hideThinkingBlock",
  "enableSkillCommands",
  "quietStartup",
  "extensions",
  "skills",
  "prompts",
  "themes",
  "packages",
]);

const SETTING_KEYS_NEEDING_ARRAY = new Set(["extensions", "skills", "prompts", "themes", "packages"]);

/** caps for the skill detail file browser */
const MAX_SKILL_FILES = 200;
const MAX_SKILL_DEPTH = 3;
const MAX_SKILL_FILE_CHARS = 512 * 1024;

/** extensions whose content is never shown as text in the skill detail reader */
const BINARY_EXTENSIONS = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "avif",
  "pdf", "zip", "gz", "tgz", "bz2", "7z", "rar", "xz", "tar",
  "exe", "dll", "so", "dylib", "bin", "o", "a", "lib", "wasm",
  "class", "jar", "pyc", "pyo",
  "ttf", "otf", "woff", "woff2", "eot",
  "mp3", "mp4", "mov", "avi", "mkv", "webm", "ogg", "wav", "flac",
  "db", "sqlite", "sqlite3", "dat", "dmg", "iso", "msi",
]);

function binaryExtension(file: string): boolean {
  const base = file.slice(file.lastIndexOf("/") + 1);
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return false;
  return BINARY_EXTENSIONS.has(base.slice(dot + 1).toLowerCase());
}

/** NUL-byte heuristic, same spirit as git's binary detection */
function looksBinary(buf: Buffer): boolean {
  const n = Math.min(buf.length, 8192);
  for (let i = 0; i < n; i++) {
    if (buf[i] === 0) return true;
  }
  return false;
}

/** text extensions the plugin detail reader may display */
const EXT_READ_EXTENSIONS = new Set([
  "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "json", "md", "txt",
]);

function settingsPath(): string {
  return join(getAgentDir(), "settings.json");
}

function hasExclusion(settings: Json, listKey: string, absPath: string): boolean {
  const list = settings[listKey];
  if (!Array.isArray(list)) return false;
  const normalized = absPath.replace(/\\/g, "/");
  return list.some(
    (entry) => typeof entry === "string" && entry.startsWith("-") && normalized.includes(entry.slice(1)),
  );
}

/** add/remove a `-path` exclusion entry, creating the array when missing */
function toggleExclusion(listKey: string, absPath: string, disable: boolean): void {
  const path = settingsPath();
  const settings = readJson(path) ?? {};
  const list = Array.isArray(settings[listKey]) ? [...settings[listKey]] : [];
  const normalized = absPath.replace(/\\/g, "/");
  const filtered = list.filter((entry) => {
    if (typeof entry !== "string" || !entry.startsWith("-")) return true;
    return !normalized.includes(entry.slice(1));
  });
  if (disable) filtered.push(`-${normalized}`);
  if (filtered.length > 0 || list.length > 0) {
    settings[listKey] = filtered;
  }
  writeJson(path, settings);
}

export class ConfigService {
  constructor(private pool: SessionPool) {}

  get(): { settings: Json | null; models: Json | null; mcp: Json | null; agent_dir: string } {
    const dir = getAgentDir();
    return {
      settings: readJson(join(dir, "settings.json")),
      models: readJson(join(dir, "models.json")),
      mcp: readJson(join(dir, "mcp.json")),
      agent_dir: dir,
    };
  }

  set(params: { patch: Json }): { ok: true } {
    if (!params.patch || typeof params.patch !== "object") {
      throw new RpcError("bad_request", "patch must be an object");
    }
    const unknown = Object.keys(params.patch).filter((k) => !SETTING_KEYS.has(k));
    if (unknown.length > 0) {
      throw new RpcError("unknown_setting", `not writable: ${unknown.join(", ")}`);
    }
    const path = settingsPath();
    const settings = readJson(path) ?? {};
    for (const [key, value] of Object.entries(params.patch)) {
      if (SETTING_KEYS_NEEDING_ARRAY.has(key) && !Array.isArray(value)) {
        throw new RpcError("bad_request", `${key} must be an array`);
      }
      settings[key] = value;
    }
    writeJson(path, settings);
    return { ok: true };
  }

  async providersList(): Promise<{
    providers: Array<{ id: string; auth: string; models: number; default: boolean }>;
  }> {
    const mr = await this.pool.modelRuntime();
    const settings = readJson(settingsPath()) ?? {};
    const providers = mr.getProviders().map((p: any) => ({
      id: p.id,
      auth: "unknown",
      models: mr.getModels(p.id).length,
      default: settings.defaultProvider === p.id,
    }));
    for (const p of providers) {
      const status = mr.getProviderAuthStatus(p.id) as
        | { configured: boolean; source?: string; label?: string }
        | undefined;
      p.auth = !status || !status.configured ? "missing" : (status.source ?? "stored");
    }
    return { providers };
  }

  async providerSetKey(params: { provider: string; key: string }): Promise<{ ok: true }> {
    if (!params.provider || !params.key) {
      throw new RpcError("bad_request", "provider and key are required");
    }
    const authPath = join(getAgentDir(), "auth.json");
    const data = readJson(authPath) ?? {};
    data[params.provider] = { type: "api_key", key: params.key };
    writeJson(authPath, data);
    // refresh the runtime's view of credentials
    await this.pool.modelRuntime().catch(() => {});
    return { ok: true };
  }

  providerRemoveKey(params: { provider: string }): { ok: true } {
    const authPath = join(getAgentDir(), "auth.json");
    const data = readJson(authPath) ?? {};
    delete data[params.provider];
    writeJson(authPath, data);
    return { ok: true };
  }

  async modelsList(): Promise<{
    models: Array<{ provider: string; id: string; name: string; reasoning: boolean; input: string[]; contextWindow?: number; maxTokens?: number }>;
  }> {
    const mr = await this.pool.modelRuntime();
    const models = mr.getModels().map((m: any) => ({
      provider: m.provider,
      id: m.id,
      name: m.name ?? m.id,
      reasoning: Boolean(m.reasoning),
      input: Array.isArray(m.input) ? m.input.map(String) : ["text"],
      contextWindow: typeof m.contextWindow === "number" ? m.contextWindow : undefined,
      maxTokens: typeof m.maxTokens === "number" ? m.maxTokens : undefined,
    }));
    return { models };
  }

  modelsSetDefault(params: { provider: string; model: string }): { ok: true } {
    if (!params.provider || !params.model) {
      throw new RpcError("bad_request", "provider and model are required");
    }
    const path = settingsPath();
    const settings = readJson(path) ?? {};
    settings.defaultProvider = params.provider;
    settings.defaultModel = params.model;
    writeJson(path, settings);
    return { ok: true };
  }

  extensionsList(params: { cwd?: string }): {
    extensions: Array<{ name: string; file: string; scope: string; enabled: boolean }>;
  } {
    const dir = getAgentDir();
    const settings = readJson(settingsPath()) ?? {};
    const out: Array<{ name: string; file: string; scope: string; enabled: boolean }> = [];
    const scan = (root: string, scope: string): void => {
      if (!existsSync(root)) return;
      for (const entry of readdirSync(root, { withFileTypes: true })) {
        if (!entry.isFile() || !/\.(ts|js)$/.test(entry.name)) continue;
        const file = join(root, entry.name);
        out.push({
          name: entry.name.replace(/\.(ts|js)$/, ""),
          file,
          scope,
          enabled: !hasExclusion(settings, "extensions", file),
        });
      }
    };
    scan(join(dir, "extensions"), "global");
    if (params.cwd) scan(join(params.cwd, ".pi", "extensions"), "project");
    return { extensions: out };
  }

  extensionsToggle(params: { file: string; enabled: boolean }): { ok: true } {
    if (!params.file) throw new RpcError("bad_request", "file is required");
    toggleExclusion("extensions", params.file, !params.enabled);
    return { ok: true };
  }

  /**
   * Read the source of an extension entry file (path as returned by
   * extensions.list). Binary content is rejected, never shipped as garbage.
   */
  extensionsRead(params: { file: string }): { text: string } {
    const file = (params.file ?? "").trim();
    const dot = file.lastIndexOf(".");
    const ext = dot > 0 ? file.slice(dot + 1).toLowerCase() : "";
    if (!file || !EXT_READ_EXTENSIONS.has(ext)) {
      throw new RpcError("bad_request", "file must be a text source file (.ts/.js/…)");
    }
    const abs = resolve(file);
    if (!existsSync(abs) || !statSync(abs).isFile()) {
      throw new RpcError("not_found", "extension file not found");
    }
    const buf = readFileSync(abs);
    if (looksBinary(buf)) {
      throw new RpcError("binary_file", "binary file content is not displayed");
    }
    let text = buf.toString("utf8");
    if (text.length > MAX_SKILL_FILE_CHARS) {
      text = text.slice(0, MAX_SKILL_FILE_CHARS) + "\n\n…（内容过长已截断）";
    }
    return { text };
  }

  skillsList(params: { cwd?: string }): {
    skills: Array<{ name: string; description: string; path: string; scope: string; enabled: boolean }>;
  } {
    const dir = getAgentDir();
    const settings = readJson(settingsPath()) ?? {};
    const out: Array<{ name: string; description: string; path: string; scope: string; enabled: boolean }> = [];
    const scan = (root: string, scope: string): void => {
      if (!existsSync(root)) return;
      const { skills } = loadSkillsFromDir({ dir: root, source: scope });
      for (const s of skills) {
        out.push({
          name: s.name,
          description: s.description,
          path: s.filePath,
          scope,
          enabled: !hasExclusion(settings, "skills", s.filePath),
        });
      }
    };
    scan(join(dir, "skills"), "global");
    if (params.cwd) scan(join(params.cwd, ".pi", "skills"), "project");
    return { skills: out };
  }

  skillsToggle(params: { path: string; enabled: boolean }): { ok: true } {
    if (!params.path) throw new RpcError("bad_request", "path is required");
    toggleExclusion("skills", params.path, !params.enabled);
    return { ok: true };
  }

  // ------------------------------------------------------------- 安装（网址/本地）
  // 技能 = 含 SKILL.md 的目录（zip/tgz 压缩包）；插件 = 单个 .ts/.js 文件。
  // 安装来源：url（host 下载）或 srcPath（本地导入，经文件选择器取得绝对路径）。

  /** 下载远端内容，50MB 上限 + 60s 超时 */
  private async download(url: string): Promise<Buffer> {
    if (!/^https?:\/\//i.test(url)) {
      throw new RpcError("bad_request", "url must be an http(s) URL");
    }
    let res: Response;
    try {
      res = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(60_000) });
    } catch (err) {
      throw new RpcError("download_failed", `下载失败: ${String(err instanceof Error ? err.message : err)}`);
    }
    if (!res.ok) {
      throw new RpcError("download_failed", `下载失败: HTTP ${res.status} ${res.statusText}`);
    }
    const buf = Buffer.from(await res.arrayBuffer());
    if (buf.length > 50 * 1024 * 1024) {
      throw new RpcError("too_large", "下载内容超过 50MB 上限");
    }
    return buf;
  }

  /** 名称仅允许字母数字与 . _ -，防路径穿越/隐蔽后缀 */
  private static sanitizeName(raw: string): string {
    const cleaned = (raw || "")
      .replace(/\\/g, "/")
      .split("/")
      .pop()!
      .replace(/[^A-Za-z0-9._-]+/g, "-")
      .replace(/^[-.]+/, "");
    if (!cleaned) throw new RpcError("bad_request", "无法从来源推导出合法名称，请手动指定");
    return cleaned;
  }

  /**
   * Windows 的 PATH 上 Git 自带的 GNU tar 常排在 System32 bsdtar 之前，
   * 而 GNU tar 会把 `C:\...` 归档名当远程主机（rsh 语法）——优先用系统自带 bsdtar。
   */
  private static tarProgram(): string {
    if (process.platform === "win32") {
      const sysTar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
      if (existsSync(sysTar)) return sysTar;
    }
    return "tar";
  }

  /** 用系统 bsdtar（Win10+ 自带，zip/tgz 通吃）解压到 destDir */
  private extractArchive(archivePath: string, destDir: string): Promise<void> {
    return new Promise((resolvePromise, reject) => {
      const child = spawn(ConfigService.tarProgram(), ["-xf", archivePath, "-C", destDir], {
        windowsHide: true,
        stdio: ["ignore", "ignore", "pipe"],
      });
      let stderr = "";
      child.stderr?.on("data", (d) => (stderr += String(d)));
      child.on("error", (err) => reject(new RpcError("extract_failed", `tar 启动失败（Win10+ 自带 tar.exe）: ${err.message}`)));
      child.on("close", (code) => {
        if (code === 0) resolvePromise();
        else reject(new RpcError("extract_failed", `解压失败 (exit ${code}): ${stderr.slice(-400)}`));
      });
    });
  }

  /** 在解压结果中定位技能目录：根目录、单层或双层包裹目录里找 SKILL.md */
  private findSkillDir(dir: string, depth = 0): string | null {
    if (existsSync(join(dir, "SKILL.md"))) return dir;
    if (depth >= 2) return null;
    const dirs = readdirSync(dir, { withFileTypes: true }).filter((e) => e.isDirectory());
    const first = dirs[0];
    if (dirs.length === 1 && first) return this.findSkillDir(join(dir, first.name), depth + 1);
    return null;
  }

  async skillsInstall(params: { url?: string; srcPath?: string; name?: string }): Promise<{ ok: true; name: string; path: string }> {
    let buf: Buffer;
    let baseName: string;
    if (params.srcPath) {
      const src = params.srcPath.trim();
      if (!src || !existsSync(src) || !statSync(src).isFile()) {
        throw new RpcError("bad_request", "srcPath 文件不存在");
      }
      buf = readFileSync(src);
      baseName = src.slice(src.lastIndexOf(sep) + 1) || "skill";
    } else if (params.url) {
      buf = await this.download(params.url.trim());
      baseName = new URL(params.url).pathname.split("/").filter(Boolean).pop() || "skill";
    } else {
      throw new RpcError("bad_request", "url 与 srcPath 至少提供一个");
    }
    if (buf.length === 0) throw new RpcError("bad_request", "压缩包内容为空");

    const tmp = mkdtempSync(join(tmpdir(), "pidock-skill-"));
    try {
      const archivePath = join(tmp, "pkg" + (/\.(zip|tgz|tar\.gz|gz)$/i.test(baseName) ? baseName.slice(baseName.lastIndexOf(".")) : ".zip"));
      writeFileSync(archivePath, buf);
      const extractDir = join(tmp, "x");
      mkdirSync(extractDir);
      await this.extractArchive(archivePath, extractDir);
      const skillDir = this.findSkillDir(extractDir);
      if (!skillDir) {
        throw new RpcError("no_skill", "压缩包中未找到 SKILL.md（技能包须含 SKILL.md）");
      }
      // zip-slip 防护：真实路径必须仍在临时目录内
      const realSkill = realpathSync(skillDir);
      const realTmp = realpathSync(tmp);
      if (!realSkill.startsWith(realTmp + sep) && realSkill !== realTmp) {
        throw new RpcError("bad_request", "压缩包路径越界，已拒绝安装");
      }
      // 包根就是技能 → 用压缩包名；有包裹目录 → 用内层目录名；显式 name 优先
      const derived =
        skillDir === extractDir
          ? baseName.replace(/\.(zip|tgz|tar\.gz|gz)$/i, "")
          : skillDir.slice(skillDir.lastIndexOf(sep) + 1);
      const name = ConfigService.sanitizeName(params.name || derived);
      const target = join(getAgentDir(), "skills", name);
      if (existsSync(target)) {
        throw new RpcError("already_exists", `技能「${name}」已存在，请先删除或更换名称`);
      }
      mkdirSync(join(getAgentDir(), "skills"), { recursive: true });
      cpSync(realSkill, target, { recursive: true });
      return { ok: true, name, path: target };
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  }

  async extensionsInstall(params: { url?: string; srcPath?: string; filename?: string }): Promise<{ ok: true; file: string }> {
    let text: string;
    let baseName: string;
    if (params.srcPath) {
      const src = params.srcPath.trim();
      if (!src || !existsSync(src) || !statSync(src).isFile()) {
        throw new RpcError("bad_request", "srcPath 文件不存在");
      }
      const raw = readFileSync(src);
      if (looksBinary(raw)) throw new RpcError("bad_request", "插件必须是文本文件（.ts/.js）");
      text = raw.toString("utf8");
      baseName = src.slice(src.lastIndexOf(sep) + 1) || "plugin.ts";
    } else if (params.url) {
      const buf = await this.download(params.url.trim());
      if (looksBinary(buf)) throw new RpcError("bad_request", "下载内容不是文本文件（.ts/.js）");
      text = buf.toString("utf8");
      baseName = new URL(params.url).pathname.split("/").filter(Boolean).pop() || "plugin.ts";
    } else {
      throw new RpcError("bad_request", "url 与 srcPath 至少提供一个");
    }
    const filename = ConfigService.sanitizeName(params.filename || baseName);
    if (!/\.(ts|js)$/i.test(filename)) {
      throw new RpcError("bad_request", "插件文件必须是 .ts 或 .js");
    }
    if (text.length > 2 * 1024 * 1024) {
      throw new RpcError("too_large", "插件超过 2MB 上限");
    }
    const dir = join(getAgentDir(), "extensions");
    mkdirSync(dir, { recursive: true });
    const target = join(dir, filename);
    if (existsSync(target)) {
      throw new RpcError("already_exists", `插件「${filename.replace(/\.(ts|js)$/i, "")}」已存在，请先删除或更换文件名`);
    }
    writeFileSync(target, text);
    return { ok: true, file: target };
  }

  /**
   * List the files of a skill directory. `path` is the skill's SKILL.md
   * absolute path as returned by skills.list; paths in the reply are
   * relative to the skill dir with forward slashes.
   */
  skillsFiles(params: { path: string }): {
    dir: string;
    files: Array<{ file: string; size: number; binary: boolean }>;
  } {
    const dir = this.skillDir(params.path);
    const files: Array<{ file: string; size: number; binary: boolean }> = [];
    const walk = (cur: string, rel: string, depth: number): void => {
      if (files.length >= MAX_SKILL_FILES || depth > MAX_SKILL_DEPTH) return;
      for (const entry of readdirSync(cur, { withFileTypes: true })) {
        if (files.length >= MAX_SKILL_FILES) return;
        const abs = join(cur, entry.name);
        const name = rel ? `${rel}/${entry.name}` : entry.name;
        if (entry.isDirectory()) walk(abs, name, depth + 1);
        else if (entry.isFile()) {
          let size = 0;
          try {
            size = statSync(abs).size;
          } catch {
            // unreadable entry — report with size 0
          }
          files.push({ file: name, size, binary: binaryExtension(name) });
        }
      }
    };
    walk(dir, "", 1);
    files.sort((a, b) =>
      a.file === "SKILL.md" ? -1 : b.file === "SKILL.md" ? 1 : a.file.localeCompare(b.file),
    );
    return { dir, files };
  }

  /** Read one file of a skill directory; `file` is relative as returned by skills.files. */
  skillsRead(params: { path: string; file: string }): { text: string } {
    const dir = this.skillDir(params.path);
    const rel = (params.file ?? "").replace(/\\/g, "/").replace(/^\/+/, "");
    if (!rel || rel.split("/").includes("..")) {
      throw new RpcError("bad_request", "file must be a relative path inside the skill dir");
    }
    const abs = resolve(dir, rel);
    if (abs !== dir && !abs.startsWith(dir + sep)) {
      throw new RpcError("bad_request", "file escapes the skill directory");
    }
    if (!existsSync(abs) || !statSync(abs).isFile()) {
      throw new RpcError("not_found", "file not found in skill dir");
    }
    const buf = readFileSync(abs);
    if (binaryExtension(rel) || looksBinary(buf)) {
      // binary content is never shipped to the UI as garbled text
      throw new RpcError("binary_file", "binary file content is not displayed");
    }
    let text = buf.toString("utf8");
    if (text.length > MAX_SKILL_FILE_CHARS) {
      text = text.slice(0, MAX_SKILL_FILE_CHARS) + "\n\n…（内容过长已截断）";
    }
    return { text };
  }

  /** validate that `path` is an existing SKILL.md and return its resolved dir */
  private skillDir(path: string | undefined): string {
    if (!path || !path.endsWith("SKILL.md")) {
      throw new RpcError("bad_request", "path must be a SKILL.md file");
    }
    const abs = resolve(path);
    if (!existsSync(abs) || !statSync(abs).isFile()) {
      throw new RpcError("not_found", "SKILL.md not found");
    }
    return dirname(abs);
  }

  attachmentGet(params: { attachment_id: string }): { text: string } {
    const id = params.attachment_id ?? "";
    if (!/^[a-f0-9]{64}$/.test(id)) {
      throw new RpcError("bad_request", "attachment_id must be a sha256 hex string");
    }
    const file = join(getAgentDir(), "pidock", "attachments", id);
    if (!existsSync(file)) {
      throw new RpcError("not_found", "attachment content not available on this machine");
    }
    return { text: readFileSync(file, "utf8") };
  }

  mcpGet(): { config: Json | null; path: string } {
    const path = join(getAgentDir(), "mcp.json");
    return { config: readJson(path), path };
  }

  mcpSet(params: { config: unknown }): { ok: true } {
    if (params.config === null || typeof params.config !== "object") {
      throw new RpcError("bad_request", "config must be an object");
    }
    writeJson(join(getAgentDir(), "mcp.json"), params.config);
    return { ok: true };
  }

  // --------------------------------------------------------------- AGENTS.md
  // 「记忆」页：pi 每次会话都会把 ~/.pi/agent/AGENTS.md 作为长期上下文注入，
  // 这里提供该文件的读写，让用户在桌面端维护跨项目记忆。

  agentsRead(): { path: string; exists: boolean; text: string } {
    const path = join(getAgentDir(), "AGENTS.md");
    if (!existsSync(path)) return { path, exists: false, text: "" };
    return { path, exists: true, text: readFileSync(path, "utf8") };
  }

  agentsWrite(params: { text: string }): { ok: true; path: string } {
    if (typeof params.text !== "string") {
      throw new RpcError("bad_request", "text must be a string");
    }
    if (params.text.length > 1024 * 1024) {
      throw new RpcError("bad_request", "AGENTS.md too large (1MB cap)");
    }
    const path = join(getAgentDir(), "AGENTS.md");
    writeFileSync(path, params.text);
    return { ok: true, path };
  }

  // -------------------------------------------------------- pidock settings
  // PiDock 自己的设置（区别于 pi 的 settings.json），目前只有代理配置。
  // 代理在 supervisor 启动 host 时转成 HTTP(S)_PROXY/NO_PROXY/NODE_EXTRA_CA_CERTS
  // 环境变量，因此修改后需要重启应用生效。

  static pidockSettingsPath(): string {
    return join(getAgentDir(), "pidock", "settings.json");
  }

  appSettingsGet(): { settings: Json; path: string } {
    const path = ConfigService.pidockSettingsPath();
    return { settings: readJson(path) ?? {}, path };
  }

  appSettingsSet(params: { proxy: Json }): { ok: true; path: string } {
    const proxy = params.proxy;
    if (!proxy || typeof proxy !== "object" || Array.isArray(proxy)) {
      throw new RpcError("bad_request", "proxy must be an object");
    }
    const mode = proxy.mode;
    if (mode !== "direct" && mode !== "http" && mode !== "system") {
      throw new RpcError("bad_request", "proxy.mode must be direct | http | system");
    }
    if (mode === "http") {
      const url = typeof proxy.url === "string" ? proxy.url.trim() : "";
      if (!/^https?:\/\/[^\s]+$/i.test(url)) {
        throw new RpcError("bad_request", "proxy.url must be an http(s) URL when mode is http");
      }
    }
    for (const key of ["url", "noProxy", "caPath"] as const) {
      const v = proxy[key];
      if (v !== undefined && v !== null && typeof v !== "string") {
        throw new RpcError("bad_request", `proxy.${key} must be a string`);
      }
      if (typeof v === "string" && v.length > 4096) {
        throw new RpcError("bad_request", `proxy.${key} too long`);
      }
    }
    const path = ConfigService.pidockSettingsPath();
    mkdirSync(dirname(path), { recursive: true });
    const settings = readJson(path) ?? {};
    settings.proxy = {
      mode,
      ...(proxy.url ? { url: proxy.url.trim() } : {}),
      ...(proxy.noProxy ? { noProxy: proxy.noProxy } : {}),
      ...(proxy.caPath ? { caPath: proxy.caPath } : {}),
    };
    writeJson(path, settings);
    return { ok: true, path };
  }

  // ------------------------------------------------------------- models.json
  // 自定义模型供应商（pi 的 models.json），供桌面端「模型供应商」页读写

  /** 设置/清除内置模型的覆盖配置（写入 models.json providers[id].modelOverrides，运行时实时应用） */
  modelOverrideSet(params: { provider: string; model: string; override: Json | null }): { ok: true } {
    const provider = (params.provider ?? "").trim();
    const model = (params.model ?? "").trim();
    if (!provider || !model) throw new RpcError("bad_request", "provider and model are required");
    const path = join(getAgentDir(), "models.json");
    const config = readJson(path) ?? {};
    if (!config.providers || typeof config.providers !== "object") config.providers = {};
    const raw = config.providers[provider];
    const entry = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
    if (params.override && typeof params.override === "object" && !Array.isArray(params.override)) {
      entry.modelOverrides = entry.modelOverrides && typeof entry.modelOverrides === "object" ? entry.modelOverrides : {};
      entry.modelOverrides[model] = params.override;
    } else if (entry.modelOverrides && typeof entry.modelOverrides === "object") {
      delete entry.modelOverrides[model];
      if (Object.keys(entry.modelOverrides).length === 0) delete entry.modelOverrides;
    }
    if (Object.keys(entry).length === 0) {
      delete config.providers[provider];
    } else {
      config.providers[provider] = entry;
    }
    writeJson(path, config);
    return { ok: true };
  }

  customProvidersGet(): { config: Json | null; path: string } {
    const path = join(getAgentDir(), "models.json");
    return { config: readJson(path), path };
  }

  customProvidersSet(params: { id: string; entry: Json }): { ok: true } {
    const id = (params.id ?? "").trim();
    if (!id || /\s/.test(id)) throw new RpcError("bad_request", "provider id is required and must not contain whitespace");
    if (!params.entry || typeof params.entry !== "object" || Array.isArray(params.entry)) {
      throw new RpcError("bad_request", "entry must be an object");
    }
    const path = join(getAgentDir(), "models.json");
    const config = readJson(path) ?? {};
    if (!config.providers || typeof config.providers !== "object") config.providers = {};
    config.providers[id] = params.entry;
    writeJson(path, config);
    return { ok: true };
  }

  customProvidersRemove(params: { id: string }): { ok: true } {
    const id = (params.id ?? "").trim();
    if (!id) throw new RpcError("bad_request", "id is required");
    const path = join(getAgentDir(), "models.json");
    const config = readJson(path);
    if (config?.providers && typeof config.providers === "object" && id in config.providers) {
      delete config.providers[id];
      writeJson(path, config);
    }
    return { ok: true };
  }

  /** 获取供应商的模型列表（OpenAI 兼容 /models，anthropic 走 /v1/models） */
  async fetchProviderModels(params: {
    baseUrl: string;
    apiKey?: string;
    api?: string;
  }): Promise<{ models: string[] }> {
    const base = (params.baseUrl ?? "").trim().replace(/\/+$/, "");
    if (!/^https?:\/\//i.test(base)) {
      throw new RpcError("bad_request", "baseUrl must be an http(s) URL");
    }
    const api = params.api ?? "openai-completions";
    const url = api === "anthropic-messages" ? `${base}/v1/models` : `${base}/models`;
    const headers: Record<string, string> = {};
    if (params.apiKey) {
      if (api === "anthropic-messages") {
        headers["x-api-key"] = params.apiKey;
        headers["anthropic-version"] = "2023-06-01";
      } else {
        headers.Authorization = `Bearer ${params.apiKey}`;
      }
    }
    const res = await fetch(url, { headers });
    if (!res.ok) {
      throw new RpcError("fetch_failed", `${res.status} ${res.statusText}`);
    }
    const data: any = await res.json();
    const list: any[] = Array.isArray(data) ? data : (data.data ?? data.models ?? []);
    const models = list
      .map((m) => String(typeof m === "string" ? m : (m?.id ?? m?.name ?? "")))
      .filter(Boolean);
    return { models };
  }
}
