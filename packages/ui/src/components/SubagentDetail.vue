<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { AgentStore, SubagentRunUi, UiBlock, UiMessageItem } from "../store.js";
import MessageItem from "./MessageItem.vue";
import AskCard from "./AskCard.vue";
import ToolRow from "./ToolRow.vue";
import Icon from "./Icon.vue";

/**
 * 子代理工作流详情视图（监控 dock 第二层）：
 * 独立于主消息流的事件流水——进入时经 loadHistory 回放该子代理会话的持久化事件，
 * 再经 store.onEnvelope 旁路订阅实时增量（message_delta/snapshot/tool_execution_*）。
 * 渲染复用 MessageItem（assistant 块含工具行/思考折叠），审批与提问在头部内联裁决。
 */

const props = defineProps<{ store: AgentStore; run: SubagentRunUi }>();
const emit = defineEmits<{ back: [] }>();

interface LiveTool {
  callId: string;
  toolName: string;
  args: string;
  status: "running" | "done" | "error";
  output: string;
}

const messages = ref<UiMessageItem[]>([]);
const liveTools = ref<Map<string, LiveTool>>(new Map());
const results = ref<Record<string, { status: "done" | "error"; output: string }>>({});
const argsMap = ref<Record<string, string>>({});
const loading = ref(true);
const pendingApproval = ref<{ approvalId: string; toolName: string; args: string } | null>(null);
const pendingAsk = ref<{ askId: string; question: any; timeoutSec?: number } | null>(null);
const scrollEl = ref<HTMLElement | null>(null);

// ---- 贴底跟随：仅当用户本就在底部附近才自动滚动；上翻阅读不被流式输出拽回 ----
const atBottom = ref(true);
function onScroll(): void {
  const el = scrollEl.value;
  if (!el) return;
  atBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
}
function scrollToEnd(force = false): void {
  const el = scrollEl.value;
  if (!el) return;
  if (!force && !atBottom.value) return;
  el.scrollTop = el.scrollHeight;
}

const isRunning = computed(
  () => !["idle", "waiting_approval", "waiting_ask"].includes(props.run.state) || !!pendingApproval.value || !!pendingAsk.value,
);

const STATE_LABELS: Record<string, string> = {
  idle: "已完成",
  thinking: "思考中",
  responding: "回复中",
  executing_tool: "执行工具",
  waiting_approval: "等待审批",
  waiting_ask: "等待回答",
  compacting: "压缩中",
  retrying: "重试中",
};
const stateLabel = computed(() => STATE_LABELS[props.run.state] ?? props.run.state);

// ---- 已运行时长（运行中每秒走表；结束后冻结） ----
const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | null = null;
watch(
  isRunning,
  (running) => {
    if (running && !ticker) ticker = setInterval(() => (now.value = Date.now()), 1000);
    if (!running && ticker) {
      clearInterval(ticker);
      ticker = null;
    }
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  if (ticker) clearInterval(ticker);
});
const elapsed = computed(() => {
  const ms = Math.max(0, now.value - Date.parse(props.run.started_at));
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  return m > 0 ? `${m}m${String(s % 60).padStart(2, "0")}s` : `${s}s`;
});

// ---- 历史回放 + 实时订阅 ----
let unsubscribe: (() => void) | null = null;

function pushMessage(item: UiMessageItem): void {
  messages.value.push(item);
  void nextTick(() => scrollToEnd());
}

/** 持久化 message_complete → 消息条目；先收掉末尾流式占位条目（与主消息流同语义） */
function applyComplete(p: any): void {
  while (messages.value.length && messages.value[messages.value.length - 1].streaming) {
    messages.value.pop();
  }
  const blocks: UiBlock[] = (Array.isArray(p?.blocks) ? p.blocks : [])
    .filter((b: any) => b && typeof b.type === "string")
    .map((b: any) => ({
      type: b.type,
      ...(b.text !== undefined ? { text: b.text } : {}),
      ...(b.thinking !== undefined ? { thinking: b.thinking } : {}),
      ...(b.callId ? { callId: b.callId, toolName: b.toolName, args: b.args } : {}),
      ...(b.output !== undefined ? { output: b.output, isError: b.isError } : {}),
      ...(b.data ? { data: b.data, mime: b.mime } : {}),
    }));
  const role = p?.role;
  if (role === "toolResult") {
    const tr = blocks.find((b) => b.type === "toolResult");
    if (tr?.callId) {
      results.value = {
        ...results.value,
        [tr.callId]: { status: tr.isError ? "error" : "done", output: tr.output ?? "" },
      };
      if (liveTools.value.delete(tr.callId)) liveTools.value = new Map(liveTools.value);
    }
    if (tr?.toolName === "AskUserQuestion") pendingAsk.value = null;
    return;
  }
  const text =
    blocks
      .filter((b) => b.type === "text")
      .map((b) => b.text ?? "")
      .join("\n\n") ?? "";
  const liveToolCalls = blocks.filter((b) => b.type === "toolCall" && b.callId);
  if (liveToolCalls.length) {
    for (const b of liveToolCalls) {
      argsMap.value = { ...argsMap.value, [b.callId!]: b.args ?? "" };
      if (liveTools.value.delete(b.callId!)) liveTools.value = new Map(liveTools.value);
    }
  }
  pushMessage({
    kind: "message",
    key: `d${p?.entry_id ?? messages.value.length}`,
    role,
    text,
    thinking: "",
    blocks,
    streaming: false,
    ...(role === "assistant" && p?.usage
      ? { usage: p.usage, ...(p.model ? { model: p.model } : {}), ...(p.duration_ms ? { durationMs: p.duration_ms } : {}) }
      : {}),
  });
}

