/**
 * 文件预览的语法高亮：独立于 markdown.ts 的 hljs 实例，按路径扩展名选语言。
 * 主题色复用 tokens.css 的全局 .hljs-* 规则（随 data-theme 切换）。
 */
import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import json from "highlight.js/lib/languages/json";
import rust from "highlight.js/lib/languages/rust";
import go from "highlight.js/lib/languages/go";
import python from "highlight.js/lib/languages/python";
import java from "highlight.js/lib/languages/java";
import cpp from "highlight.js/lib/languages/cpp";
import c from "highlight.js/lib/languages/c";
import bash from "highlight.js/lib/languages/bash";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import css from "highlight.js/lib/languages/css";
import sql from "highlight.js/lib/languages/sql";
import ini from "highlight.js/lib/languages/ini";
import diff from "highlight.js/lib/languages/diff";
import markdownLang from "highlight.js/lib/languages/markdown";

for (const [name, lang] of Object.entries({
  javascript, typescript, json, rust, go, python, java, cpp, c, bash,
  xml, yaml, css, sql, ini, diff, markdown: markdownLang,
})) {
  hljs.registerLanguage(name, lang as any);
}
hljs.registerAliases(["js", "jsx", "mjs", "cjs"], { languageName: "javascript" });
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["rs"], { languageName: "rust" });
hljs.registerAliases(["py"], { languageName: "python" });
hljs.registerAliases(["sh", "shell", "zsh", "console", "ps1"], { languageName: "bash" });
hljs.registerAliases(["html", "vue", "svg"], { languageName: "xml" });
hljs.registerAliases(["yml"], { languageName: "yaml" });
hljs.registerAliases(["md"], { languageName: "markdown" });
hljs.registerAliases(["toml"], { languageName: "ini" });
hljs.registerAliases(["h", "hpp", "cc", "cxx"], { languageName: "cpp" });
hljs.registerAliases(["jsonc", "json5"], { languageName: "json" });

const escapeHtml = (s: string): string =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** 按路径扩展名返回 hljs 语言名；未注册的扩展名返回 null（明文渲染） */
export function languageForPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (!ext) return null;
  return hljs.getLanguage(ext) ? ext : null;
}

/** 高亮整段文本；输出 span 可能跨行，须整体放入 <pre> 而非按行拆分 */
export function highlightCode(path: string, text: string): string {
  const lang = languageForPath(path);
  if (!lang) return escapeHtml(text);
  try {
    return hljs.highlight(text, { language: lang, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(text);
  }
}
