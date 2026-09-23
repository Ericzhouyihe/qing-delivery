import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyTheme, initTheme, readStoredTheme } from "./theme";

function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length(): number {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k: string) => map.get(k) ?? null,
    key: (i: number) => [...map.keys()][i] ?? null,
    removeItem: (k: string) => map.delete(k),
    setItem: (k: string, v: string) => map.set(k, v)
  } as Storage;
}

describe("主题存储(FR-002)", () => {
  beforeEach(() => {
    document.documentElement.removeAttribute("data-theme");
    vi.stubGlobal("localStorage", memoryStorage());
  });

  it("默认浅色;记忆 dark;非法值回退浅色", () => {
    expect(readStoredTheme()).toBe("light");
    applyTheme("dark");
    expect(readStoredTheme()).toBe("dark");
    localStorage.setItem("qing-theme", "blue");
    expect(readStoredTheme()).toBe("light");
  });

  it("initTheme 渲染前应用 data-theme", () => {
    applyTheme("dark");
    expect(initTheme()).toBe("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("存储不可用时仍能应用且不抛错", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      }
    });
    expect(readStoredTheme()).toBe("light");
    expect(() => applyTheme("dark")).not.toThrow();
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });
});
