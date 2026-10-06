<script setup lang="ts">
import { computed, ref } from "vue";
import type { UiMessageItem } from "../store.js";
import { fmtBytes, fmtClock, formatSpan } from "../utils/time.js";
import { fmtCost, fmtTokens } from "../utils/format.js";
import MdContent from "./MdContent.vue";
import ToolRow from "./ToolRow.vue";
import ThinkingRow from "./ThinkingRow.vue";
import FileIcon from "./FileIcon.vue";
import Icon from "./Icon.vue";

/**
 * 消息渲染（活动流风格）：
 * - user      右侧气泡 + 下方元信息条（时间 / 复制 / 分叉）
 * - assistant 按块顺序渲染：思考（可折叠）/ 工具活动行 / 纯文本 markdown，
 *             下方元信息条（模型 / token 用量 / 花费 / 生成耗时 / 时间 / 复制）
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

const emit = defineEmits<{ fork: [] }>();

const copied = ref(false);
async function copyMessage(): Promise<void> {
  const it = props.item;
  const text =
    it.role === "user"
      ? copyableText.value || it.text
      : it.text ||
        (it.blocks ?? [])
          .filter((b) => b.type === "text" && (b.text ?? "").trim() !== "")
          .map((b) => b.text ?? "")
          .join("\n\n");
  await navigator.clipboard.writeText(text);
  copied.value = true;
  setTimeout(() => (copied.value = false), 1500);
}

// ---- 消息元信息条 ----
/** 用户消息展示正文（去附件块；复制与默认展示共用） */
const copyableText = computed(() => props.item.text.replace(ATTACHMENT_BLOCK_RE, "").trim());

const assistantMeta = computed(() => {
  const it = props.item;
  if (it.role !== "assistant" || it.streaming) return null;
  const u = it.usage;
  const usageShort = u && (u.input > 0 || u.output > 0) ? { input: fmtTokens(u.input), output: fmtTokens(u.output) } : null;
  const usageTooltip = u
    ? `输入 ${u.input.toLocaleString()} · 输出 ${u.output.toLocaleString()} · 缓存读 ${u.cacheRead.toLocaleString()} · 缓存写 ${u.cacheWrite.toLocaleString()} · 共 ${u.totalTokens.toLocaleString()} tokens`
    : "";
  const cost = fmtCost(u?.costTotal);
  const duration = it.durationMs ? formatSpan(it.durationMs) : "";
  const time = fmtClock(it.ts);
  const modelShort = it.model ? it.model.slice(it.model.indexOf("/") + 1) : "";
  const stopBadge = it.stopReason === "aborted" ? "已中止" : it.stopReason === "length" ? "已截断" : "";
  const show = Boolean(stopBadge || modelShort || usageShort || cost || duration || time);
  if (!show) return null;
  return { usageShort, usageTooltip, cost, duration, time, modelShort, stopBadge };
});

/** 用户消息元信息条（发送确认后展示；分叉需要条目 id） */
const userMeta = computed(() => {
  const it = props.item;
  if (it.role !== "user" || it.pending) return null;
  const time = fmtClock(it.ts);
  if (!time && !it.entryId) return null;
  return { time, forkable: Boolean(it.entryId) };
});

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

// ---- 文档附件（host 把提取文本以 <attachment> 块注入用户消息，这里折叠为 chip） ----
const ATTACHMENT_BLOCK_RE = /<attachment name="([^"]*)" size="(\d+)">\n([\s\S]*?)\n<\/attachment>/g;
interface UserAttachment {
  name: string;
  size: number;
  /** null = 乐观气泡（全文尚在途/在 host 侧），只有名字可展示 */
  body: string | null;
}
/** 去掉 attachment 块后的正文（技能匹配与内容展示都用它，避免块体混进参数区） */
const cleanText = computed(() => props.item.text.replace(ATTACHMENT_BLOCK_RE, "").trim());
const userAttachments = computed<UserAttachment[]>(() => {
  if (props.item.role !== "user") return [];
  if (props.item.pending) {
    return (props.item.attachmentNames ?? []).map((a) => ({ name: a.name, size: a.size ?? 0, body: null }));
  }
  const found: UserAttachment[] = [];
  for (const m of props.item.text.matchAll(ATTACHMENT_BLOCK_RE)) {
    found.push({ name: m[1]!, size: Number(m[2]!), body: m[3]! });
  }
  return found;
});
const openAtt = ref<number | null>(null);

