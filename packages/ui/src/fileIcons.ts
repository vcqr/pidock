/**
 * 文件类型图标 — material-icon-theme（MIT）。
 *
 * 与 icons.ts 的单色 currentColor 体系不同：这里是多彩品牌风 SVG（fill 写死，
 * 不随主题变色，属预期）。按「精确文件名 → 前缀 → 扩展名」三级映射，未命中
 * 回退到库的通用文件图标。只静态内联常用类型（?raw），不引入运行时 IO。
 */
import svgTypescript from "material-icon-theme/icons/typescript.svg?raw";
import svgTypescriptDef from "material-icon-theme/icons/typescript-def.svg?raw";
import svgJavascript from "material-icon-theme/icons/javascript.svg?raw";
import svgReact from "material-icon-theme/icons/react.svg?raw";
import svgVue from "material-icon-theme/icons/vue.svg?raw";
import svgSvelte from "material-icon-theme/icons/svelte.svg?raw";
import svgRust from "material-icon-theme/icons/rust.svg?raw";
import svgPython from "material-icon-theme/icons/python.svg?raw";
import svgGo from "material-icon-theme/icons/go.svg?raw";
import svgGoMod from "material-icon-theme/icons/go-mod.svg?raw";
import svgJava from "material-icon-theme/icons/java.svg?raw";
import svgKotlin from "material-icon-theme/icons/kotlin.svg?raw";
import svgC from "material-icon-theme/icons/c.svg?raw";
import svgCpp from "material-icon-theme/icons/cpp.svg?raw";
import svgCsharp from "material-icon-theme/icons/csharp.svg?raw";
import svgSwift from "material-icon-theme/icons/swift.svg?raw";
import svgRuby from "material-icon-theme/icons/ruby.svg?raw";
import svgPhp from "material-icon-theme/icons/php.svg?raw";
import svgLua from "material-icon-theme/icons/lua.svg?raw";
import svgDart from "material-icon-theme/icons/dart.svg?raw";
import svgZig from "material-icon-theme/icons/zig.svg?raw";
import svgElm from "material-icon-theme/icons/elm.svg?raw";
import svgErlang from "material-icon-theme/icons/erlang.svg?raw";
import svgElixir from "material-icon-theme/icons/elixir.svg?raw";
import svgClojure from "material-icon-theme/icons/clojure.svg?raw";
import svgHtml from "material-icon-theme/icons/html.svg?raw";
import svgCss from "material-icon-theme/icons/css.svg?raw";
import svgSass from "material-icon-theme/icons/sass.svg?raw";
import svgLess from "material-icon-theme/icons/less.svg?raw";
import svgJson from "material-icon-theme/icons/json.svg?raw";
import svgYaml from "material-icon-theme/icons/yaml.svg?raw";
import svgToml from "material-icon-theme/icons/toml.svg?raw";
import svgXml from "material-icon-theme/icons/xml.svg?raw";
import svgProto from "material-icon-theme/icons/proto.svg?raw";
import svgGraphql from "material-icon-theme/icons/graphql.svg?raw";
import svgMarkdown from "material-icon-theme/icons/markdown.svg?raw";
import svgPdf from "material-icon-theme/icons/pdf.svg?raw";
import svgPowershell from "material-icon-theme/icons/powershell.svg?raw";
import svgConsole from "material-icon-theme/icons/console.svg?raw";
import svgDatabase from "material-icon-theme/icons/database.svg?raw";
import svgImage from "material-icon-theme/icons/image.svg?raw";
import svgVideo from "material-icon-theme/icons/video.svg?raw";
import svgAudio from "material-icon-theme/icons/audio.svg?raw";
import svgZip from "material-icon-theme/icons/zip.svg?raw";
import svgLog from "material-icon-theme/icons/log.svg?raw";
import svgLock from "material-icon-theme/icons/lock.svg?raw";
import svgWord from "material-icon-theme/icons/word.svg?raw";
import svgPowerpoint from "material-icon-theme/icons/powerpoint.svg?raw";
import svgDocker from "material-icon-theme/icons/docker.svg?raw";
import svgNpm from "material-icon-theme/icons/npm.svg?raw";
import svgPnpm from "material-icon-theme/icons/pnpm.svg?raw";
import svgBun from "material-icon-theme/icons/bun.svg?raw";
import svgDeno from "material-icon-theme/icons/deno.svg?raw";
import svgVite from "material-icon-theme/icons/vite.svg?raw";
import svgEslint from "material-icon-theme/icons/eslint.svg?raw";
import svgPrettier from "material-icon-theme/icons/prettier.svg?raw";
import svgMakefile from "material-icon-theme/icons/makefile.svg?raw";
import svgCmake from "material-icon-theme/icons/cmake.svg?raw";
import svgGradle from "material-icon-theme/icons/gradle.svg?raw";
import svgMaven from "material-icon-theme/icons/maven.svg?raw";
import svgGit from "material-icon-theme/icons/git.svg?raw";
import svgLicense from "material-icon-theme/icons/license.svg?raw";
import svgFile from "material-icon-theme/icons/file.svg?raw";