async function onEnvelope(e: { session_id: string; kind: string; payload: any }): Promise<void> {
  if (e.session_id !== props.run.session_id) return;
  switch (e.kind) {
    case "message_delta":
    case "message_snapshot": {
      // 增量写入末尾流式占位条目；message_complete 到达时定稿替换
      let tail = messages.value[messages.value.length - 1];
      if (!tail || !tail.streaming) {
        tail = {
          kind: "message",
          key: `dstream-${Date.now()}`,
          role: "assistant",
          text: "",
          thinking: "",
          blocks: null,
          streaming: true,
        };
        pushMessage(tail);
      }
      if (e.kind === "message_delta") {
        if (e.payload?.part === "text") tail.text += e.payload.delta ?? "";
        else tail.thinking += e.payload?.delta ?? "";
      } else {
        if ((e.payload?.text ?? "").length >= tail.text.length) tail.text = e.payload?.text ?? "";
        if ((e.payload?.thinking ?? "").length >= tail.thinking.length) tail.thinking = e.payload?.thinking ?? "";
      }
      void nextTick(scrollToEnd);
      break;
    }
    case "message_complete":
      applyComplete(e.payload);
      break;
    case "tool_execution_start": {
      if (e.payload?.tool_name === "AskUserQuestion") break;
      const next = new Map(liveTools.value);
      next.set(e.payload.call_id, {
        callId: e.payload.call_id,
        toolName: e.payload.tool_name,
        args: e.payload.args ?? "",
        status: "running",
        output: "",
      });
      liveTools.value = next;
      void nextTick(scrollToEnd);
      break;
    }
    case "tool_execution_end": {
      const t = liveTools.value.get(e.payload?.call_id);
      if (t) {
        t.status = e.payload?.is_error ? "error" : "done";
        t.output = e.payload?.output ?? "";
      }
      break;
    }
    case "tool_approval":
      pendingApproval.value = e.payload?.approval_id
        ? {
            approvalId: e.payload.approval_id,
            toolName: e.payload?.tool_name ?? "",
            args: e.payload?.args ?? "",
          }
        : null;
      break;
    case "ask_user_question": {
      const q = e.payload?.question;
      pendingAsk.value =
        e.payload?.ask_id && q && Array.isArray(q.options) && q.options.length
          ? { askId: e.payload.ask_id, question: q, ...(Number(e.payload.timeout_sec) > 0 ? { timeoutSec: Number(e.payload.timeout_sec) } : {}) }
          : null;
      break;
    }
    default:
      break;
  }
}

onMounted(async () => {
  unsubscribe = props.store.onEnvelope(onEnvelope);
  // 回放持久化历史；挂起的审批/提问经 session.pending 恢复（与主界面同源）
  const events = await props.store.loadHistory(props.run.session_id);
  for (const ev of events) {
    if (ev.kind === "message_complete") applyComplete(ev.payload);
  }
  loading.value = false;
  void nextTick(() => scrollToEnd(true));
  atBottom.value = true;
  try {
    const p = await props.store.rpc("session.pending", { session_id: props.run.session_id });
    if (p?.approval?.approval_id) {
      pendingApproval.value = {
        approvalId: p.approval.approval_id,
        toolName: p.approval.tool_name ?? "",
        args: p.approval.args ?? "",
      };
    }
    if (p?.ask?.ask_id) pendingAsk.value = { askId: p.ask.ask_id, question: p.ask.question };
  } catch {
    // 旧 host 无 session.pending 时忽略
  }
});

onBeforeUnmount(() => {
  unsubscribe?.();
  unsubscribe = null;
});

async function resolveApproval(approved: boolean): Promise<void> {
  const p = pendingApproval.value;
  if (!p) return;
  pendingApproval.value = null;
  try {
    await props.store.rpc("session.resolve_approval", { approval_id: p.approvalId, approved });
  } catch {
    // 结算失败不阻塞（host 超时/中止路径兜底）
  }
}

