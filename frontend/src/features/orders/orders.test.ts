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
  parseOrderRow,
  parseSyncReports,
  singleSyncOutcomeLabel,
  syncFailedDetail,
  syncReportLine
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

  it("人工触发交付/确认平台已发货仅携带原因(无风险勾选字段,US6/T071)", () => {
    expect(manualActionBody("trigger_delivery", "买家催促发货", false)).toEqual({ reason: "买家催促发货" });
    expect(manualActionBody("confirm_shipment", "平台侧已发货", true)).toEqual({ reason: "平台侧已发货" });
  });
});

describe("两新人工动作可用性(US6/FR-061/FR-062,T071)", () => {
  it("confirm_shipment 仅在内容 accepted(delivered)时可用;否则禁用并注明原因", () => {
    expect(actionAvailability("confirm_shipment", "delivered", null).enabled).toBe(true);
    for (const state of ["unknown", "needs_review", "ready", "sending", "terminated", null]) {
      const no = actionAvailability("confirm_shipment", state, null);
      expect(no.enabled).toBe(false);
      expect(no.reason).toContain("需内容交付 accepted");
    }
  });

  it("trigger_delivery 在未 accepted 且未终止时可用;已交付/已终止禁用", () => {
    for (const state of ["needs_review", "unknown", "ready", "sending", "definitely_not_sent"]) {
      expect(actionAvailability("trigger_delivery", state, null).enabled).toBe(true);
    }
    const done = actionAvailability("trigger_delivery", "delivered", null);
    expect(done.enabled).toBe(false);
    expect(done.reason).toContain("已交付");
    expect(actionAvailability("trigger_delivery", "terminated", null).enabled).toBe(false);
  });
});

describe("单笔同步 outcome 中文映射(US6/T071)", () => {
  it("六种 HandleOutcome 全量映射;未知值原样呈现", () => {
    expect(singleSyncOutcomeLabel("delivered")).toBe("已交付");
    expect(singleSyncOutcomeLabel("ineligible")).toBe("不合格");
    expect(singleSyncOutcomeLabel("not_sent")).toBe("未发送");
    expect(singleSyncOutcomeLabel("unknown")).toBe("结果未知");
    expect(singleSyncOutcomeLabel("already_handled")).toBe("已处理过");
    expect(singleSyncOutcomeLabel("busy")).toBe("忙");
    expect(singleSyncOutcomeLabel("future_outcome")).toBe("future_outcome");
  });
});

