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
