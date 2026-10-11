<script setup lang="ts">
import { computed } from "vue";
import { FILE_ICON_SVGS, fileIconName } from "../fileIcons.js";

/**
 * 文件类型图标（material-icon-theme，多彩 SVG，不随主题变色）。
 * 按路径自动映射类型；size 为标准边长（px），实际尺寸随全局字号缩放
 * （--pd-font-scale，与周边文字同步缩放）。
 */
const props = withDefaults(defineProps<{ path: string; size?: number; dir?: boolean }>(), { size: 14 });

const html = computed(() => {
  const svg = FILE_ICON_SVGS[fileIconName(props.path, props.dir)] ?? FILE_ICON_SVGS.file!;
  return svg.replace("<svg ", `<svg width="${props.size}" height="${props.size}" `);
});
const box = computed(() => `calc(${props.size}px * var(--pd-font-scale, 1))`);
</script>

<template>
  <span class="file-icon" :style="{ width: box, height: box }" v-html="html" />
</template>

<style scoped>
.file-icon {
  display: inline-grid;
  place-items: center;
  flex: none;
}
.file-icon :deep(svg) {
  display: block;
  /* CSS 尺寸覆盖 svg 的 width/height 属性，viewBox 保证随外框等比缩放 */
  width: 100%;
  height: 100%;
}
</style>
