<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import type { AgentStore, UiItem } from "../store.js";
import { formatSpan, greeting } from "../utils/time.js";
import MdContent from "./MdContent.vue";
import Composer from "./Composer.vue";
import Icon from "./Icon.vue";

const props = withDefaults(
  defineProps<{
    store: AgentStore;
    disabled?: boolean;
    disabledHint?: string;
    model?: string;
    /** 新建任务模式：强制显示默认对话页，发送首条消息时自动创建会话 */
    newTask?: boolean;
  }>(),
  { disabled: false },
);

const emit = defineEmits<{ "open-settings": [tab?: string] }>();

const scroller = ref<HTMLElement | null>(null);
const preset = ref("");
const home = computed(() => props.newTask === true || !props.store.activeId);

/** 当前会话的权限模式 / 思考级别（缺省与 host 一致） */
const permissionMode = computed(
  () => (props.store.activeId ? props.store.permissionModes[props.store.activeId] : undefined) ?? "full",
);
const thinkingLevel = computed(
  () => (props.store.activeId ? props.store.thinkingLevels[props.store.activeId] : undefined) ?? "medium",
);
/** 工作区里出现过的模型（模型下拉的可选项） */
const modelOptions = computed(() => {
  const set = new Set<string>();
  for (const s of props.store.sessions) if (s.model) set.add(s.model);
  return [...set];
});

// distinct project dirs for the composer folder selector
const projects = computed(() => {
  const set = new Set<string>();
  for (const s of props.store.sessions) if (s.cwd) set.add(s.cwd);
  return [...set];
});
const defaultCwd = computed(() => props.store.sessions[0]?.cwd ?? projects.value[0] ?? "");

// ---- 活动流辅助 ----
const busy = computed(() => props.store.agentState !== "idle");
/** callId → 结果状态与输出（从 toolResult 消息汇总，合并进 toolCall 活动行） */
const resultsMap = computed(() => {
  const m: Record<string, { status: "done" | "error"; output: string }> = {};
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "toolResult") {
      for (const b of it.blocks ?? []) {
        if (b.callId) m[b.callId] = { status: b.isError ? "error" : "done", output: b.output ?? "" };
      }
    }
  }
  return m;
});
/** callId → 工具调用参数（assistant toolCall 块），供 toolResult 行回填展示 */
const argsMap = computed(() => {
  const m: Record<string, string> = {};
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "assistant") {
      for (const b of it.blocks ?? []) {
        if (b.type === "toolCall" && b.callId) m[b.callId] = b.args ?? "";
      }
    }
  }
  return m;
});
const assistantCallIds = computed(() => {
  const s = new Set<string>();
  for (const it of props.store.items) {
    if (it.kind === "message" && it.role === "assistant") {
      for (const b of it.blocks ?? []) {
        if (b.type === "toolCall" && b.callId) s.add(b.callId);
      }
    }
  }
  return s;
});
function isMergedResult(item: UiItem): boolean {
  if (item.kind !== "message" || item.role !== "toolResult") return false;
  const blocks = item.blocks ?? [];
  return blocks.length > 0 && blocks.every((b) => b.callId !== undefined && assistantCallIds.value.has(b.callId));
}
/** 工作中分隔线：插在最后一条用户消息之后，时长每秒跳动 */
const lastUserIndex = computed(() => {
  const items = props.store.items;
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it && it.kind === "message" && it.role === "user") return i;
  }
  return -1;
});
/** 回合结束 → 活动流折叠为「已工作 · 耗时」一行；点击展开卡片，总结正文保持可见 */
const turnCollapsed = ref(true);
const finishedTurn = computed(
  () =>
    !busy.value &&
    lastUserIndex.value >= 0 &&
    !!props.store.turnStartedAt &&
    !!props.store.turnEndedAt &&
    props.store.items.length > lastUserIndex.value + 1,
);
const turnDuration = computed(() =>
  props.store.turnStartedAt && props.store.turnEndedAt
    ? formatSpan(props.store.turnEndedAt - props.store.turnStartedAt)
    : "",
);
watch(finishedTurn, (done) => {
  if (done) turnCollapsed.value = true;
});
/** 折叠时仍可见的总结正文（回合内最后一条带文本的助手消息） */
const finalTextIndex = computed(() => {
  if (!finishedTurn.value) return -1;
  const items = props.store.items;
  for (let i = items.length - 1; i > lastUserIndex.value; i--) {
    const it = items[i];
    if (it && it.kind === "message" && it.role === "assistant" && (it.text || (it.blocks ?? []).some((b) => b.type === "text"))) {
      return i;
    }
  }
  return -1;
});
const nowTick = ref(Date.now());
let dividerTimer: ReturnType<typeof setInterval> | undefined;
watch(
  busy,
  (b) => {
    if (b) dividerTimer = setInterval(() => (nowTick.value = Date.now()), 1000);
    else if (dividerTimer) {
      clearInterval(dividerTimer);
      dividerTimer = undefined;
    }
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  if (dividerTimer) clearInterval(dividerTimer);
});

