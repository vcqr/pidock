<script setup lang="ts">
import { ref, watch } from "vue";
import Icon from "./Icon.vue";
import type { AgentStore, ToolInfoUi, TreeNodeUi } from "../store.js";

/**
 * 会话设置弹窗：自动压缩/自动重试开关、工具子集管理、会话导出、分支树导航。
 * 数据在打开时按需拉取（store 动作），不做常驻轮询。
 */
const props = defineProps<{ store: AgentStore; open: boolean }>();
const emit = defineEmits<{ close: [] }>();

const autoCompaction = ref(true);
const autoRetry = ref(true);
const tools = ref<ToolInfoUi[]>([]);
const toolsLoading = ref(false);
const treeNodes = ref<TreeNodeUi[]>([]);
const leafId = ref<string | null>(null);
const exporting = ref<"html" | "jsonl" | null>(null);
const navigatingId = ref<string | null>(null);
const notice = ref<string | null>(null);
/** 工具清单本地勾选集（open 时从 store 快照初始化，保存时一次性下发） */
const enabledTools = ref<Set<string>>(new Set());

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    notice.value = null;
    const sid = props.store.activeId;
    const info = sid ? props.store.contextBySession[sid] : undefined;
    autoCompaction.value = info?.auto_compaction ?? true;
    autoRetry.value = info?.auto_retry ?? true;
    toolsLoading.value = true;
    try {
      tools.value = await props.store.listTools();
      enabledTools.value = new Set(tools.value.filter((t) => t.active).map((t) => t.name));
    } finally {
      toolsLoading.value = false;
    }
    const tree = await props.store.fetchSessionTree();
    treeNodes.value = tree.nodes;
    leafId.value = tree.leafId;
  },
);

function toggleTool(name: string): void {
  const next = new Set(enabledTools.value);
  if (next.has(name)) next.delete(name);
  else next.add(name);
  enabledTools.value = next;
}

async function saveTools(): Promise<void> {
  await props.store.setActiveTools([...enabledTools.value]);
  notice.value = "工具集已更新";
}

async function exportSession(format: "html" | "jsonl"): Promise<void> {
  exporting.value = format;
  try {
    const path = await props.store.exportSession(format);
    notice.value = `已导出：${path}`;
  } catch (err) {
    notice.value = `导出失败：${err instanceof Error ? err.message : String(err)}`;
  } finally {
    exporting.value = null;
  }
}

async function navigateTo(node: TreeNodeUi): Promise<void> {
  if (node.id === leafId.value || navigatingId.value) return;
  navigatingId.value = node.id;
  try {
    await props.store.navigateTree(node.id);
    emit("close");
  } catch (err) {
    notice.value = `切换失败：${err instanceof Error ? err.message : String(err)}`;
  } finally {
    navigatingId.value = null;
  }
}

async function compactNow(): Promise<void> {
  notice.value = (await props.store.compactSession()) ? "已开始压缩上下文…" : "压缩已在进行中";
}
</script>

