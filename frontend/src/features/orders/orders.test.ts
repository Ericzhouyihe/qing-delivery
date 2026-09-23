import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../shared/http", () => ({
  httpGet: vi.fn(),
  httpPost: vi.fn()
}));

import {
  actionAvailability,
  applyContentStateFilter,
  filtersToQuery,
  manualActionBody,
  parseOrderRow
} from "./model";
import { buildTimelineNodes, openOrderDetail } from "./detail";
import { renderTimeline } from "../../ui/timeline";
import { httpGet, httpPost } from "../../shared/http";

describe("订单筛选与查询(FR-015/T043)", () => {
  it("filtersToQuery 仅携带非空参数", () => {
    expect(filtersToQuery({ orderId: "", accountId: "", contentState: "", platformState: "", from: "", to: "" })).toBe(
      "/api/v1/orders?limit=50"
    );
    const q = filtersToQuery({
      orderId: "XY-99",
      accountId: "acct-1",
      contentState: "delivered",
      platformState: "pending_ship",
      from: "2026-09-22",
      to: ""
    });
    expect(q).toContain("platform_order_id=XY-99");
    expect(q).toContain("account_id=acct-1");
    expect(q).toContain("platform_state=pending_ship");
    expect(q).toContain("from=2026-09-22");
    expect(q).not.toContain("to=");
    expect(q).not.toContain("contentState");
  });

  it("内容轴过滤为客户端二次过滤", () => {
    const rows = [
      parseOrderRow({ id: "1", delivery_state: "delivered" }),
      parseOrderRow({ id: "2", delivery_state: "unknown" })
    ];
    expect(applyContentStateFilter(rows, "unknown").map((r) => r.id)).toEqual(["2"]);
    expect(applyContentStateFilter(rows, "").length).toBe(2);
  });

  it("解析容错:缺省字段安全回退", () => {
    const r = parseOrderRow({ id: "o1" });
    expect(r.paidAt).toBeNull();
    expect(r.deliveryState).toBeNull();
    expect(r.platformState).toBe("unknown");
  });
});

describe("人工动作显示可用性(FR-017:禁用附原因,绝不点击后才报错)", () => {
  it("补发仅限等待人工/确定未发送/结果未知", () => {
    expect(actionAvailability("resend", "needs_review", null).enabled).toBe(true);
    expect(actionAvailability("resend", "definitely_not_sent", null).enabled).toBe(true);
    expect(actionAvailability("resend", "unknown", null).enabled).toBe(true);
    const no = actionAvailability("resend", "delivered", null);
    expect(no.enabled).toBe(false);
    expect(no.reason).toContain("不在可补发范围");
  });

  it("确认收到仅在确认失败/未知时可用;终止/接管在已终止时禁用", () => {
    expect(actionAvailability("mark_received", "delivered", "failed").enabled).toBe(true);
    expect(actionAvailability("mark_received", "delivered", "unknown").enabled).toBe(true);
    expect(actionAvailability("mark_received", "delivered", "succeeded").enabled).toBe(false);
    expect(actionAvailability("terminate", "terminated", null).enabled).toBe(false);
    expect(actionAvailability("takeover", "terminated", null).enabled).toBe(false);
    expect(actionAvailability("takeover", "sending", null).enabled).toBe(true);
  });
});

describe("人工动作提交体(T064/FR-022:旗标按动作语义映射)", () => {
  it("补发携带重复风险确认;接管携带历史范围确认", () => {
    expect(manualActionBody("resend", "买家反馈未收到", true)).toEqual({
      reason: "买家反馈未收到",
      acknowledge_duplicate_risk: true
    });
    expect(manualActionBody("takeover", "接入前订单", true)).toEqual({
      reason: "接入前订单",
      acknowledge_historical_scope: true
    });
  });

  it("终止/确认收到仅携带原因", () => {
    expect(manualActionBody("terminate", "买家已退款", false)).toEqual({ reason: "买家已退款" });
    expect(manualActionBody("mark_received", "平台侧已核对", false)).toEqual({ reason: "平台侧已核对" });
  });
});