const chips: { name: string; text: string }[] = [
  {
    name: "folder-line",
    text: "帮我看看这个项目的结构",
  },
  {
    name: "bug-line",
    text: "修复当前项目里的报错",
  },
  {
    name: "file-add-line",
    text: "给项目写一份 README",
  },
  {
    name: "code-line",
    text: "代码评审最近的改动",
  },
];


function fillChip(text: string): void {
  preset.value = "";
  requestAnimationFrame(() => (preset.value = text));
}

async function sendFromHome(text: string, cwd?: string | null): Promise<void> {
  // cwd 为 null = 「不在项目中工作」（host 落到主目录）；
  // 未指定时用最近会话的目录兜底
  const last = props.store.sessions[0];
  const dir = cwd === null ? undefined : cwd || last?.cwd || ".";
  await props.store.newSession(dir);
  await props.store.send(text);
}

function dismissError(): void {
  props.store.lastError = null;
}

const scrollTarget = () => {
  const el = scroller.value;
  if (el) el.scrollTop = el.scrollHeight;
};

watch(
  () => props.store.items.length,
  async () => {
    await nextTick();
    scrollTarget();
  },
);

let lastLen = 0;
watch(
  () => props.store.items.reduce((n, it) => n + ("text" in it ? it.text.length : 0), 0),
  async (len) => {
    if (len - lastLen > 0 && len - lastLen < 500) {
      await nextTick();
      scrollTarget();
    }
    lastLen = len;
  },
);
</script>

<template>
  <div class="chat" :class="{ home }">
    <!-- home: watermark + greeting + composer -->
    <div v-if="home" class="home-wrap">
      <div class="watermark">π</div>
      <div class="home-inner">
        <h1 class="greeting">{{ greeting() }}，接下来交给我吧</h1>
        <div v-if="store.lastError" class="errbar">
          <Icon name="error-warning-line" :size="15" />
          <span class="err-text" :title="store.lastError">{{ store.lastError }}</span>
          <button class="err-close" title="忽略" @click="dismissError">
            <Icon name="close-line" :size="13" />
          </button>
        </div>
        <Composer
          :busy="false"
          :disabled="disabled"
          :disabled-hint="disabledHint"
          :model="model"
          centered
          :projects="projects"
          :default-cwd="defaultCwd"
          placeholder="描述你的任务，Enter 发送"
          :preset="preset"
          @send="sendFromHome"
        />
        <div class="chips">
          <button v-for="c in chips" :key="c.text" class="chip" @click="fillChip(c.text)">
            <Icon :name="c.name" :size="14" />{{ c.text }}
          </button>
        </div>
      </div>
    </div>

    <!-- conversation -->
    <template v-else>
      <div ref="scroller" class="scroll">
        <div v-if="store.loadingHistory" class="hint">加载历史中…</div>
        <template v-for="(item, index) in store.items" :key="item.key">
          <button
            v-if="finishedTurn && index === lastUserIndex + 1"
            class="turn-header"
            @click="turnCollapsed = !turnCollapsed"
          >
            <span>已工作 · {{ turnDuration }}</span>
            <Icon name="arrow-down-s-line" :size="13" :class="{ fold: turnCollapsed }" />
          </button>
          <template v-if="index <= lastUserIndex || !finishedTurn || !turnCollapsed || index === finalTextIndex">
            <ToolCard
              v-if="item.kind === 'tool'"
              :tool-name="item.toolName"
              :status="item.status"
              :args="item.args"
              :partial="item.partial"
              :output="item.output"
              :dimmed="busy"
            />
            <MessageItem
              v-else-if="!isMergedResult(item)"
              :item="item"
              :dimmed="busy"
              :results="resultsMap"
              :args-map="argsMap"
              :hide-thinking="finishedTurn && turnCollapsed && index === finalTextIndex"
            />
          </template>
          <div v-if="index === lastUserIndex && busy" class="turn-divider">
            <span class="turn-label">工作中 · {{ formatSpan(Math.max(1000, nowTick - (store.turnStartedAt ?? nowTick))) }}</span>
            <span class="turn-line"></span>
          </div>
        </template>
        <div v-if="busy && lastUserIndex === -1" class="turn-divider">
          <span class="turn-label">工作中 · {{ formatSpan(Math.max(1000, nowTick - (store.turnStartedAt ?? nowTick))) }}</span>
          <span class="turn-line"></span>
        </div>
      </div>
      <div v-if="store.pendingApproval" class="approval">
        <Icon name="shield-flash-line" :size="15" />
        <div class="approval-info">
          <b>请求执行：{{ store.pendingApproval.toolName }}</b>
          <span class="approval-args" :title="store.pendingApproval.args">{{ store.pendingApproval.args }}</span>
        </div>
        <button class="approval-btn ok" @click="store.resolveApproval(true)">批准</button>
        <button class="approval-btn no" @click="store.resolveApproval(false)">拒绝</button>
      </div>
      <div class="dock">
        <Composer
          :busy="store.agentState !== 'idle'"
          :model="model"
          :permission-mode="permissionMode"
          :thinking-level="thinkingLevel"
          :models="modelOptions"
          @send="(t: string) => store.send(t)"
          @abort="store.abort()"
          @set-permission-mode="(m: string) => store.setPermissionMode(m)"
          @set-thinking-level="(l: string) => store.setThinkingLevel(l)"
          @set-model="(m: string) => store.setModel(m)"
          @open-settings="emit('open-settings', 'models')"
        />
      </div>
    </template>
  </div>
