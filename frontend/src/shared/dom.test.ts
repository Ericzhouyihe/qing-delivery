import { describe, expect, it } from "vitest";
import { el, errorMessage } from "./dom";
import { parsePage, parseSessionResponse } from "./contracts";

describe("dom helpers", () => {
  it("el 创建带属性与文本子节点的元素", () => {
    const node = el("p", { class: "muted", "data-x": "1" }, "hello", null, " world");
    expect(node.tagName).toBe("P");
    expect(node.className).toBe("muted");
    expect(node.dataset["x"]).toBe("1");
    expect(node.textContent).toBe("hello world");
  });

  it("errorMessage 包装非 Error 值", () => {
    expect(errorMessage(new Error("boom"))).toBe("boom");
    expect(errorMessage(42)).toBe("42");
  });
});

describe("contract runtime validation", () => {
  it("parseSessionResponse 拒绝缺 CSRF 的响应", () => {
    expect(() => parseSessionResponse({ initialized: true, authenticated: false })).toThrow();
  });

  it("parseSessionResponse 解析管理员摘要", () => {
    const res = parseSessionResponse({
      initialized: true,
      authenticated: true,
      csrf_token: "t",
      administrator: { id: "a1", display_name: "管理员" }
    });
    expect(res.administrator?.display_name).toBe("管理员");
  });

  it("parsePage 解析分页与游标", () => {
    const page = parsePage({ items: [{ id: "1" }, { id: "2" }], next_cursor: "c1" }, (r) => {
      const rec = r as Record<string, unknown>;
      return { id: String(rec["id"]) };
    });
    expect(page.items).toHaveLength(2);
    expect(page.next_cursor).toBe("c1");
  });
});
