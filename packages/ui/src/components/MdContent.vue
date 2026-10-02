<script setup lang="ts">
/**
 * Markdown content with streaming throttle, code-block copy buttons
 * (event delegation) and safe external links.
 */
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { renderMarkdown } from "../markdown.js";

const props = withDefaults(
  defineProps<{ source: string; live?: boolean }>(),
  { live: false },
);

const displayed = ref(props.source);
let timer: ReturnType<typeof setTimeout> | null = null;

watch(
  () => props.source,
  (v) => {
    if (!props.live) {
      displayed.value = v;
      return;
    }
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      displayed.value = props.source;
    }, 80);
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  if (timer) clearTimeout(timer);
});

const html = computed(() => renderMarkdown(displayed.value));
const host = ref<HTMLElement | null>(null);

function onClick(ev: MouseEvent): void {
  const target = ev.target as HTMLElement;
  const btn = target.closest("[data-copy]") as HTMLElement | null;
  if (!btn) return;
  const code = btn.closest(".md-code")?.querySelector("code")?.textContent ?? "";
  void navigator.clipboard.writeText(code).then(() => {
    btn.textContent = "已复制";
    setTimeout(() => (btn.textContent = "复制"), 1500);
  });
}
</script>

<template>
  <div ref="host" class="md-root" @click="onClick" v-html="html" />
</template>

<style>
/* markdown typography — global because content is v-html injected */
.md-root {
  font-size: 13.5px;
  line-height: 1.65;
  color: var(--pd-text);
  word-break: break-word;
  min-width: 0;
}
.md-root > :first-child { margin-top: 0; }
.md-root > :last-child { margin-bottom: 0; }
.md-root h1, .md-root h2, .md-root h3, .md-root h4 {
  color: var(--pd-text);
  margin: 14px 0 8px;
  line-height: 1.35;
}
.md-root h1 { font-size: 1.35em; }
.md-root h2 { font-size: 1.2em; }
.md-root h3 { font-size: 1.08em; }
.md-root p { margin: 6px 0; }
.md-root ul, .md-root ol { margin: 6px 0; padding-left: 22px; }
.md-root li { margin: 3px 0; }
.md-root blockquote {
  margin: 8px 0;
  padding: 2px 12px;
  border-left: 3px solid var(--pd-accent);
  background: var(--pd-bg-hover);
  border-radius: 0 6px 6px 0;
  color: var(--pd-text-2);
}
.md-root a { color: var(--pd-accent); text-decoration: none; }
.md-root a:hover { text-decoration: underline; }
.md-root code:not(.hljs) {
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border);
  border-radius: 4px;
  padding: 1px 5px;
  font-size: 0.9em;
  font-family: Consolas, "JetBrains Mono", monospace;
  color: var(--pd-accent-text);
}
.md-root table {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 12.5px;
  width: 100%;
}
.md-root th, .md-root td {
  border: 1px solid var(--pd-border);
  padding: 5px 9px;
  text-align: left;
}
.md-root th { background: var(--pd-bg-hover); color: var(--pd-text); }
.md-root hr { border: none; border-top: 1px solid var(--pd-border); margin: 12px 0; }
.md-root img { max-width: 100%; border-radius: 8px; }

.md-code {
  margin: 8px 0;
  border: 1px solid var(--pd-code-border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--pd-code-bg);
}
.md-code-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 10px;
  border-bottom: 1px solid var(--pd-code-border);
}
.md-code-lang {
  font-size: 10.5px;
  color: var(--pd-text-3);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-family: Consolas, monospace;
}
.md-code-copy {
  background: transparent;
  border: none;
  color: var(--pd-text-3);
  font-size: 11px;
  cursor: pointer;
  padding: 0 2px;
}
.md-code-copy:hover { color: var(--pd-accent); }
.md-code pre {
  margin: 0;
  padding: 10px 12px;
  overflow-x: auto;
}
.md-code code.hljs {
  background: transparent;
  padding: 0;
  font-size: 12px;
  line-height: 1.6;
  font-family: Consolas, "JetBrains Mono", monospace;
  color: var(--pd-text);
}
</style>
