<script setup lang="ts">
import { computed } from "vue";
import { FILE_ICON_SVGS, fileIconName } from "../fileIcons.js";

/**
 * 文件类型图标（material-icon-theme，多彩 SVG，不随主题变色）。
 * 按路径自动映射类型；size 控制渲染边长。
 */
const props = withDefaults(defineProps<{ path: string; size?: number }>(), { size: 14 });

const html = computed(() => {
  const svg = FILE_ICON_SVGS[fileIconName(props.path)] ?? FILE_ICON_SVGS.file!;
  return svg.replace("<svg ", `<svg width="${props.size}" height="${props.size}" `);
});
</script>

<template>
  <span class="file-icon" v-html="html" />
</template>

<style scoped>
.file-icon {
  display: inline-grid;
  place-items: center;
  flex: none;
}
.file-icon :deep(svg) {
  display: block;
}
</style>
