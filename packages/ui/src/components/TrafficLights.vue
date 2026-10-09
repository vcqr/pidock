<script setup lang="ts">
// macOS 风格窗口控制（红绿灯）。仅 macOS 且在 Tauri 壳内渲染；
// 颜色/尺寸对齐系统标准（12px 圆点、8px 间距）。
// 绿灯：点击=全屏（系统默认），悬停弹出等效 Sequoia「Move & Resize」菜单
// （半屏平铺/最大化/全屏——系统原生的该菜单属于 NSWindow 标准按钮，自绘按钮无法
// 触发，这里用 Tauri 的 setPosition/setSize + workArea 实现同等动作）。
// Windows/Linux 仍用各自的方块按钮，不由本组件渲染。
import { computed, inject } from "vue";
import { NDropdown, type DropdownOption } from "naive-ui";
import { IS_MAC, WINDOW_CONTROLS } from "../databus.js";

const win = inject(WINDOW_CONTROLS, null);
const visible = computed(() => IS_MAC && win != null);

function onZoomClick(e: MouseEvent): void {
  if (!win) return;
  if (e.altKey) win.toggleMaximize();
  else win.toggleFullscreen();
}

const zoomOptions = computed<DropdownOption[]>(() => [
  { label: win?.isFullscreen.value ? "退出全屏" : "进入全屏", key: "fullscreen" },
  { label: "移到左半屏", key: "left" },
  { label: "移到右半屏", key: "right" },
  { label: win?.isMax.value ? "还原" : "最大化", key: "maximize" },
]);
function onZoomSelect(key: string): void {
  if (!win) return;
  if (key === "fullscreen") win.toggleFullscreen();
  else if (key === "left" || key === "right") win.tile(key);
  else win.toggleMaximize();
}
</script>

<template>
  <div v-if="visible" class="tlights">
    <button class="tlight close" title="关闭" @click="win!.close()">
      <svg width="8" height="8" viewBox="0 0 8 8" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M1.2 1.2l5.6 5.6M6.8 1.2 1.2 6.8" /></svg>
    </button>
    <button class="tlight min" title="最小化" @click="win!.minimize()">
      <svg width="8" height="8" viewBox="0 0 8 8" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M1 4h6" /></svg>
    </button>
    <n-dropdown trigger="hover" placement="bottom-start" :options="zoomOptions" @select="onZoomSelect">
      <button class="tlight zoom" title="全屏" @click="onZoomClick">
        <svg width="8" height="8" viewBox="0 0 8 8" fill="currentColor"><path d="M1.5 1H4a.4.4 0 0 1 .28.68L2.08 3.78A.4.4 0 0 1 1.5 3.5V1zM6.5 7H4a.4.4 0 0 1-.28-.68l2.2-2.1a.4.4 0 0 1 .58.28V7z" /></svg>
      </button>
    </n-dropdown>
  </div>
</template>

<style scoped>
.tlights {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
}
.tlight {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: none;
  padding: 0;
  display: grid;
  place-items: center;
  color: rgba(255, 255, 255, 0.9);
  cursor: pointer;
  box-shadow: inset 0 0 0 0.5px rgba(0, 0, 0, 0.18);
}
/* 系统行为：悬停红绿灯组时才显示符号，悬停的按钮加深 */
.tlight svg { opacity: 0; display: block; }
.tlights:hover .tlight svg { opacity: 1; }
.tlight.close { background: #ff5f57; }
.tlight.close:hover { background: #e0443e; }
.tlight.min { background: #febc2e; }
.tlight.min:hover { background: #e0a425; }
.tlight.zoom { background: #28c840; }
.tlight.zoom:hover { background: #1faa33; }
.tlight:focus-visible { outline: 2px solid var(--pd-accent, #4a9eff); outline-offset: 1px; }
</style>