describe("一键同步逐账号摘要派生(US6/T071,纯函数)", () => {
  const resultRef = JSON.stringify({
    accounts: [
      {
        account_id: "acct-1",
        orders_seen: 5,
        created: 2,
        restored: 1,
        reassigned: 1,
        ineligible: 1,
        failed: [
          { order_id: "XY-9", reason: "平台限流" },
          { order_id: null, reason: "追溯失败:超时" }
        ],
        coverage: { complete: true }
      },
      { account_id: "acct-2", offline: true, orders_seen: 0 }
    ]
  });

  it("解析报告并派生逐账号摘要行;离线账号标注跳过", () => {
    const reports = parseSyncReports(resultRef);
    expect(reports).toHaveLength(2);
    expect(reports[0]!.ordersSeen).toBe(5);
    expect(syncReportLine(reports[0]!, "小店")).toBe("小店:新建 2 · 恢复 1 · 修正 1 · 不合格 1 · 失败 2");
    expect(syncReportLine(reports[1]!, "备用号")).toContain("备用号");
    expect(syncReportLine(reports[1]!)).toContain("离线跳过");
  });

  it("失败明细悬浮文本逐条列出(无失败订单为空)", () => {
    const reports = parseSyncReports(resultRef);
    expect(syncFailedDetail(reports[0]!)).toBe("XY-9:平台限流\n未知订单:追溯失败:超时");
    expect(syncFailedDetail(reports[1]!)).toBe("");
  });

  it("result_ref 缺失/非 JSON/非契约形状时安全回退空数组", () => {
    expect(parseSyncReports(null)).toEqual([]);
    expect(parseSyncReports("")).toEqual([]);
    expect(parseSyncReports("not-json")).toEqual([]);
    expect(parseSyncReports(JSON.stringify({ items: [] }))).toEqual([]);
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

  it("accepted 订单:确认平台已发货可用、人工触发交付禁用并附原因(T071)", async () => {
    vi.mocked(httpGet).mockResolvedValue({
      summary: {},
      delivery: {
        id: "d1",
        state: "delivered",
        raw_content_state: "accepted",
        review_state: "none",
        confirmation_state: "pending",
        accepted_evidence: "none",
        automatic_retries_used: 0,
        attempts: []
      }
    });
    vi.mocked(httpPost).mockResolvedValue({ operation_id: "op-1", platform_confirmed: true });
    const row = parseOrderRow({ id: "ord-3", account_id: "acct-1", platform_order_id: "XY-3" });
    openOrderDetail(row, () => undefined);
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain("人工动作");
    });
    const buttons = [...document.querySelectorAll<HTMLButtonElement>(".drawer button")];
    const confirmBtn = buttons.find((b) => b.textContent === "确认平台已发货");
    const triggerBtn = buttons.find((b) => b.textContent === "人工触发交付");
    expect(confirmBtn).toBeDefined();
    expect(confirmBtn!.disabled).toBe(false);
    expect(triggerBtn).toBeDefined();
    expect(triggerBtn!.disabled).toBe(true);
    expect(triggerBtn!.title).toContain("已交付");
  });

  it("单笔同步按钮 POST .../syncs 并以中文呈现 outcome(T071)", async () => {
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
    vi.mocked(httpPost).mockResolvedValue({ outcome: "already_handled" });
    let reloaded = false;
    const row = parseOrderRow({ id: "ord-4", account_id: "acct-1", platform_order_id: "XY-4" });
    openOrderDetail(row, () => {
      reloaded = true;
    });
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain("订单同步");
    });
    const syncBtn = [...document.querySelectorAll<HTMLButtonElement>(".drawer button")].find(
      (b) => b.textContent === "单笔同步"
    );
    expect(syncBtn).toBeDefined();
    syncBtn!.click();
    await vi.waitFor(() => {
      expect(httpPost).toHaveBeenCalledWith("/api/v1/accounts/acct-1/orders/ord-4/syncs", {});
    });
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer")?.textContent).toContain("结果:已处理过");
      expect(reloaded).toBe(true);
    });
  });

  it("人工触发交付确认流仅提交原因;422 缺失要素呈现于错误行(T071)", async () => {
    vi.mocked(httpGet).mockResolvedValue({
      summary: {},
      delivery: {
        id: "d1",
        state: "needs_review",
        raw_content_state: "unknown",
        review_state: "required",
        confirmation_state: "disabled",
        accepted_evidence: "none",
        automatic_retries_used: 0,
        attempts: []
      }
    });
    vi.mocked(httpPost).mockRejectedValueOnce(
      new Error('付款事实不完整,缺失:["paid_at(付款时间)", "amount(金额)"];请先经订单同步或等待平台事件补全')
    );
    const row = parseOrderRow({ id: "ord-5", account_id: "acct-1", platform_order_id: "XY-5" });
    openOrderDetail(row, () => undefined);
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain("人工动作");
    });
    const triggerBtn = [...document.querySelectorAll<HTMLButtonElement>(".drawer button")].find(
      (b) => b.textContent === "人工触发交付"
    );
    expect(triggerBtn).toBeDefined();
    expect(triggerBtn!.disabled).toBe(false);
    triggerBtn!.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".overlay")).not.toBeNull();
    });
    const reasonInput = document.querySelector<HTMLInputElement>(".modal input[type=text]");
    reasonInput!.value = "买家已付款,补齐事实后人工触发";
    reasonInput!.dispatchEvent(new Event("input", { bubbles: true }));
    const submitBtn = [...document.querySelectorAll<HTMLButtonElement>(".modal button")].find(
      (b) => b.textContent === "人工触发交付"
    );
    await vi.waitFor(() => {
      expect(submitBtn!.disabled).toBe(false);
    });
    submitBtn!.click();
    await vi.waitFor(() => {
      // 新动作提交体不带任何风险勾选旗标
      expect(httpPost).toHaveBeenCalledWith("/api/v1/accounts/acct-1/orders/ord-5/deliveries", {
        reason: "买家已付款,补齐事实后人工触发"
      });
    });
    await vi.waitFor(() => {
      expect(document.querySelector(".modal")?.textContent).toContain("缺失");
      expect(document.querySelector(".modal")?.textContent).toContain("paid_at");
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
