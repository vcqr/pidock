// 产出 tauri externalBin 需要的 host sidecar：
//   apps/desktop/src-tauri/binaries/pidock-host-<host-triple>[.exe]
// triple 取自 rustc -Vv（与 tauri 对 host target 的解析一致），macOS/Linux/Windows
// 通用；binaries/ 被 gitignore 且 bun --compile 不建父目录，这里统一 mkdir。
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = fileURLToPath(new URL("..", import.meta.url));
const outDir = join(rootDir, "apps", "desktop", "src-tauri", "binaries");
mkdirSync(outDir, { recursive: true });

// 优先 tauri 注入的目标 triple（交叉编译时与 tauri build 的 target 一致），
// 否则取本机 rustc host triple
const triple =
  process.env.TAURI_ENV_TARGET_TRIPLE ??
  execSync("rustc -Vv", { encoding: "utf8" })
    .split("\n")
    .find((l) => l.startsWith("host:"))
    ?.slice("host:".length)
    .trim();
if (!triple) throw new Error("无法从 rustc -Vv 解析 host triple（请确认已安装 rust）");

const outfile = join(outDir, `pidock-host-${triple}${triple.includes("windows") ? ".exe" : ""}`);
execSync(
  `bun build ./src/main.ts ./src/extensions/codemode/worker.ts --compile --root . --outfile ${JSON.stringify(outfile)}`,
  { cwd: join(rootDir, "host"), stdio: "inherit" },
);
console.log(`[bundle:host] ${outfile}`);
