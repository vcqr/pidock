<script setup lang="ts">
import { computed, ref } from "vue";
import type { AskQuestion } from "@pidock/protocol";
import Icon from "./Icon.vue";

/**
 * AskUserQuestion 问题卡：输入区上方内联（与工具审批横幅同位）。
 * 单选点击选项立即作答；多选打勾后提交；两种模式都可用「其他」自由输入。
 */
const props = defineProps<{ ask: { askId: string; question: AskQuestion } }>();
const emit = defineEmits<{
  resolve: [answer: { labels?: string[]; text?: string; interrupted?: boolean }];
}>();

const selected = ref<string[]>([]);
const custom = ref("");

const multi = computed(() => props.ask.question.multiSelect === true);
const canSubmit = computed(
  () => selected.value.length > 0 || !!custom.value.trim(),
);

function toggle(label: string): void {
  if (!multi.value) {
    emit("resolve", { labels: [label] });
    return;
  }
  const idx = selected.value.indexOf(label);
  if (idx >= 0) selected.value.splice(idx, 1);
  else selected.value.push(label);
}

function submit(): void {
  if (!canSubmit.value) return;
  const text = custom.value.trim();
  if (selected.value.length) emit("resolve", text ? { labels: selected.value, text } : { labels: selected.value });
  else emit("resolve", { text });
}
</script>

<template>
  <div class="ask">
    <div class="ask-head">
      <Icon name="questionnaire-line" :size="15" />
      <b class="ask-header">{{ ask.question.header }}</b>
      <span class="ask-q">{{ ask.question.question }}</span>
      <span v-if="multi" class="ask-multi">可多选</span>
    </div>
    <div class="ask-opts">
      <button
        v-for="opt in ask.question.options"
        :key="opt.label"
        class="ask-opt"
        :class="{ on: selected.includes(opt.label) }"
        :title="opt.description"
        @click="toggle(opt.label)"
      >
        <Icon
          :size="14"
          :name="
            multi
              ? selected.includes(opt.label)
                ? 'checkbox-multiple-line'
                : 'checkbox-multiple-blank-line'
              : 'radio-button-line'
          "
        />
        <span class="opt-label">{{ opt.label }}</span>
        <span v-if="opt.description" class="opt-desc">{{ opt.description }}</span>
      </button>
    </div>
    <div class="ask-foot">
      <input
        v-model="custom"
        class="ask-custom"
        :placeholder="multi ? '其他（可与多选一起提交）…' : '其他，自定义回答…'"
        @keydown.enter.prevent="submit"
      />
      <button v-if="multi" class="ask-btn ok" :disabled="!canSubmit" @click="submit">提交</button>
      <button class="ask-btn no" @click="emit('resolve', { interrupted: true })">取消回答</button>
    </div>
  </div>
</template>

<style scoped>
.ask {
  margin: 0 16px 8px;
  padding: 10px 14px;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-accent);
  border-radius: 12px;
  color: var(--pd-text-2);
}
.ask-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}
.ask-head svg { color: var(--pd-accent); flex: none; }
.ask-header { color: var(--pd-accent); flex: none; }
.ask-q { color: var(--pd-text); font-weight: 600; min-width: 0; }
.ask-multi {
  flex: none;
  margin-left: auto;
  font-size: 11px;
  color: var(--pd-text-3);
  border: 1px solid var(--pd-border);
  border-radius: 999px;
  padding: 1px 8px;
}
.ask-opts {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 10px;
}
.ask-opt {
  display: flex;
  align-items: baseline;
  gap: 8px;
  text-align: left;
  border: 1px solid var(--pd-border);
  background: transparent;
  border-radius: 8px;
  padding: 7px 10px;
  font-size: 12.5px;
  cursor: pointer;
  color: var(--pd-text);
}
.ask-opt svg {
  flex: none;
  color: var(--pd-text-3);
  align-self: center;
  transition: color 0.15s;
}
.ask-opt:hover { border-color: var(--pd-accent); background: var(--pd-bg-hover); }
.ask-opt:hover svg { color: var(--pd-accent); }
.ask-opt.on { border-color: var(--pd-accent); background: var(--pd-bg-hover); }
.ask-opt.on svg { color: var(--pd-accent); }
.opt-label { font-weight: 600; flex: none; }
.opt-desc { color: var(--pd-text-3); font-size: 11.5px; min-width: 0; }
.ask-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
}
.ask-custom {
  flex: 1;
  min-width: 0;
  border: 1px solid var(--pd-border);
  background: transparent;
  border-radius: 8px;
  padding: 6px 10px;
  font-size: 12.5px;
  color: var(--pd-text);
  outline: none;
}
.ask-custom:focus { border-color: var(--pd-accent); }
.ask-custom::placeholder { color: var(--pd-text-3); }
.ask-btn {
  flex: none;
  border: none;
  border-radius: 8px;
  padding: 7px 14px;
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}
.ask-btn.ok { background: var(--pd-accent); color: #1a1a1a; }
.ask-btn.ok:hover:not(:disabled) { background: var(--pd-accent-hover); }
.ask-btn.ok:disabled { opacity: 0.45; cursor: not-allowed; }
.ask-btn.no { background: var(--pd-bg-hover); color: var(--pd-text-2); }
.ask-btn.no:hover { background: var(--pd-bg-active); color: var(--pd-text); }
</style>
