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
  initFonts();
  media?.addEventListener?.("change", () => {
    if (themePref.value === "system") apply();
  });
}

// ------------------------------------------------------------------ fonts

/** 字体设置：字号缩放系数 + 自定义字体族（空串 = 跟随默认）。同 themePref 存 localStorage。 */
export interface FontSettings {
  /** 全局字号缩放（1 = 标准；组件字号均为 calc(Npx * var(--pd-font-scale))） */
  scale: number;
  /** 界面字体族（CSS font-family 值，留空默认） */
  family: string;
  /** 等宽字体族（代码/diff/文件预览，留空默认） */
  monoFamily: string;
}

const FONT_KEY = "pidock.font";

function readFont(): FontSettings {
  try {
    const raw = JSON.parse(localStorage.getItem(FONT_KEY) ?? "{}") as Partial<FontSettings>;
    return {
      scale: typeof raw.scale === "number" && raw.scale >= 0.8 && raw.scale <= 1.5 ? raw.scale : 1,
      family: typeof raw.family === "string" ? raw.family.slice(0, 200) : "",
      monoFamily: typeof raw.monoFamily === "string" ? raw.monoFamily.slice(0, 200) : "",
    };
  } catch {
    return { scale: 1, family: "", monoFamily: "" };
  }
}

export const fontSettings = ref<FontSettings>(readFont());

function applyFonts(): void {
  const f = fontSettings.value;
  const root = document.documentElement;
  root.style.setProperty("--pd-font-scale", String(f.scale));
  // 字体族留空 = 清除自定义，走主题默认（body 的 font-family / --pd-mono 默认栈）
  if (f.family.trim()) {
    root.style.setProperty("--pd-font-ui", f.family.trim());
    root.style.fontFamily = f.family.trim();
  } else {
    root.style.removeProperty("--pd-font-ui");
    root.style.fontFamily = "";
  }
  if (f.monoFamily.trim()) root.style.setProperty("--pd-mono", f.monoFamily.trim());
  else root.style.removeProperty("--pd-mono");
}

/** 更新并立即应用字体设置；persist=false 仅供拖动实时预览（不落盘） */
export function applyFontSettings(patch: Partial<FontSettings>, persist = true): void {
  fontSettings.value = { ...fontSettings.value, ...patch };
  if (persist) {
    try {
      localStorage.setItem(FONT_KEY, JSON.stringify(fontSettings.value));
    } catch {
      // localStorage 不可用时仅本次会话生效
    }
  }
  applyFonts();
}

function initFonts(): void {
  applyFonts();
}
