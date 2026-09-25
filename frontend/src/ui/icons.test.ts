import { describe, expect, it } from "vitest";
import { ICON_NAMES, icon } from "./icons";

describe("图标表(006 契约 §2.2)", () => {
  it("每个键名都渲染为 svg 元素并携带无障碍属性", () => {
    for (const name of ICON_NAMES) {
      const svg = icon(name);
      expect(svg.tagName.toLowerCase()).toBe("svg");
      expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
      expect(svg.getAttribute("aria-hidden")).toBe("true");
      expect(svg.getAttribute("focusable")).toBe("false");
      expect(svg.getAttribute("stroke")).toBe("currentColor");
      expect(svg.getAttribute("fill")).toBe("none");
      // 每个图标都必须有实际路径,不允许空表
      expect(svg.innerHTML.length).toBeGreaterThan(0);
    }
  });

  it("默认尺寸 18,可自定义", () => {
    expect(icon("search").getAttribute("width")).toBe("18");
    expect(icon("search").getAttribute("height")).toBe("18");
    expect(icon("grid", 24).getAttribute("width")).toBe("24");
    expect(icon("grid", 24).getAttribute("height")).toBe("24");
  });

  it("未知键名按编程错误抛出(运行时防线)", () => {
    expect(() => icon("nope" as never)).toThrow();
  });
});