export const FILE_ICON_SVGS: Record<string, string> = {
  typescript: svgTypescript,
  "typescript-def": svgTypescriptDef,
  javascript: svgJavascript,
  react: svgReact,
  vue: svgVue,
  svelte: svgSvelte,
  rust: svgRust,
  python: svgPython,
  go: svgGo,
  "go-mod": svgGoMod,
  java: svgJava,
  kotlin: svgKotlin,
  c: svgC,
  cpp: svgCpp,
  csharp: svgCsharp,
  swift: svgSwift,
  ruby: svgRuby,
  php: svgPhp,
  lua: svgLua,
  dart: svgDart,
  zig: svgZig,
  elm: svgElm,
  erlang: svgErlang,
  elixir: svgElixir,
  clojure: svgClojure,
  html: svgHtml,
  css: svgCss,
  sass: svgSass,
  less: svgLess,
  json: svgJson,
  yaml: svgYaml,
  toml: svgToml,
  xml: svgXml,
  proto: svgProto,
  graphql: svgGraphql,
  markdown: svgMarkdown,
  pdf: svgPdf,
  powershell: svgPowershell,
  console: svgConsole,
  database: svgDatabase,
  image: svgImage,
  video: svgVideo,
  audio: svgAudio,
  zip: svgZip,
  log: svgLog,
  lock: svgLock,
  word: svgWord,
  powerpoint: svgPowerpoint,
  docker: svgDocker,
  npm: svgNpm,
  pnpm: svgPnpm,
  bun: svgBun,
  deno: svgDeno,
  vite: svgVite,
  eslint: svgEslint,
  prettier: svgPrettier,
  makefile: svgMakefile,
  cmake: svgCmake,
  gradle: svgGradle,
  maven: svgMaven,
  git: svgGit,
  license: svgLicense,
  file: svgFile,
};

/** 精确文件名（basename 小写）→ 图标名 */
const FILE_NAMES: Record<string, string> = {
  "package.json": "npm",
  "package-lock.json": "npm",
  "pnpm-lock.yaml": "pnpm",
  "pnpm-workspace.yaml": "pnpm",
  "bun.lock": "bun",
  "bun.lockb": "bun",
  "deno.json": "deno",
  "deno.jsonc": "deno",
  "cargo.toml": "rust",
  "cargo.lock": "lock",
  "go.mod": "go-mod",
  "go.sum": "go-mod",
  dockerfile: "docker",
  ".dockerignore": "docker",
  makefile: "makefile",
  "cmakelists.txt": "cmake",
  ".gitignore": "git",
  ".gitattributes": "git",
  ".gitmodules": "git",
  license: "license",
  "license.md": "license",
  "license.txt": "license",
  "requirements.txt": "python",
  pipfile: "python",
};

/** 文件名前缀 → 图标名（覆盖 vite.config.ts/.js/.mts 这类一族配置） */
const FILE_PREFIXES: Array<[string, string]> = [
  ["vite.config.", "vite"],
  ["eslint.config.", "eslint"],
  [".eslintrc", "eslint"],
  [".prettierrc", "prettier"],
  ["prettier.config.", "prettier"],
  ["docker-compose.", "docker"],
];

/** 扩展名（小写）→ 图标名 */
const EXT_NAMES: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "react",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "react",
  vue: "vue",
  svelte: "svelte",
  rs: "rust",
  py: "python",
  pyi: "python",
  pyw: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  hh: "cpp",
  hxx: "cpp",
  cs: "csharp",
  swift: "swift",
  rb: "ruby",
  php: "php",
  lua: "lua",
  dart: "dart",
  zig: "zig",
  elm: "elm",
  erl: "erlang",
  ex: "elixir",
  exs: "elixir",
  clj: "clojure",
  cljs: "clojure",
  edn: "clojure",
  html: "html",
  htm: "html",
  css: "css",
  sass: "sass",
  scss: "sass",
  less: "less",
  json: "json",
  jsonc: "json",
  json5: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  xml: "xml",
  proto: "proto",
  graphql: "graphql",
  gql: "graphql",
  md: "markdown",
  markdown: "markdown",
  pdf: "pdf",
  ps1: "powershell",
  psm1: "powershell",
  psd1: "powershell",
  sh: "console",
  bash: "console",
  zsh: "console",
  fish: "console",
  sql: "database",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  ico: "image",
  bmp: "image",
  avif: "image",
  tiff: "image",
  mp4: "video",
  mkv: "video",
  mov: "video",
  avi: "video",
  webm: "video",
  mp3: "audio",
  wav: "audio",
  flac: "audio",
  ogg: "audio",
  m4a: "audio",
  zip: "zip",
  gz: "zip",
  tgz: "zip",
  bz2: "zip",
  xz: "zip",
  "7z": "zip",
  rar: "zip",
  tar: "zip",
  log: "log",
  lock: "lock",
  doc: "word",
  docx: "word",
  ppt: "powerpoint",
  pptx: "powerpoint",
};

/** 按路径解析文件图标名：精确文件名 → 前缀（含 *.d.ts）→ 扩展名 → 通用文件 */
export function fileIconName(path: string): string {
  const base = path.replace(/\\/g, "/").split("/").pop()?.toLowerCase() ?? "";
  if (FILE_NAMES[base]) return FILE_NAMES[base]!;
  for (const [prefix, name] of FILE_PREFIXES) {
    if (base.startsWith(prefix)) return name;
  }
  if (base.endsWith(".d.ts")) return "typescript-def";
  const dot = base.lastIndexOf(".");
  if (dot > 0) {
    const icon = EXT_NAMES[base.slice(dot + 1)];
    if (icon) return icon;
  }
  return "file";
}
