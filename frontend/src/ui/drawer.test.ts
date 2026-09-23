import { afterEach, describe, expect, it, vi } from "vitest";
import { openDrawer } from "./drawer";

describe("drawer 抽屉(键盘可用性/US4)", () => {
  afterEach(() => {
    document.querySelectorAll(".drawer-overlay").forEach((n) => n.remove());
    vi.restoreAllMocks();
  });

  it("打开渲染面板与标题/正文/底栏", () => {
    const handle = openDrawer(null, { title: "规则编辑" });
    expect(document.querySelector(".drawer .modal-title")?.textContent).toBe("规则编辑");
    handle.body.append("正文");
    handle.foot.append("底栏");
    expect(document.querySelector(".drawer-body")?.textContent).toContain("正文");
    expect(document.querySelector(".drawer-foot")?.textContent).toContain("底栏");
    handle.close(true);
  });

  it("脏状态 Esc 关闭需确认;force 跳过确认", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const handle = openDrawer(null, { title: "编辑", isDirty: () => true });
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".drawer")).not.toBeNull();
    handle.close(true);
    expect(document.querySelector(".drawer")).toBeNull();
  });

  it("close() 无脏状态直接关闭", () => {
    const handle = openDrawer(null, { title: "详情" });
    handle.close();
    expect(document.querySelector(".drawer")).toBeNull();
  });
});
