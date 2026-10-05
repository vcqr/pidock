import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { RpcError } from "./pool.js";

/**
 * 技能/插件安装共用件：从 config.ts（全局安装）与 experts.ts（专家私有安装）
 * 共用的下载、解压、SKILL.md 定位逻辑。技能 = 含 SKILL.md 的目录（zip/tgz）；
 * 插件 = 单个 .ts/.js 文本文件。
 */

/** 下载远端内容，50MB 上限 + 60s 超时 */
export async function downloadToBuffer(url: string): Promise<Buffer> {
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
export function sanitizeInstallName(raw: string): string {
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
export function tarProgram(): string {
  if (process.platform === "win32") {
    const sysTar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
    if (existsSync(sysTar)) return sysTar;
  }
  return "tar";
}

/** 用系统 bsdtar（Win10+ 自带，zip/tgz 通吃）解压到 destDir */
export function extractArchive(archivePath: string, destDir: string): Promise<void> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(tarProgram(), ["-xf", archivePath, "-C", destDir], {
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
export function findSkillDir(dir: string, depth = 0): string | null {
  if (existsSync(join(dir, "SKILL.md"))) return dir;
  if (depth >= 2) return null;
  const dirs = readdirSync(dir, { withFileTypes: true }).filter((e) => e.isDirectory());
  const first = dirs[0];
  if (dirs.length === 1 && first) return findSkillDir(join(dir, first.name), depth + 1);
  return null;
}

/** NUL-byte heuristic, same spirit as git's binary detection */
export function looksBinary(buf: Buffer): boolean {
  const n = Math.min(buf.length, 8192);
  for (let i = 0; i < n; i++) {
    if (buf[i] === 0) return true;
  }
  return false;
}

/** 内容永远不作为文本展示/注入的扩展名（技能详情阅读器、知识库清单共用） */
export const BINARY_EXTENSIONS = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "avif",
  "pdf", "zip", "gz", "tgz", "bz2", "7z", "rar", "xz", "tar",
  "exe", "dll", "so", "dylib", "bin", "o", "a", "lib", "wasm",
  "class", "jar", "pyc", "pyo",
  "ttf", "otf", "woff", "woff2", "eot",
  "mp3", "mp4", "mov", "avi", "mkv", "webm", "ogg", "wav", "flac",
  "db", "sqlite", "sqlite3", "dat", "dmg", "iso", "msi",
]);

/**
 * 解压 zip/tgz 到目标技能目录（zip-slip 防护 + SKILL.md 定位），
 * 返回最终技能目录绝对路径。供全局与专家私有技能安装复用。
 */
export async function installSkillFromBuffer(
  buf: Buffer,
  baseName: string,
  targetRoot: string,
  explicitName?: string,
): Promise<{ name: string; path: string }> {
  if (buf.length === 0) throw new RpcError("bad_request", "压缩包内容为空");
  const tmp = mkdtempSync(join(tmpdir(), "pidock-skill-"));
  try {
    const archivePath = join(tmp, "pkg" + (/\.(zip|tgz|tar\.gz|gz)$/i.test(baseName) ? baseName.slice(baseName.lastIndexOf(".")) : ".zip"));
    writeFileSync(archivePath, buf);
    const extractDir = join(tmp, "x");
    mkdirSync(extractDir);
    await extractArchive(archivePath, extractDir);
    const skillDir = findSkillDir(extractDir);
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
    const name = sanitizeInstallName(explicitName || derived);
    const target = join(targetRoot, name);
    if (existsSync(target)) {
      throw new RpcError("already_exists", `技能「${name}」已存在，请先删除或更换名称`);
    }
    mkdirSync(targetRoot, { recursive: true });
    cpSync(realSkill, target, { recursive: true });
    return { name, path: target };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}
