/**
 * Markdown → sanitized HTML, with syntax-highlighted code fences.
 * Streaming-safe: callers throttle how often they re-render.
 */
import MarkdownIt from "markdown-it";
import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import json from "highlight.js/lib/languages/json";
import bash from "highlight.js/lib/languages/bash";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import css from "highlight.js/lib/languages/css";
import sql from "highlight.js/lib/languages/sql";

import ini from "highlight.js/lib/languages/ini";
import diff from "highlight.js/lib/languages/diff";
import markdownLang from "highlight.js/lib/languages/markdown";
import DOMPurify from "dompurify";

for (const [name, lang] of Object.entries({
  javascript, typescript: typescript, python, rust, go, java, json, bash,
  xml, yaml, css, sql, ini, diff, markdown: markdownLang,
})) {
  hljs.registerLanguage(name, lang as any);
}
hljs.registerAliases(["js", "jsx", "mjs", "cjs"], { languageName: "javascript" });
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["py"], { languageName: "python" });
hljs.registerAliases(["sh", "shell", "zsh", "console"], { languageName: "bash" });
hljs.registerAliases(["html", "vue"], { languageName: "xml" });
hljs.registerAliases(["yml"], { languageName: "yaml" });
hljs.registerAliases(["md"], { languageName: "markdown" });

const escapeHtml = (s: string): string =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

export const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
  highlight(code, langStr): string {
    const lang = (langStr || "").trim().split(/\s+/)[0];
    let highlighted = "";
    let effective = "";
    if (lang && hljs.getLanguage(lang)) {
      try {
        highlighted = hljs.highlight(code, { language: lang }).value;
        effective = lang;
      } catch {
        highlighted = "";
      }
    }
    if (!highlighted) {
      highlighted = escapeHtml(code);
      effective = "plaintext";
    }
    return `<div class="md-code"><div class="md-code-head"><span class="md-code-lang">${escapeHtml(
      effective,
    )}</span><button class="md-code-copy" data-copy type="button">复制</button></div><pre><code class="hljs language-${escapeHtml(
      effective,
    )}">${highlighted}</code></pre></div>`;
  },
});

export function renderMarkdown(source: string): string {
  const raw = md.render(source ?? "");
  return DOMPurify.sanitize(raw, {
    ADD_ATTR: ["data-copy", "target"],
  });
}
