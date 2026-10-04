import { ref } from "vue";

export type ThemePref = "dark" | "light" | "system";
export type ThemeMode = "dark" | "light";

const KEY = "pidock.theme";

function readPref(): ThemePref {
  const v = localStorage.getItem(KEY);
  return v === "dark" || v === "light" || v === "system" ? v : "dark";
}

/**
 * 用户偏好（持久化）。system = 跟随系统 prefers-color-scheme，实时响应系统切换。
 * 组件一律消费下面的 themeMode（实际生效值），只有外观设置页读写偏好。
 */
export const themePref = ref<ThemePref>(readPref());
export const themeMode = ref<ThemeMode>(themePref.value === "system" ? "dark" : themePref.value);

const media =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : undefined;

function resolved(): ThemeMode {
  if (themePref.value !== "system") return themePref.value;
  return media?.matches ? "dark" : "light";
}

function apply(): void {
  themeMode.value = resolved();
  document.documentElement.dataset.theme = themeMode.value;
}

export function applyTheme(pref: ThemePref): void {
  themePref.value = pref;
  localStorage.setItem(KEY, pref);
  apply();
}

export function toggleTheme(): void {
  applyTheme(themeMode.value === "dark" ? "light" : "dark");
}

/** call once at app startup */
export function initTheme(): void {
  apply();
  media?.addEventListener?.("change", () => {
    if (themePref.value === "system") apply();
  });
}