async function resolveAsk(answer: { labels?: string[]; text?: string; interrupted?: boolean }): Promise<void> {
  const p = pendingAsk.value;
  if (!p) return;
  pendingAsk.value = null;
  try {
    await props.store.rpc("session.resolve_ask", { session_id: props.run.session_id, ask_id: p.askId, ...answer });
  } catch {
    // 同上
  }
}

async function abortChild(): Promise<void> {
  try {
    await props.store.rpc("agent.abort", { session_id: props.run.session_id });
  } catch {
    // 已结束的子代理中止报错可忽略
  }
}

/** 以完整会话在主区打开（子代理本就是池化会话——重开可回放历史并继续对话） */
async function openFull(): Promise<void> {
  try {
    // 事件先行 upsert 的最小摘要没有 file，先刷新注册表补全
    await props.store.refreshSessions();
    await props.store.openSession(props.run.session_id);
  } catch {
    // 打开失败经 lastError 通道可见
  }
}

const steerText = ref("");

/** 插话（steer）：运行中的子代理会在当前回合内消化这条消息 */
async function sendSteer(): Promise<void> {
  const text = steerText.value.trim();
  if (!text) return;
  steerText.value = "";
  try {
    await props.store.rpc("agent.steer", { session_id: props.run.session_id, text });
  } catch {
    // 会话已结束等错误忽略（输入框仅运行中显示）
  }
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}
</script>

<template>
  <div class="sdetail">
    <header class="head">
      <button class="hbtn" title="返回列表" @click="emit('back')">
        <Icon name="arrow-left-line" :size="14" />
      </button>
      <span class="title" :title="run.name">{{ run.name }}</span>
      <span class="state" :class="{ warn: ['waiting_approval', 'waiting_ask'].includes(run.state) }">
        <span v-if="isRunning" class="spin"></span>
        {{ stateLabel }}
      </span>
      <span class="flex-sp"></span>
      <button class="hbtn" title="以完整会话在主区打开（可继续追问/steer）" @click="openFull">
        <Icon name="arrow-right-line" :size="14" />
      </button>
      <button v-if="isRunning" class="hbtn danger" title="中止子代理" @click="abortChild">
        <Icon name="close-circle-line" :size="15" />
      </button>
    </header>

    <div class="meta">
      <span v-if="run.expert_name" class="tag" :title="`专家 · ${run.expert_name}`">
        <Icon name="user-star-line" :size="11" />{{ run.expert_name }}
      </span>
      <span v-if="run.model" class="tag" :title="run.model">{{ run.model.split("/").pop() }}</span>
      <span class="tag">{{ elapsed }}</span>
      <span v-if="run.tokens" class="tag">{{ fmtTokens(run.tokens) }} tok</span>
    </div>

    <!-- 审批/提问内联裁决：子代理撞上需要人工放行的操作时在此处理 -->
    <div v-if="pendingApproval" class="interact">
      <div class="i-title"><Icon name="shield-keyhole-line" :size="13" />工具审批 · {{ pendingApproval.toolName }}</div>
      <pre class="i-args">{{ pendingApproval.args.slice(0, 400) }}</pre>
      <div class="i-actions">
        <button class="btn primary" @click="resolveApproval(true)">允许</button>
        <button class="btn" @click="resolveApproval(false)">拒绝</button>
      </div>
    </div>
    <AskCard v-if="pendingAsk" class="interact" :ask="pendingAsk" @resolve="resolveAsk" />

    <div ref="scrollEl" class="flow" @scroll="onScroll">
      <div v-if="loading && !messages.length" class="empty">加载中…</div>
      <div v-else-if="!messages.length && !liveTools.size" class="empty">尚无输出</div>
      <template v-for="item in messages" :key="item.key">
        <MessageItem v-if="item.role !== 'toolResult'" :item="item" :results="results" :args-map="argsMap" />
      </template>
      <ToolRow
        v-for="t in liveTools.values()"
        :key="t.callId"
        :tool-name="t.toolName"
        :args="t.args"
        :status="t.status"
        :output="t.output"
      />
    </div>
    <!-- 插话（steer）：运行中引导子代理调整方向，不必中止重来 -->
    <div v-if="isRunning" class="steer">
      <input
        v-model="steerText"
        class="steer-input"
        placeholder="插话引导子代理（它会带着这条调整后续工作）…"
        maxlength="8000"
        @keydown.enter="sendSteer"
      />
      <button class="steer-send" :disabled="!steerText.trim()" title="发送插话" @click="sendSteer">
        <Icon name="arrow-up-line" :size="14" />
      </button>
    </div>

    <!-- 上翻阅读时的回底入口：流式输出继续也不打扰 -->
    <button v-if="!atBottom && isRunning" class="jump-bottom" @click="atBottom = true; scrollToEnd(true)">
      <Icon name="arrow-down-line" :size="13" />回到底部
    </button>
  </div>
