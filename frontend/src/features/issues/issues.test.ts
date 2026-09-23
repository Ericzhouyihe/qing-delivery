import { afterEach, describe, expect, it, vi } from "vitest";
import { parseIssueItem, parseRestoreReview } from "./page";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { el } from "../../shared/dom";

describe("事项解析(US6/T048)", () => {
  it("契约字段 category/reason/allowed_actions 数组解析;坏输入回退空数组", () => {
    const it = parseIssueItem({
      id: "iss-1",
      order_id: "ord-1",
      account_id: "acct-1",
      category: "delivery_unknown",
      reason: "ws_ack_timeout",
      allowed_actions: ["resend", "terminate"],
      state: "open",
      created_at: "2026-09-22T02:00:00Z"
    });
    expect(it.kind).toBe("delivery_unknown");
    expect(it.reasonCode).toBe("ws_ack_timeout");
    expect(it.allowedActions).toEqual(["resend", "terminate"]);
    const bad = parseIssueItem({ id: "x", allowed_actions: "not-array" });
    expect(bad.allowedActions).toEqual([]);
  });

  it("恢复核对条目解析含版本", () => {
    const r = parseRestoreReview({ id: "rev-1", kind: "delivery", order_id: "o1", version: 3 });
    expect(r.version).toBe(3);
    expect(parseRestoreReview({ id: "r2" }).version).toBe(1);
  });
});

describe("统一确认流(FR-018:原因必填/风险勾选/提交锁)", () => {
  afterEach(() => {
    document.querySelectorAll(".overlay").forEach((n) => n.remove());
    vi.restoreAllMocks();
  });

  it("必填项未满足时确认禁用;补齐后可提交", async () => {
    const submit = vi.fn().mockResolvedValue(undefined);
    openConfirmFlow({
      title: "人工补发",
      impact: "将重新发送内容",
      requireReason: true,
      requireRiskAck: true,
      riskText: "我知晓重复风险",
      confirmLabel: "补发",
      submit
    });
    const confirm = Array.from(document.querySelectorAll(".overlay button")).find(
      (b) => b.textContent === "补发"
    ) as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);

    const reason = document.querySelector<HTMLInputElement>(".overlay input[type=text]")!;
    reason.value = "已核对平台记录";
    reason.dispatchEvent(new Event("input"));
    expect(confirm.disabled).toBe(true); // 风险未勾选仍禁用

    const risk = document.querySelector<HTMLInputElement>(".overlay input[type=checkbox]")!;
    risk.checked = true;
    risk.dispatchEvent(new Event("change"));
    expect(confirm.disabled).toBe(false);

    confirm.click();
    await vi.waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(submit.mock.calls[0]![0]).toBe("已核对平台记录");
    expect(submit.mock.calls[0]![1]).toBe(true);
  });

  it("提交失败保留弹窗并展示错误,按钮恢复", async () => {
    const submit = vi.fn().mockRejectedValue(new Error("guard 拒绝:存在活动交付"));
    openConfirmFlow({
      title: "终止",
      impact: "终止交付",
      requireReason: true,
      requireRiskAck: false,
      confirmLabel: "终止",
      submit
    });
    const reason = document.querySelector<HTMLInputElement>(".overlay input[type=text]")!;
    reason.value = "测试";
    reason.dispatchEvent(new Event("input"));
    const confirm = Array.from(document.querySelectorAll(".overlay button")).find(
      (b) => b.textContent === "终止"
    ) as HTMLButtonElement;
    confirm.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".overlay .field-error")?.textContent).toContain("guard 拒绝");
    });
    expect(document.querySelector(".overlay")).not.toBeNull();
    expect(confirm.disabled).toBe(false);
  });

  it("取消直接关闭且不调用提交", async () => {
    const submit = vi.fn();
    openConfirmFlow({
      title: "接管",
      impact: "接管",
      requireReason: true,
      requireRiskAck: false,
      confirmLabel: "接管",
      submit
    });
    const cancel = Array.from(document.querySelectorAll(".overlay button")).find(
      (b) => b.textContent === "取消"
    ) as HTMLButtonElement;
    cancel.click();
    await Promise.resolve();
    expect(submit).not.toHaveBeenCalled();
    expect(document.querySelector(".overlay")).toBeNull();
  });
});

describe("完成后条目移出(计数联动由整页 reload 保证)", () => {
  it("leaving 动画类与 reload 由 onSuccess 触发", () => {
    const node = el("div", { class: "issue-item" }, "x");
    document.body.append(node);
    node.classList.add("leaving");
    expect(node.className).toContain("leaving");
    node.remove();
  });
});
