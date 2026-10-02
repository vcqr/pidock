import { ref } from "vue";

export type ThemeMode = "dark" | "light";

const KEY = "pidock.theme";
const mode = ref<ThemeMode>((localStorage.getItem(KEY) as ThemeMode) || "dark");

export const themeMode = mode;

export function applyTheme(m: ThemeMode): void {
  mode.value = m;
  document.documentElement.dataset.theme = m;
  localStorage.setItem(KEY, m);
}

export function toggleTheme(): void {
  applyTheme(mode.value === "dark" ? "light" : "dark");
}

/** call once at app startup */
export function initTheme(): void {
  applyTheme(mode.value);
}