describe("内容揭示与人工动作接线(T063/T064)", () => {
  afterEach(() => {
    document.querySelectorAll(".drawer-overlay").forEach((n) => n.remove());
    document.querySelectorAll(".overlay").forEach((n) => n.remove());
    vi.clearAllMocks();
  });

  it("揭示按钮以 POST 调用授权端点(端点仅接受 POST,T063)", async () => {
    vi.mocked(httpGet).mockResolvedValue({
      summary: {},
      delivery: {
        id: "d1",
        state: "delivered",
        raw_content_state: "delivered",
        review_state: "none",
        confirmation_state: "disabled",
        accepted_evidence: "none",
        automatic_retries_used: 0,
        attempts: []
      }
    });
    vi.mocked(httpPost).mockResolvedValue({
      content: "https://example.com/dl/abc123",
      content_hash: "h",
      content_kind: "fixed_text"
    });
    const row = parseOrderRow({ id: "ord-1", account_id: "acct-1", platform_order_id: "XY-1" });
    openOrderDetail(row, () => undefined);
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer")).not.toBeNull();
    });
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain("交付内容");
    });
    const revealBtn = [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
      b.textContent?.includes("揭示交付内容")
    );
    expect(revealBtn).toBeDefined();
    revealBtn!.click();
    await vi.waitFor(() => {
      expect(httpPost).toHaveBeenCalled();
    });
    const [url] = vi.mocked(httpPost).mock.calls[0]!;
    expect(String(url)).toContain("/api/v1/accounts/acct-1/orders/ord-1/content");
    expect(document.body.textContent).toContain("https://example.com/dl/abc123");
  });

  it("takeover 确认流提交 acknowledge_historical_scope", async () => {
    vi.mocked(httpGet).mockResolvedValue({
      summary: {},
      delivery: {
        id: "d1",
        state: "unknown",
        raw_content_state: "unknown",
        review_state: "none",
        confirmation_state: "disabled",
        accepted_evidence: "none",
        automatic_retries_used: 0,
        attempts: []
      }
    });
    vi.mocked(httpPost).mockResolvedValue({ operation_id: "op-1" });
    const row = parseOrderRow({ id: "ord-2", account_id: "acct-1", platform_order_id: "XY-2" });
    openOrderDetail(row, () => undefined);
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain("人工动作");
    });
    const takeoverBtn = [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
      b.textContent === "人工接管"
    );
    expect(takeoverBtn).toBeDefined();
    takeoverBtn!.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".overlay")).not.toBeNull();
    });
    const reasonInput = document.querySelector<HTMLInputElement>(".modal input[type=text]");
    reasonInput!.value = "接入前的历史订单";
    reasonInput!.dispatchEvent(new Event("input", { bubbles: true }));
    const scopeCheck = document.querySelector<HTMLInputElement>(".modal input[type=checkbox]");
    scopeCheck!.checked = true;
    scopeCheck!.dispatchEvent(new Event("change", { bubbles: true }));
    const submitBtn = [...document.querySelectorAll<HTMLButtonElement>(".modal button")].find((b) =>
      b.textContent === "人工接管"
    );
    await vi.waitFor(() => {
      expect(submitBtn!.disabled).toBe(false);
    });
    submitBtn!.click();
    await vi.waitFor(() => {
      expect(httpPost).toHaveBeenCalledWith(
        "/api/v1/accounts/acct-1/orders/ord-2/takeovers",
        { reason: "接入前的历史订单", acknowledge_historical_scope: true }
      );
    });
  });
});

describe("三轴时间线(US5/FR-016)", () => {
  it("内容/审核/确认三轴节点齐全;none/disabled 不出节点", () => {
    const row = parseOrderRow({ id: "o1", paid_at: "2026-09-22T02:00:00Z" });
    const nodes = buildTimelineNodes(row, {
      summary: null,
      delivery: {
        id: "d1",
        state: "unknown",
        rawContentState: "unknown",
        reviewState: "required",
        confirmationState: "unknown",
        acceptedEvidence: "none",
        automaticRetriesUsed: 1,
        attempts: []
      }
    });
    const axes = nodes.map((n) => n.axis);
    expect(axes).toContain("订单");
    expect(axes).toContain("内容交付");
    expect(axes).toContain("审核");
    expect(axes).toContain("平台确认");
    const dom = renderTimeline(nodes);
    expect(dom.querySelectorAll("li").length).toBe(4);
    expect(dom.textContent).toContain("结果未知");
    expect(dom.textContent).toContain("待人工审核");
  });
});
