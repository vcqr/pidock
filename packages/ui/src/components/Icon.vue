<script setup lang="ts">
import { computed } from "vue";
import { RI } from "../icons.js";

/** RemixIcon 渲染组件：fill 模式，24×24 viewBox，名字见 icons.ts 的 RI 表。 */
const props = withDefaults(defineProps<{ name: string; size?: number }>(), { size: 16 });

const paths = computed(() => RI[props.name] ?? []);
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="currentColor"
    aria-hidden="true"
  >
    <path v-for="(d, i) in paths" :key="i" :d="d" />
  </svg>
</template>