</template>

<style scoped>
.sdetail {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  margin-top: 10px;
  background: var(--pd-bg);
  border: 1px solid var(--pd-border-soft);
  border-radius: 10px;
  overflow: hidden;
  position: relative;
}
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--pd-border-soft);
  flex: none;
  min-width: 0;
}
.title {
  font-size: calc(12.5px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.state {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: none;
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-accent);
}
.state.warn { color: var(--pd-orange, #e08a3c); }
.flex-sp { flex: 1; }
.hbtn {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-4);
  cursor: pointer;
  padding: 0;
  flex: none;
}
.hbtn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.hbtn.danger:hover { color: var(--pd-red); }

.meta {
  display: flex;
  gap: 5px;
  flex-wrap: wrap;
  padding: 7px 10px 0;
  flex: none;
}
.tag {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  border: 1px solid var(--pd-border-soft);
  border-radius: 4px;
  padding: 1px 6px;
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.interact {
  margin: 8px 10px 0;
  padding: 8px 10px;
  background: color-mix(in srgb, var(--pd-orange, #e08a3c) 8%, var(--pd-bg-panel));
  border: 1px solid color-mix(in srgb, var(--pd-orange, #e08a3c) 35%, transparent);
  border-radius: 8px;
  flex: none;
}
.i-title {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: calc(12px * var(--pd-font-scale));
  font-weight: 600;
  color: var(--pd-text);
}
.i-args {
  margin: 6px 0;
  font-size: calc(10.5px * var(--pd-font-scale));
  color: var(--pd-text-3);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 80px;
  overflow: hidden;
}
.i-actions { display: flex; gap: 6px; }
.btn {
  border: 1px solid var(--pd-border);
  border-radius: 6px;
  background: var(--pd-bg-panel);
  color: var(--pd-text-2);
  font-size: calc(11.5px * var(--pd-font-scale));
  padding: 3px 12px;
  cursor: pointer;
}
.btn:hover { border-color: var(--pd-accent); color: var(--pd-text); }
.btn.primary {
  background: var(--pd-accent);
  border-color: var(--pd-accent);
  color: #fff;
}

.flow {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
/* 窄宽（dock 260–560px）紧凑适配：主消息流的排版规则在此收紧 */
.flow :deep(.row) {
  margin: 4px 0;
}
.flow :deep(.meta-row) {
  flex-wrap: wrap;
  row-gap: 2px;
  font-size: calc(10px * var(--pd-font-scale));
}
.flow :deep(.stream),
.flow :deep(.text-block),
.flow :deep(.user-col) {
  min-width: 0;
  max-width: 100%;
}
.flow :deep(.stream) {
  overflow-wrap: anywhere;
}
.flow :deep(.tool-wrap) {
  min-width: 0;
}
.flow :deep(pre) {
  max-width: 100%;
  overflow-x: auto;
}
.jump-bottom {
  position: absolute;
  bottom: 12px;
  right: 12px;
  z-index: 6;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  border: 1px solid var(--pd-border);
  border-radius: 13px;
  background: var(--pd-bg-panel);
  color: var(--pd-text-2);
  font-size: calc(11px * var(--pd-font-scale));
  cursor: pointer;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}
.jump-bottom:hover { border-color: var(--pd-accent); color: var(--pd-text); }

/* 插话输入条：dock 底部一行，运行中显示 */
.steer {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  border-top: 1px solid var(--pd-border-soft);
  flex: none;
}
.steer-input {
  flex: 1;
  min-width: 0;
  height: 28px;
  border: 1px solid var(--pd-border);
  border-radius: 14px;
  background: var(--pd-bg-panel);
  color: var(--pd-text);
  font-size: calc(12px * var(--pd-font-scale));
  padding: 0 12px;
  outline: none;
}
.steer-input::placeholder { color: var(--pd-text-4); }
.steer-input:focus { border-color: var(--pd-accent); }
.steer-send {
  flex: none;
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: 50%;
  background: var(--pd-accent);
  color: #fff;
  cursor: pointer;
  padding: 0;
}
.steer-send:disabled { opacity: 0.4; cursor: default; }
.empty {
  margin: auto;
  color: var(--pd-text-4);
  font-size: calc(12px * var(--pd-font-scale));
}
.spin {
  width: 10px;
  height: 10px;
  border: 2px solid currentColor;
  border-top-color: transparent;
  border-radius: 50%;
  animation: sd-spin 0.8s linear infinite;
}
@keyframes sd-spin {
  to { transform: rotate(360deg); }
}
</style>
