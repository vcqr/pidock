<script lang="ts">
/**
 * 自定义下拉选择（替代原生 <select> 的丑样式）：
 * 触发框与表单输入框同风格，弹出菜单用主题令牌绘制，支持键盘导航。
 * 仅在表单环境中使用；选项较多/需要分组时可再扩展。
 */
export interface SelectOption {
  value: string;
  label: string;
  /** 右侧次要说明（如类型的技术名），可选 */
  hint?: string;
}
</script>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import Icon from "./Icon.vue";

const props = defineProps<{
  options: SelectOption[];
  placeholder?: string;
}>();

const value = defineModel<string>({ required: true });

const root = ref<HTMLElement | null>(null);
const open = ref(false);
/** 键盘高亮索引（aria-activedescendant 的简化版，焦点保持在触发钮上） */
const active = ref(0);

const current = computed(() => props.options.find((o) => o.value === value.value));

function toggle(): void {
  if (open.value) {
    open.value = false;
    return;
  }
  const idx = props.options.findIndex((o) => o.value === value.value);
  active.value = idx >= 0 ? idx : 0;
  open.value = true;
}

function choose(opt: SelectOption): void {
  value.value = opt.value;
  open.value = false;
}

function onKeydown(e: KeyboardEvent): void {
  if (!open.value) {
    if (["Enter", " ", "ArrowDown", "ArrowUp"].includes(e.key)) {
      e.preventDefault();
      toggle();
    }
    return;
  }
  if (e.key === "Escape" || e.key === "Tab") {
    open.value = false;
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    active.value = Math.min(props.options.length - 1, active.value + 1);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    active.value = Math.max(0, active.value - 1);
  } else if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    const opt = props.options[active.value];
    if (opt) choose(opt);
  }
}

function onDocMousedown(e: MouseEvent): void {
  if (open.value && root.value && !root.value.contains(e.target as Node)) open.value = false;
}

onMounted(() => document.addEventListener("mousedown", onDocMousedown));
onBeforeUnmount(() => document.removeEventListener("mousedown", onDocMousedown));
</script>

<template>
  <div ref="root" class="sel" :class="{ open }">
    <button
      type="button"
      class="sel-trigger"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggle"
      @keydown="onKeydown"
    >
      <span class="sel-label" :class="{ ph: !current }">{{ current?.label ?? placeholder ?? "请选择" }}</span>
      <span v-if="current?.hint" class="sel-hint">{{ current.hint }}</span>
      <span class="sel-chev"><Icon name="arrow-down-s-line" :size="14" /></span>
    </button>
    <ul v-if="open" class="sel-menu" role="listbox">
      <li
        v-for="(o, i) in options"
        :key="o.value"
        role="option"
        :aria-selected="o.value === value"
        :class="{ on: o.value === value, hot: i === active }"
        @mousedown.prevent
        @click="choose(o)"
        @mousemove="active = i"
      >
        <span class="sel-item-label">{{ o.label }}</span>
        <span v-if="o.hint" class="sel-hint">{{ o.hint }}</span>
        <Icon v-if="o.value === value" class="sel-check" name="check-line" :size="14" />
      </li>
    </ul>
  </div>
</template>

<style scoped>
.sel {
  position: relative;
  min-width: 0;
}
.sel-trigger {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  font-family: inherit;
  padding: 9px 12px;
  cursor: pointer;
  text-align: left;
}
.sel.open .sel-trigger,
.sel-trigger:focus-visible {
  outline: none;
  border-color: var(--pd-accent);
}
.sel-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sel-label.ph {
  color: var(--pd-text-4);
}
.sel-hint {
  flex: none;
  font-size: calc(11px * var(--pd-font-scale));
  color: var(--pd-text-4);
  font-family: var(--pd-mono);
}
.sel-chev {
  display: grid;
  place-items: center;
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.sel.open .sel-chev {
  transform: rotate(180deg);
}
.sel-menu {
  position: absolute;
  left: 0;
  right: 0;
  top: calc(100% + 4px);
  margin: 0;
  padding: 4px;
  list-style: none;
  background: var(--pd-bg-raised);
  border: 1px solid var(--pd-border);
  border-radius: 10px;
  box-shadow: var(--pd-shadow);
  z-index: 60;
  max-height: 264px;
  overflow-y: auto;
}
.sel-menu li {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 7px;
  font-size: calc(13px * var(--pd-font-scale));
  color: var(--pd-text-2);
  cursor: pointer;
}
.sel-menu li.hot {
  background: var(--pd-bg-hover);
  color: var(--pd-text);
}
.sel-menu li.on {
  color: var(--pd-text);
  font-weight: 600;
}
.sel-item-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sel-check {
  color: var(--pd-accent);
  flex: none;
}
</style>
