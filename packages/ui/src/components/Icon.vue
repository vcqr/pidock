<script setup lang="ts">
withDefaults(
  defineProps<{
    /** svg path `d` strings, drawn on a 24×24 stroke grid */
    paths: string[];
    size?: number;
    stroke?: number;
  }>(),
  { size: 16, stroke: 1.8 },
);
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="stroke"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path v-for="(d, i) in paths" :key="i" :d="d" />
  </svg>
</template>
