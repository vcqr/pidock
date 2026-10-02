<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { AgentStore } from "../store.js";
import { greeting } from "../utils/time.js";
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

const chips: { icon: string[]; text: string }[] = [
  {
    icon: ["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"],
    text: "帮我看看这个项目的结构",
  },
  {
    icon: ["M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"],
    text: "修复当前项目里的报错",
  },
  {
    icon: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z", "M14 2v6h6", "M16 13H8", "M16 17H8"],
    text: "给项目写一份 README",
  },
  {
    icon: ["m16 18 6-6-6-6", "m8 6-6 6 6 6"],
    text: "代码评审最近的改动",
  },
];

const errIcon = ["M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z", "M12 8h.01", "M12 11v5"];
const xIcon = ["M6 6l12 12M18 6 6 18"];
const shieldIcon = [
  "M20 13c0 5-3.5 7.5-7.7 9a.6.6 0 0 1-.6 0C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.2-2.7a1.2 1.2 0 0 1 1.6 0C14.5 3.8 17 5 19 5a1 1 0 0 1 1 1z",
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
          <Icon :paths="errIcon" :size="15" />
          <span class="err-text" :title="store.lastError">{{ store.lastError }}</span>
          <button class="err-close" title="忽略" @click="dismissError">
            <Icon :paths="xIcon" :size="13" />
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
            <Icon :paths="c.icon" :size="14" />{{ c.text }}
          </button>
        </div>
      </div>
    </div>

    <!-- conversation -->
    <template v-else>
      <div ref="scroller" class="scroll">
        <div v-if="store.loadingHistory" class="hint">加载历史中…</div>
        <template v-for="item in store.items" :key="item.key">
          <ToolCard
            v-if="item.kind === 'tool'"
            :tool-name="item.toolName"
            :status="item.status"
            :args="item.args"
            :partial="item.partial"
            :output="item.output"
          />
          <MessageItem v-else :item="item" />
        </template>
      </div>
      <div v-if="store.pendingApproval" class="approval">
        <Icon :paths="shieldIcon" :size="15" />
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
          @send="(t) => store.send(t)"
          @abort="store.abort()"
          @set-permission-mode="(m) => store.setPermissionMode(m)"
          @set-thinking-level="(l) => store.setThinkingLevel(l)"
          @set-model="(m) => store.setModel(m)"
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
