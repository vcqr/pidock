<script setup lang="ts">
import { computed, ref } from "vue";
import type { UiMessageItem } from "../store.js";
import MdContent from "./MdContent.vue";
import ToolRow from "./ToolRow.vue";
import ThinkingRow from "./ThinkingRow.vue";
import Icon from "./Icon.vue";

/**
 * 消息渲染（活动流风格）：
 * - user      右侧气泡
 * - assistant 按块顺序渲染：思考（可折叠）/ 工具活动行 / 纯文本 markdown
 * - toolResult 独立结果（多数已被合并进 toolCall 行，ChatView 会跳过重复项）
 */
const props = withDefaults(
  defineProps<{
    item: UiMessageItem;
    dimmed?: boolean;
    /** callId → 结果状态与输出（由 ChatView 从 toolResult 消息汇总） */
    results?: Record<string, { status: "done" | "error"; output: string }>;
    /** callId → 工具调用参数（来自 assistant 的 toolCall 块，补全 toolResult 行的展示） */
    argsMap?: Record<string, string>;
    /** 折叠态的总结正文：只渲染文本条目（思考/工具行留在「已工作」展开卡片里） */
    textOnly?: boolean;
  }>(),
  { dimmed: false, results: undefined },
);

const copied = ref(false);
async function copyMessage(): Promise<void> {
  const it = props.item;
  const text =
    it.text ||
    (it.blocks ?? [])
      .map((b) => b.text ?? b.output ?? "")
      .join("\n");
  await navigator.clipboard.writeText(text);
  copied.value = true;
  setTimeout(() => (copied.value = false), 1500);
}

type Entry =
  | { kind: "thinking"; text: string; startedAt?: number; durationMs?: number }
  | { kind: "tool"; toolName: string; args: string; callId?: string; status?: "done" | "error"; output?: string }
  | { kind: "text"; text: string };

const entries = computed<Entry[]>(() => {
  const it = props.item;
  if (it.kind !== "message") return [];
  const out: Entry[] = [];
  if (it.role === "assistant") {
    // 思考行始终展示（默认折叠，点击展开原文）；流式阶段带实时计时
    if (it.thinking && !it.blocks) {
      out.push({
        kind: "thinking",
        text: it.thinking,
        startedAt: it.thinkingStartedAt,
        durationMs: it.thinkingMs,
      });
    }
    for (const b of it.blocks ?? []) {
      if (b.type === "thinking") out.push({ kind: "thinking", text: b.thinking ?? "" });
      else if (b.type === "text") out.push({ kind: "text", text: b.text ?? "" });
      else if (b.type === "toolCall") {
        out.push({
          kind: "tool",
          toolName: b.toolName ?? "",
          args: b.args ?? "",
          callId: b.callId,
          output: props.results?.[b.callId ?? ""]?.output,
        });
      }
    }
    if (it.streaming && it.text) out.push({ kind: "text", text: it.text });
    if (!out.length && it.text) out.push({ kind: "text", text: it.text });
  } else if (it.role === "toolResult") {
    for (const b of it.blocks ?? []) {
      if (b.type === "toolResult") {
        out.push({
          kind: "tool",
          toolName: b.toolName ?? "",
          args: props.argsMap?.[b.callId ?? ""] ?? "",
          status: b.isError ? "error" : "done",
          output: b.output ?? "",
        });
      }
    }
  }
  return out;
});

const visibleEntries = computed(() =>
  props.textOnly ? entries.value.filter((e) => e.kind === "text") : entries.value,
);

/** 用户消息的图片（发送时随附 / 历史回放的 image 块） */
const userImages = computed<string[]>(() => {
  if (props.item.role !== "user") return [];
  const urls = (props.item.imageUrls ?? []).slice();
  for (const b of props.item.blocks ?? []) {
    if (b.type === "image" && b.data) {
      const url = `data:${b.mime || "image/png"};base64,${b.data}`;
      if (!urls.includes(url)) urls.push(url);
    }
  }
  return urls;
});

const toolEntries = computed(
  () => entries.value.filter((e) => e.kind === "tool") as Extract<Entry, { kind: "tool" }>[],
);

