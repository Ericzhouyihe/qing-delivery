/** 收敛任务测试(T063 揭示方法 / T066 分页保留筛选)。 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { el } from "../../shared/dom";
import { ordersPage } from "./page";
import { openOrderDetail } from "./detail";
import { parseOrderRow } from "./model";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

const ORDER = (id: string): Record<string, unknown> => ({
  id,
  account_id: "acct-1",
  platform_order_id: `XY-${id}`,
  paid_at: "2026-09-23T02:00:00Z",
  amount: { minor_units: 1000, currency: "CNY" },
  delivery_state: "delivered",
  confirmation_state: "succeeded"
});

describe("订单页:游标分页保留筛选(US5-1/US5-5,T066)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  const orderUrls: string[] = [];

  beforeEach(() => {
    orderUrls.length = 0;
    fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/api/v1/accounts")) {
        return Promise.resolve(jsonResponse({ items: [{ id: "acct-1", display_name: "小店" }] }));
      }
      if (url.includes("/api/v1/orders")) {
        orderUrls.push(url);
        const hasCursor = url.includes("cursor=c1");
        if (hasCursor) {
          return Promise.resolve(jsonResponse({ items: [ORDER("o3")], next_cursor: null }));
        }
        return Promise.resolve(jsonResponse({ items: [ORDER("o1"), ORDER("o2")], next_cursor: "c1" }));
      }
      return Promise.resolve(jsonResponse({}, Number((init?.method ?? "GET") === "POST" ? 202 : 200)));
    });
    vi.stubGlobal("fetch", fetchMock);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    document.querySelectorAll(".drawer, .drawer-overlay").forEach((n) => n.remove());
  });

  it("设置筛选后翻页:第二页请求同时携带 cursor 与筛选参数;上一页返回去掉游标但保留筛选", async () => {
    const root = el("div");
    document.body.append(root);
    ordersPage(root);
    await vi.waitFor(() => {
      expect(root.querySelectorAll("table.data tr.row-click").length).toBe(2);
    });

    // 选择账号筛选 → 重新加载且请求带 account_id
    const accountSel = root.querySelectorAll<HTMLSelectElement>(".filter-bar select")[0]!;
    accountSel.value = "acct-1";
    accountSel.dispatchEvent(new Event("change"));
    await vi.waitFor(() => {
      expect(orderUrls.some((u) => u.includes("account_id=acct-1") && !u.includes("cursor="))).toBe(true);
    });

    // 下一页:cursor 与筛选共存
    const next = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "下一页 →") as HTMLButtonElement;
    expect(next.disabled).toBe(false);
    next.click();
    await vi.waitFor(() => {
      expect(root.textContent).toContain("第 2 页");
    });
    const page2Url = orderUrls.find((u) => u.includes("cursor=c1"));
    expect(page2Url).toBeDefined();
    expect(page2Url!).toContain("account_id=acct-1");

    // 末页:下一页禁用;上一页返回无 cursor 但保留筛选
    expect((Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "下一页 →") as HTMLButtonElement).disabled).toBe(true);
    const prev = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "← 上一页") as HTMLButtonElement;
    prev.click();
    await vi.waitFor(() => {
      expect(root.textContent).toContain("第 1 页");
    });
    const afterPrev = orderUrls.filter((u) => !u.includes("cursor=")).length;
    expect(afterPrev).toBeGreaterThanOrEqual(2);
    expect(orderUrls.every((u) => u.includes("limit=50"))).toBe(true);
  });
});

describe("订单详情:内容揭示走授权 POST 端点(T063/FR-017)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    document.querySelectorAll(".drawer, .drawer-overlay").forEach((n) => n.remove());
  });

  it("揭示以 POST 调用 /content 并携带 CSRF 头;404 时如实呈现服务端错误", async () => {
    const calls: Array<{ url: string; method: string; csrf: string | null }> = [];
    const fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      calls.push({ url, method, csrf: (init?.headers as Record<string, string>)?.["X-CSRF-Token"] ?? null });
      if (url.includes("/orders/o1") && !url.includes("/content")) {
        return Promise.resolve(
          jsonResponse({
            summary: ORDER("o1"),
            delivery: {
              id: "d1",
              state: "delivered",
              raw_content_state: "accepted",
              review_state: "none",
              confirmation_state: "succeeded",
              accepted_evidence: "platform_event",
              automatic_retries_used: 0,
              attempts: [{ id: "a1", kind: "send_message", sequence: 1, state: "succeeded" }]
            }
          })
        );
      }
      if (url.includes("/content")) {
        return Promise.resolve(
          jsonResponse({ error: { code: "resource_not_found", message: "订单无冻结内容", request_id: "r", retryable: false } }, 404)
        );
      }
      return Promise.resolve(jsonResponse({}));
    });
    vi.stubGlobal("fetch", fetchMock);

    openOrderDetail(parseOrderRow(ORDER("o1")), () => undefined);
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer")?.textContent).toContain("状态时间线");
    });
    const reveal = Array.from(document.querySelectorAll<HTMLButtonElement>(".drawer button")).find((b) =>
      (b.textContent ?? "").includes("揭示")
    )!;
    reveal.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer")?.textContent).toContain("订单无冻结内容");
    });
    const contentCall = calls.find((c) => c.url.includes("/content"));
    expect(contentCall?.method).toBe("POST");
    expect(contentCall?.csrf).not.toBeNull();
  });

  it("揭示成功路径:POST 返回内容则折叠展示并可收起", async () => {
    const fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/content")) {
        return Promise.resolve(jsonResponse({ content: "链接 https://x 提取码 a1b2", content_hash: "h", content_kind: "fixed_text" }));
      }
      if (url.includes("/orders/o1")) {
        return Promise.resolve(
          jsonResponse({
            summary: ORDER("o1"),
            delivery: {
              id: "d1", state: "delivered", raw_content_state: "accepted", review_state: "none",
              confirmation_state: "succeeded", accepted_evidence: "platform_event", automatic_retries_used: 0, attempts: []
            }
          })
        );
      }
      return Promise.resolve(jsonResponse({}, (init?.method ?? "GET") === "POST" ? 202 : 200));
    });
    vi.stubGlobal("fetch", fetchMock);
    openOrderDetail(parseOrderRow(ORDER("o1")), () => undefined);
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer")?.textContent).toContain("状态时间线");
    });
    const reveal = Array.from(document.querySelectorAll<HTMLButtonElement>(".drawer button")).find((b) =>
      (b.textContent ?? "").includes("揭示")
    )!;
    reveal.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer .reveal-box pre")?.textContent).toContain("提取码 a1b2");
    });
    // 收起
    reveal.click();
    await vi.waitFor(() => {
      expect(document.querySelector(".drawer .reveal-box pre")).toBeNull();
    });
  });
});

describe("订单页:一键同步任务轮询与中途取消(US6/T071)", () => {
  const ACCOUNTS = { items: [{ id: "acct-1", display_name: "小店" }, { id: "acct-2", display_name: "备用号" }] };
  const REPORTS = JSON.stringify({
    accounts: [
      {
        account_id: "acct-1",
        orders_seen: 3,
        created: 1,
        restored: 1,
        reassigned: 0,
        ineligible: 1,
        failed: [{ order_id: "XY-8", reason: "平台限流" }],
        coverage: { complete: true }
      },
      { account_id: "acct-2", offline: true, orders_seen: 0 }
    ]
  });

  const stubJobs = (
    fetchMock: ReturnType<typeof vi.fn>,
    states: Array<{ state: string; version: number; result_ref?: string; safe_error?: string }>,
    cancelAllowedFor: (state: string) => boolean
  ): void => {
    let polls = 0;
    fetchMock.mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/api/v1/orders/syncs")) {
        return Promise.resolve(jsonResponse({ operation_id: "op-1", job: { id: "job-1", kind: "order_sync", state: "queued", version: 1 } }, 202));
      }
      if (url.includes("/api/v1/jobs/job-1/cancel")) {
        return Promise.resolve(jsonResponse({ operation_id: "op-2", job: { id: "job-1", state: "cancel_requested", version: 99 } }, 202));
      }
      if (url.includes("/api/v1/jobs/job-1")) {
        const s = states[Math.min(polls, states.length - 1)]!;
        polls += 1;
        return Promise.resolve(
          jsonResponse({
            summary: { id: "job-1", state: s.state, version: s.version, result_ref: s.result_ref, safe_error: s.safe_error },
            stage: "",
            cancel_allowed: cancelAllowedFor(s.state)
          })
        );
      }
      if (url.includes("/api/v1/accounts")) {
        return Promise.resolve(jsonResponse(ACCOUNTS));
      }
      if (url.includes("/api/v1/orders?")) {
        return Promise.resolve(jsonResponse({ items: [ORDER("o1")], next_cursor: null }));
      }
      return Promise.resolve(jsonResponse({}, (init?.method ?? "GET") === "POST" ? 202 : 200));
    });
  };

  const startSyncFromToolbar = async (root: HTMLElement): Promise<void> => {
    const syncBtn = Array.from(root.querySelectorAll("button")).find(
      (b) => b.textContent === "一键同步订单"
    ) as HTMLButtonElement;
    syncBtn.click();
    const confirm = Array.from(document.querySelectorAll(".modal button")).find(
      (b) => b.textContent === "开始同步"
    ) as HTMLButtonElement;
    confirm.click();
    await vi.advanceTimersByTimeAsync(0); // 发起 POST /orders/syncs 并进入轮询循环
  };

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    document.querySelectorAll(".drawer, .drawer-overlay, .overlay").forEach((n) => n.remove());
    document.querySelectorAll("main, div").forEach((n) => {
      if (n instanceof HTMLElement && n.dataset.testRoot === "1") n.remove();
    });
  });

  it("发起同步 → 轮询至 succeeded → 逐账号摘要 + 失败明细悬浮;完成后刷新列表", async () => {
    const fetchMock = vi.fn();
    stubJobs(fetchMock, [
      { state: "running", version: 3 },
      { state: "succeeded", version: 4, result_ref: REPORTS }
    ], (s) => s !== "succeeded");
    vi.stubGlobal("fetch", fetchMock);
    vi.useFakeTimers();
    const root = el("div", { "data-test-root": "1" }) as HTMLElement;
    document.body.append(root);
    ordersPage(root);
    await vi.advanceTimersByTimeAsync(0);
    expect(root.querySelectorAll("table.data tr.row-click").length).toBe(1);
    const listCallsBefore = fetchMock.mock.calls.filter((c) => String(c[0]).includes("/api/v1/orders?")).length;

    await startSyncFromToolbar(root);
    expect(fetchMock.mock.calls.some((c) => String(c[0]).includes("/api/v1/orders/syncs"))).toBe(true);

    // 第一轮:running → 反馈行进行中 + 取消任务可见
    await vi.advanceTimersByTimeAsync(2_000);
    expect(root.textContent).toContain("同步中");
    const cancelBtn = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "取消任务") as HTMLButtonElement;
    expect(cancelBtn.style.display).not.toBe("none");

    // 第二轮:succeeded → 摘要行 + 悬浮明细 + 列表刷新
    await vi.advanceTimersByTimeAsync(2_000);
    await vi.advanceTimersByTimeAsync(0);
    expect(root.textContent).toContain("同步完成(2 个账号)");
    expect(root.textContent).toContain("小店:新建 1 · 恢复 1 · 修正 0 · 不合格 1 · 失败 1");
    expect(root.textContent).toContain("备用号:离线跳过");
    const titles = Array.from(root.querySelectorAll("[title]")).map((n) => n.getAttribute("title") ?? "");
    expect(titles.some((t) => t.includes("XY-8:平台限流"))).toBe(true);
    expect(Array.from(root.querySelectorAll("button")).some((b) => b.textContent === "一键同步订单")).toBe(true);
    const listCallsAfter = fetchMock.mock.calls.filter((c) => String(c[0]).includes("/api/v1/orders?")).length;
    expect(listCallsAfter).toBe(listCallsBefore + 1);
  });

  it("取消任务:POST /jobs/{id}/cancel 携带 expected_version;终态 cancelled 如实呈现已处理部分", async () => {
    const fetchMock = vi.fn();
    stubJobs(fetchMock, [
      { state: "running", version: 2 },
      { state: "cancel_requested", version: 5 },
      { state: "cancelled", version: 6, result_ref: JSON.stringify({ accounts: [{ account_id: "acct-1", orders_seen: 1, created: 1, restored: 0, reassigned: 0, ineligible: 0, failed: [] }] }) }
    ], (s) => s === "running");
    vi.stubGlobal("fetch", fetchMock);
    vi.useFakeTimers();
    const root = el("div", { "data-test-root": "1" }) as HTMLElement;
    document.body.append(root);
    ordersPage(root);
    await vi.advanceTimersByTimeAsync(0);
    await startSyncFromToolbar(root);

    // 第一轮 running(version 2)→ 点击取消任务
    await vi.advanceTimersByTimeAsync(2_000);
    const cancelBtn = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "取消任务") as HTMLButtonElement;
    cancelBtn.click();
    await vi.advanceTimersByTimeAsync(0);
    const cancelCall = fetchMock.mock.calls.find((c) => String(c[0]).includes("/jobs/job-1/cancel"));
    expect(cancelCall).toBeDefined();
    expect(JSON.parse(String((cancelCall![1] as RequestInit | undefined)?.body))).toEqual({
      expected_version: 2,
      reason: "用户取消"
    });

    // 第二轮 cancel_requested → 等待账号边界停下
    await vi.advanceTimersByTimeAsync(2_000);
    expect(root.textContent).toContain("已请求取消");

    // 第三轮 cancelled → 已处理部分保留 + 部分摘要
    await vi.advanceTimersByTimeAsync(2_000);
    expect(root.textContent).toContain("已取消,已处理部分保留(1 个账号)");
    expect(root.textContent).toContain("小店:新建 1 · 恢复 0 · 修正 0 · 不合格 0 · 失败 0");
  });

  it("终态 failed:如实展示 safe_error,不虚报", async () => {
    const fetchMock = vi.fn();
    stubJobs(fetchMock, [
      { state: "running", version: 2 },
      { state: "failed", version: 3, safe_error: "数据库锁定" }
    ], (s) => s === "running");
    vi.stubGlobal("fetch", fetchMock);
    vi.useFakeTimers();
    const root = el("div", { "data-test-root": "1" }) as HTMLElement;
    document.body.append(root);
    ordersPage(root);
    await vi.advanceTimersByTimeAsync(0);
    await startSyncFromToolbar(root);
    await vi.advanceTimersByTimeAsync(2_000);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(root.textContent).toContain("同步失败:数据库锁定");
  });
});