const skillExpansion = computed<SkillExpansion | null>(() => {
  if (props.item.role !== "user") return null;
  const m = cleanText.value.match(SKILL_EXPANSION_RE);
  if (!m) return null;
  return { name: m[1]!, location: m[2]!, body: m[3]!, args: m[4]! };
});
const skillOpen = ref(false);

// ---- /expert: 注入（host 把用户消息整段替换为 <expert> 块 + 参数 + 知识库清单）→ 折叠 ----
// host 段序固定：<expert>块 → 参数 → "# 知识库" 清单附录，这里按标记切出纯参数
const EXPERT_BLOCK_RE = /^<expert name="([^"]*)" location="([^"]*)">\n([\s\S]*?)\n<\/expert>\n\n?([\s\S]*)$/;
const KB_MARKER = "\n\n# 知识库";
interface ExpertBlock {
  name: string;
  body: string;
  args: string;
}
const expertBlock = computed<ExpertBlock | null>(() => {
  if (props.item.role !== "user") return null;
  if (skillExpansion.value) return null; // 技能展开优先
  const m = cleanText.value.match(EXPERT_BLOCK_RE);
  if (!m) return null;
  const after = m[4] ?? "";
  const kbIdx = after.indexOf(KB_MARKER);
  return { name: m[1]!, body: m[3]!, args: kbIdx >= 0 ? after.slice(0, kbIdx) : after };
});
const expertOpen = ref(false);
</script>

<template>
  <!-- user message -->
  <div v-if="item.role === 'user'" class="row user" :class="{ dimmed }">
    <div class="user-col">
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
        <template v-else-if="expertBlock">
          <button class="skill-chip" :title="expertOpen ? '收起专家提示词' : '展开专家提示词'" @click="expertOpen = !expertOpen">
            <Icon name="user-star-line" :size="13" />
            <span>专家 · {{ expertBlock.name }}</span>
            <Icon :name="expertOpen ? 'subtract-line' : 'add-line'" :size="12" />
          </button>
          <pre v-if="expertOpen" class="skill-body">{{ expertBlock.body }}</pre>
          <span v-if="expertBlock.args" class="content">{{ expertBlock.args }}</span>
        </template>
        <span v-else-if="cleanText" class="content">{{ cleanText }}</span>
        <div v-if="userAttachments.length" class="att-row">
          <template v-for="(a, i) in userAttachments" :key="i">
            <button
              class="att-chip"
              :title="a.body ? (openAtt === i ? '收起附件内容' : '展开附件内容') : a.name"
              @click="a.body && (openAtt = openAtt === i ? null : i)"
            >
              <FileIcon :path="a.name" :size="14" />
              <span class="att-name">{{ a.name }}</span>
              <span class="att-size">{{ fmtBytes(a.size) }}</span>
              <Icon v-if="a.body" :name="openAtt === i ? 'subtract-line' : 'add-line'" :size="12" />
            </button>
            <pre v-if="a.body && openAtt === i" class="att-body">{{ a.body }}</pre>
          </template>
        </div>
        <span v-if="item.pending" class="pending-mark">· 发送中</span>
        <div v-if="userImages.length" class="bubble-imgs">
          <img v-for="(u, i) in userImages" :key="i" :src="u" alt="" />
        </div>
      </div>
      <!-- 元信息条：时间 + 复制 / 分叉（悬停浮现） -->
      <div v-if="userMeta" class="meta-row user-meta">
        <span v-if="userMeta.time" class="meta-time">{{ userMeta.time }}</span>
        <button class="act" :title="copied ? '已复制' : '复制'" @click="copyMessage">
          <Icon :name="copied ? 'check-line' : 'file-copy-line'" :size="13" />
        </button>
        <button v-if="userMeta.forkable" class="act" title="从此条消息分叉新会话" @click="emit('fork')">
          <Icon name="git-branch-line" :size="13" />
        </button>
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
    <!-- 元信息条：模型 / token 用量 / 花费 / 生成耗时 / 时间 + 复制（悬停浮现） -->
    <div v-if="assistantMeta" class="meta-row assistant-meta">
      <span v-if="assistantMeta.stopBadge" class="stop-badge">{{ assistantMeta.stopBadge }}</span>
      <span v-if="assistantMeta.modelShort" class="meta-model" :title="item.model">{{ assistantMeta.modelShort }}</span>
      <span v-if="assistantMeta.usageShort" class="meta-usage" :title="assistantMeta.usageTooltip">
        <Icon name="arrow-up-line" :size="11" />{{ assistantMeta.usageShort.input || 0 }}
        <Icon name="arrow-down-line" :size="11" class="dn" />{{ assistantMeta.usageShort.output || 0 }}
      </span>
      <span v-if="assistantMeta.cost" class="meta-cost" :title="assistantMeta.usageTooltip">{{ assistantMeta.cost }}</span>
      <span v-if="assistantMeta.duration" class="meta-time">{{ assistantMeta.duration }}</span>
      <span v-if="assistantMeta.time" class="meta-time">{{ assistantMeta.time }}</span>
      <button v-if="item.text || item.blocks?.some((b) => b.type === 'text' && (b.text ?? '').trim())" class="act" :title="copied ? '已复制' : '复制'" @click="copyMessage">
        <Icon :name="copied ? 'check-line' : 'file-copy-line'" :size="13" />
      </button>
    </div>
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
.user-col {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  max-width: 86%;
  min-width: 0;
}
.bubble {
  max-width: 100%;
  border-radius: var(--pd-radius);
  padding: 9px 13px;
  font-size: calc(13.5px * var(--pd-font-scale));
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
.pending-mark { font-size: calc(11px * var(--pd-font-scale)); opacity: 0.75; margin-left: 6px; }

/* 文档附件 chip（发送中的乐观气泡只有名字，历史消息可展开全文） */
.att-row { display: flex; flex-direction: column; gap: 5px; margin-top: 7px; align-items: flex-end; }
.att-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 7px;
  font-size: calc(12px * var(--pd-font-scale));
  padding: 4px 9px;
  cursor: pointer;
  text-align: left;
}
.att-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--pd-text); font-weight: 500; }
.att-size { flex: none; font-size: calc(11px * var(--pd-font-scale)); color: var(--pd-text-4); }
.att-body {
  margin: 0;
  max-height: 260px;
  overflow-y: auto;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 8px;
  padding: 10px 12px;
  font-family: var(--pd-mono);
  font-size: calc(11.5px * var(--pd-font-scale));
  line-height: 1.65;
  color: var(--pd-text-2);
  white-space: pre-wrap;
  word-break: break-word;
  align-self: stretch;
}

