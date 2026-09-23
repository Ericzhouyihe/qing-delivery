import { afterEach, describe, expect, it, vi } from "vitest";
import { confirmDialog, openModal } from "./modal";
import { el } from "../shared/dom";

describe("modal 弹窗(键盘可用性/US1)", () => {
  afterEach(() => {
    document.querySelectorAll(".overlay").forEach((n) => n.remove());
    vi.restoreAllMocks();
  });

  it("打开渲染遮罩与标题;close 移除并归还焦点", () => {
    const opener = el("button", {}, "打开");
    document.body.append(opener);
    opener.focus();
    const handle = openModal(opener, { title: "扫码接入" });
    expect(document.querySelector(".overlay .modal-title")?.textContent).toBe("扫码接入");
    handle.close();
    expect(document.querySelector(".overlay")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("Esc 触发关闭;脏状态先确认", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    let dirty = false;
    const handle = openModal(null, { title: "编辑", isDirty: () => dirty });
    dirty = true;
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".overlay")).toBeNull();

    const handle2 = openModal(null, { title: "编辑2", isDirty: () => true });
    confirmSpy.mockReturnValue(false);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(document.querySelector(".overlay")).not.toBeNull(); // 取消放弃 → 保留
    handle2.close(true);
  });

  it("叠层至多一层:新弹窗移除旧弹窗", () => {
    const h1 = openModal(null, { title: "一" });
    const h2 = openModal(null, { title: "二" });
    expect(document.querySelectorAll(".overlay").length).toBe(1);
    h1.close(true);
    h2.close(true);
  });

  it("confirmDialog 确认回调执行后自动关闭", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    confirmDialog({ title: "停用账号", message: "停用后停止接单与发货", onConfirm });
    const btns = document.querySelectorAll(".overlay button");
    const confirm = Array.from(btns).find((b) => b.textContent === "确认") as HTMLButtonElement;
    confirm.click();
    await Promise.resolve();
    await Promise.resolve();
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".overlay")).toBeNull();
  });
});
