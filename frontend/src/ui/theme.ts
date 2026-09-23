/** 主题存储(T012,研究 D2):localStorage 记忆,默认浅色,非法值回退浅色(FR-002)。 */
export type ThemeMode = "light" | "dark";

const STORAGE_KEY = "qing-theme";

export function readStoredTheme(storage: Storage | null = safeStorage()): ThemeMode {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    return raw === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

export function applyTheme(mode: ThemeMode, storage: Storage | null = safeStorage()): void {
  document.documentElement.setAttribute("data-theme", mode);
  try {
    storage?.setItem(STORAGE_KEY, mode);
  } catch {
    // 隐私模式等存储不可用:仅本会话生效
  }
}

/** 启动时渲染前调用,避免主题闪烁。 */
export function initTheme(): ThemeMode {
  const mode = readStoredTheme();
  document.documentElement.setAttribute("data-theme", mode);
  return mode;
}

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}