</template>

<script lang="ts">
import ToolCard from "./ToolCard.vue";
import MessageItem from "./MessageItem.vue";
export default { components: { ToolCard, MessageItem } };
</script>

<style scoped>
.chat {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}
.chat.home { background: var(--pd-bg); }
.home-wrap {
  flex: 1;
  position: relative;
  display: grid;
  place-items: center;
  padding: 24px 40px 60px;
  overflow: hidden;
}
.watermark {
  position: absolute;
  top: 2%;
  left: 50%;
  transform: translateX(-50%) skewX(-10deg);
  font-size: 220px;
  font-weight: 800;
  font-style: italic;
  line-height: 1;
  letter-spacing: -12px;
  color: transparent;
  -webkit-text-stroke: 1.5px var(--pd-watermark-line);
  user-select: none;
  pointer-events: none;
}
.home-inner {
  width: min(760px, 100%);
  display: flex;
  flex-direction: column;
  position: relative;
}
.greeting {
  text-align: center;
  font-size: 26px;
  font-weight: 600;
  color: var(--pd-text);
  margin: 0 0 34px;
  letter-spacing: 0.02em;
}
.errbar {
  display: flex;
  align-items: center;
  gap: 10px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  padding: 9px 14px;
  margin-bottom: 16px;
  color: var(--pd-text-2);
  font-size: 13px;
}
.errbar svg { color: var(--pd-text-3); flex: none; }
.err-text {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.err-close {
  background: none;
  border: none;
  color: var(--pd-text-3);
  cursor: pointer;
  border-radius: 6px;
  padding: 4px;
  display: grid;
  place-items: center;
  flex: none;
}
.err-close:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.chips {
  display: flex;
  gap: 12px;
  margin-top: 30px;
  flex-wrap: wrap;
  justify-content: center;
}
.chip {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  border-radius: 10px;
  border: 1px solid var(--pd-border);
  background: var(--pd-bg-card);
  color: var(--pd-text-2);
  font-size: 13px;
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s;
}
.chip:hover { border-color: #4a4a4a; color: var(--pd-text); }
.scroll {
  flex: 1;
  overflow-y: auto;
  padding: 18px 22px;
}
.hint {
  color: var(--pd-text-4);
  text-align: center;
  margin-top: 40vh;
  font-size: 13px;
}
.dock { padding: 10px 16px 14px; }

/* ---- 回合分隔线（工作中 · 耗时） ---- */
.turn-divider {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 14px 0 6px;
  user-select: none;
}
.turn-label {
  flex: none;
  font-size: 12px;
  color: var(--pd-text-3);
}
.turn-line {
  flex: 1;
  height: 1px;
  background: var(--pd-border-soft);
}

/* ---- 回合结束折叠行（已工作 · 耗时） ---- */
.turn-header {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 8px 0 4px;
  background: none;
  border: none;
  color: var(--pd-text-3);
  font-size: 13px;
  cursor: pointer;
  padding: 4px 0;
  user-select: none;
}
.turn-header:hover { color: var(--pd-text-2); }
.turn-header svg {
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.turn-header svg.fold { transform: rotate(-90deg); }

/* ---- 工具审批横幅 ---- */
.approval {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0 16px 8px;
  padding: 10px 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-accent);
  border-radius: 12px;
  color: var(--pd-text-2);
}
.approval svg { color: var(--pd-accent); flex: none; }
.approval-info { flex: 1; min-width: 0; }
.approval-info b { display: block; font-size: 13px; color: var(--pd-text); }
.approval-args {
  display: block;
  font-size: 11.5px;
  color: var(--pd-text-3);
  font-family: Consolas, monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-top: 2px;
}
.approval-btn {
  flex: none;
  border: none;
  border-radius: 8px;
  padding: 7px 14px;
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}
.approval-btn.ok { background: var(--pd-accent); color: #1a1a1a; }
.approval-btn.ok:hover { background: var(--pd-accent-hover); }
.approval-btn.no { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.approval-btn.no:hover { background: var(--pd-bg-active); color: var(--pd-text); }
</style>