<template>
  <div v-if="open" class="ss-mask" @click.self="emit('close')">
    <div class="ss-box" role="dialog" aria-modal="true">
      <div class="ss-head">
        <span class="ss-title">会话设置</span>
        <button class="ss-x" title="关闭" @click="emit('close')"><Icon name="close-line" :size="15" /></button>
      </div>

      <div class="ss-body">
        <!-- 开关 -->
        <section class="ss-sec">
          <div class="sec-title">运行行为</div>
          <label class="toggle-row">
            <span>
              <b>自动压缩</b>
              <small>上下文接近模型窗口上限时自动压缩历史</small>
            </span>
            <input v-model="autoCompaction" type="checkbox" @change="store.setAutoCompaction(autoCompaction)" />
          </label>
          <label class="toggle-row">
            <span>
              <b>自动重试</b>
              <small>供应商瞬时错误（限流/断流）按退避策略自动重试</small>
            </span>
            <input v-model="autoRetry" type="checkbox" @change="store.setAutoRetry(autoRetry)" />
          </label>
          <button class="ss-btn" @click="compactNow"><Icon name="collapse-vertical-line" :size="14" />立即压缩上下文</button>
        </section>

        <!-- 导出 -->
        <section class="ss-sec">
          <div class="sec-title">导出</div>
          <div class="btn-row">
            <button class="ss-btn" :disabled="exporting !== null" @click="exportSession('html')">
              <Icon name="file-code-line" :size="14" />{{ exporting === "html" ? "导出中…" : "导出 HTML" }}
            </button>
            <button class="ss-btn" :disabled="exporting !== null" @click="exportSession('jsonl')">
              <Icon name="braces-line" :size="14" />{{ exporting === "jsonl" ? "导出中…" : "导出 JSONL" }}
            </button>
          </div>
          <small class="hint">导出到桌面（HTML 为自包含单文件，JSONL 为当前分支原始条目）</small>
        </section>

        <!-- 工具 -->
        <section class="ss-sec">
          <div class="sec-title">
            工具（{{ enabledTools.size }}/{{ tools.length }} 启用）
            <button class="ss-btn slim" :disabled="toolsLoading" @click="saveTools">保存工具集</button>
          </div>
          <div class="tool-list">
            <div v-if="toolsLoading" class="hint">加载中…</div>
            <label v-for="t in tools" :key="t.name" class="tool-row" :title="t.description">
              <input type="checkbox" :checked="enabledTools.has(t.name)" @change="toggleTool(t.name)" />
              <span class="t-name">{{ t.name }}</span>
              <span v-if="t.namespace" class="t-ns">{{ t.namespace }}</span>
              <span class="t-desc">{{ t.description }}</span>
            </label>
            <div v-if="!toolsLoading && !tools.length" class="hint">没有可用工具</div>
          </div>
          <small class="hint">被排除的工具不进入系统提示，模型无法调用；保存立即生效</small>
        </section>

        <!-- 分支树 -->
        <section class="ss-sec">
          <div class="sec-title">分支历史（就地切换到某条消息继续）</div>
          <div class="tree-list">
            <div v-if="!treeNodes.length" class="hint">还没有用户消息</div>
            <button
              v-for="n in treeNodes"
              :key="n.id"
              class="tree-row"
              :class="{ current: n.id === leafId }"
              :disabled="n.id === leafId || navigatingId !== null"
              @click="navigateTo(n)"
            >
              <Icon name="git-branch-line" :size="13" />
              <span class="n-text">{{ n.text || "（空消息）" }}</span>
              <span v-if="n.id === leafId" class="n-cur">当前</span>
              <span v-else-if="navigatingId === n.id" class="n-cur">切换中…</span>
            </button>
          </div>
          <small class="hint">其后的对话会退出上下文（JSONL 历史保留，可随时切回）</small>
        </section>

        <div v-if="notice" class="ss-notice">{{ notice }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ss-mask {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: rgba(0, 0, 0, 0.5);
  display: grid;
  place-items: center;
}
.ss-box {
  width: 560px;
  max-width: 92vw;
  max-height: 84vh;
  display: flex;
  flex-direction: column;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 12px;
  box-shadow: var(--pd-shadow);
}
.ss-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px 10px;
  border-bottom: 1px solid var(--pd-border);
}
.ss-title { font-size: 14px; font-weight: 600; color: var(--pd-text); }
.ss-x {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-2);
  cursor: pointer;
}
.ss-x:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.ss-body { overflow-y: auto; padding: 14px 18px 16px; display: flex; flex-direction: column; gap: 18px; }
.ss-sec { display: flex; flex-direction: column; gap: 8px; }
.sec-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
  font-weight: 600;
  color: var(--pd-text-2);
  text-transform: none;
  letter-spacing: 0.02em;
}
.toggle-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 10px;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  cursor: pointer;
}
.toggle-row b { display: block; font-size: 12.5px; color: var(--pd-text); font-weight: 600; }
.toggle-row small { display: block; font-size: calc(11px * var(--pd-font-scale, 1)); color: var(--pd-text-3); margin-top: 2px; }
.toggle-row input { accent-color: var(--pd-accent); width: 15px; height: 15px; cursor: pointer; }
.ss-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 8px;
  border: 1px solid var(--pd-border);
  background: none;
  color: var(--pd-text-2);
  font-size: 12px;
  cursor: pointer;
  align-self: flex-start;
}
.ss-btn:hover { background: var(--pd-bg-hover); color: var(--pd-text); }
.ss-btn:disabled { opacity: 0.5; cursor: default; }
.ss-btn.slim { padding: 3px 10px; font-size: calc(11px * var(--pd-font-scale, 1)); }
.btn-row { display: flex; gap: 8px; }
.hint { font-size: calc(11px * var(--pd-font-scale, 1)); color: var(--pd-text-3); line-height: 1.5; }
.tool-list {
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid var(--pd-border);
  border-radius: 8px;
  padding: 4px;
}
.tool-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  border-radius: 6px;
  cursor: pointer;
  min-width: 0;
}
.tool-row:hover { background: var(--pd-bg-hover); }
.tool-row input { accent-color: var(--pd-accent); width: 14px; height: 14px; flex: none; }
.t-name { font-size: calc(12px * var(--pd-font-scale, 1)); font-family: var(--pd-mono); color: var(--pd-text); flex: none; }
.t-ns {
  font-size: calc(10px * var(--pd-font-scale, 1));
  color: var(--pd-accent);
  border: 1px solid var(--pd-accent);
  border-radius: 4px;
  padding: 0 4px;
  flex: none;
}
.t-desc {
  font-size: calc(11px * var(--pd-font-scale, 1));
  color: var(--pd-text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}
.tree-list { max-height: 180px; overflow-y: auto; border: 1px solid var(--pd-border); border-radius: 8px; padding: 4px; display: flex; flex-direction: column; gap: 2px; }
.tree-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--pd-text-2);
  font-size: calc(12px * var(--pd-font-scale, 1));
  cursor: pointer;
  text-align: left;
  min-width: 0;
}
.tree-row:hover:not(:disabled) { background: var(--pd-bg-hover); color: var(--pd-text); }
.tree-row:disabled { cursor: default; }
.tree-row.current { color: var(--pd-text-3); }
.n-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
.n-cur { flex: none; font-size: calc(10px * var(--pd-font-scale, 1)); color: var(--pd-accent); }
.ss-notice {
  font-size: calc(12px * var(--pd-font-scale, 1));
  color: var(--pd-text-2);
  background: var(--pd-bg-hover);
  border-radius: 8px;
  padding: 8px 10px;
  word-break: break-all;
}
</style>