/* /skill: 展开折叠 */
.user-bubble .skill-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--pd-accent-soft);
  color: var(--pd-accent-text);
  border: none;
  border-radius: 7px;
  font-size: calc(12px * var(--pd-font-scale));
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
  font-family: var(--pd-mono);
  font-size: calc(11.5px * var(--pd-font-scale));
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

/* ---- 消息元信息条（时间 / 模型 / token 用量 / 花费 / 耗时 + 复制 / 分叉） ---- */
.meta-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 4px;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  line-height: 1.2;
  user-select: none;
  min-width: 0;
}
.user-meta { justify-content: flex-end; padding-right: 2px; }
.assistant-meta { padding-left: 2px; }
.meta-model {
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.meta-usage, .meta-cost { font-variant-numeric: tabular-nums; }
.meta-usage { display: inline-flex; align-items: center; gap: 2px; }
.meta-usage svg { color: var(--pd-text-4); }
.meta-usage .dn { margin-left: 4px; }
.stop-badge {
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  border: 1px solid var(--pd-border);
  border-radius: 5px;
  padding: 1px 6px;
  white-space: nowrap;
}
.act {
  background: transparent;
  border: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 2px;
  display: grid;
  place-items: center;
  border-radius: 5px;
  opacity: 0;
  transition: opacity 0.15s;
}
.row.user:hover .meta-row .act,
.stream:hover .meta-row .act { opacity: 1; }
.act:hover { color: var(--pd-accent); background: var(--pd-bg-hover); }

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
  font-size: calc(12.5px * var(--pd-font-scale));
}
.msg-error svg { flex: none; color: var(--pd-red); }
</style>