// ---- /skill: 展开（pi 会把用户消息整段替换为 <skill> 块 + 参数）→ 渲染层折叠 ----
interface SkillExpansion {
  name: string;
  location: string;
  body: string;
  args: string;
}
const SKILL_EXPANSION_RE = /^<skill name="([^"]*)" location="([^"]*)">\n([\s\S]*?)\n<\/skill>\n\n?([\s\S]*)$/;
const skillExpansion = computed<SkillExpansion | null>(() => {
  if (props.item.role !== "user") return null;
  const m = props.item.text.match(SKILL_EXPANSION_RE);
  if (!m) return null;
  return { name: m[1]!, location: m[2]!, body: m[3]!, args: m[4]! };
});
const skillOpen = ref(false);
</script>

<template>
  <!-- user message -->
  <div v-if="item.role === 'user'" class="row user" :class="{ dimmed }">
    <div class="bubble user-bubble" :class="{ pending: item.pending }">
      <template v-if="skillExpansion">
        <button class="skill-chip" :title="skillOpen ? '收起技能内容' : '展开技能内容'" @click="skillOpen = !skillOpen">
          <Icon name="magic-line" :size="13" />
          <span>技能 · {{ skillExpansion.name }}</span>
          <Icon :name="skillOpen ? 'subtract-line' : 'add-line'" :size="12" />
        </button>
        <pre v-if="skillOpen" class="skill-body">{{ skillExpansion.body }}</pre>
        <span v-if="skillExpansion.args" class="content">{{ skillExpansion.args }}</span>
      </template>
      <span v-else class="content">{{ item.text }}</span>
      <span v-if="item.pending" class="pending-mark">· 发送中</span>
      <div v-if="userImages.length" class="bubble-imgs">
        <img v-for="(u, i) in userImages" :key="i" :src="u" alt="" />
      </div>
    </div>
  </div>

  <!-- assistant: 活动流 -->
  <div v-else-if="item.role === 'assistant'" class="stream" :class="{ dimmed }">
    <template v-for="(e, i) in visibleEntries" :key="i">
      <ThinkingRow
        v-if="e.kind === 'thinking'"
        :text="e.text"
        :started-at="e.startedAt"
        :duration-ms="e.durationMs"
      />
      <ToolRow
        v-else-if="e.kind === 'tool'"
        :tool-name="e.toolName"
        :args="e.args"
        :status="e.status ?? results?.[e.callId ?? '']?.status ?? 'done'"
        :output="e.output"
      />
      <div v-else class="text-block">
        <MdContent :source="e.text" />
      </div>
    </template>
    <span v-if="item.streaming && !item.text" class="cursor">▍</span>
    <div v-if="item.errorMessage" class="msg-error">
      <Icon name="error-warning-line" :size="14" />
      <span>{{ item.errorMessage }}</span>
    </div>
    <button v-if="!item.streaming && item.text" class="copy" :title="copied ? '已复制' : '复制'" @click="copyMessage">
      <Icon :name="copied ? 'check-line' : 'file-copy-line'" :size="13" />
    </button>
  </div>

  <!-- toolResult 兜底（callId 未在上方出现过时） -->
  <div v-else class="stream" :class="{ dimmed }">
    <ToolRow
      v-for="(t, i) in toolEntries"
      :key="i"
      :tool-name="t.toolName"
      :args="t.args"
      :status="t.status"
      :output="t.output"
    />
  </div>
</template>

<style scoped>
.row { display: flex; margin: 10px 0; gap: 8px; align-items: flex-start; }
.row.user { justify-content: flex-end; }
.bubble {
  max-width: 86%;
  border-radius: var(--pd-radius);
  padding: 9px 13px;
  font-size: 13.5px;
  min-width: 0;
}
.user-bubble {
  background: var(--pd-bg-hover);
  border: 1px solid var(--pd-border);
  color: var(--pd-text);
  border-radius: 10px;
  white-space: pre-wrap;
  word-break: break-word;
}
.user-bubble.pending { opacity: 0.65; }
.pending-mark { font-size: 11px; opacity: 0.75; margin-left: 6px; }

/* /skill: 展开折叠 */
.user-bubble .skill-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--pd-accent-soft);
  color: var(--pd-accent-text);
  border: none;
  border-radius: 7px;
  font-size: 12px;
  font-weight: 600;
  padding: 3px 9px;
  cursor: pointer;
  margin-bottom: 6px;
}
.user-bubble .skill-body {
  margin: 0 0 8px;
  max-height: 260px;
  overflow-y: auto;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  padding: 10px 12px;
  font-family: Consolas, monospace;
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--pd-text-2);
  white-space: pre-wrap;
}

.stream {
  position: relative;
  padding: 4px 0 4px 2px;
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.text-block { min-width: 0; }
.cursor {
  animation: blink 1s step-start infinite;
  color: var(--pd-accent);
  font-weight: 700;
}
@keyframes blink { 50% { opacity: 0; } }
.copy {
  position: absolute;
  top: 6px;
  right: 0;
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 2px 4px;
  opacity: 0;
  transition: opacity 0.15s;
  display: grid;
  place-items: center;
}
.stream:hover .copy { opacity: 1; }
.copy:hover { color: var(--pd-accent); }
.dimmed { opacity: 0.55; }
.bubble-imgs { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 7px; }
.bubble-imgs img {
  max-width: 180px;
  max-height: 140px;
  border-radius: 8px;
  display: block;
  border: 1px solid var(--pd-border-soft);
}
.msg-error {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 6px;
  padding: 6px 10px;
  border-radius: 8px;
  background: var(--pd-red-soft);
  color: var(--pd-red-text);
  font-size: 12.5px;
}
.msg-error svg { flex: none; color: var(--pd-red); }
</style>
