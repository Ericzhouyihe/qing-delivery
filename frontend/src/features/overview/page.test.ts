/** 概览统计加载行为测试(005/T022/T027):
 *  ①旧范围迟到响应不覆盖新范围(竞态守卫,spec Edge Case + FR-013);
 *  ②查询失败显示错误占位且统计卡保留上次值,重试可恢复(US2-AC4)。 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { overviewPage } from "./page";

type MockResponse = { ok: boolean; status: number; text: () => Promise<string> };
type Dispose = () => void;

function jsonResponse(status: number, body: string): MockResponse {
  return { ok: status >= 200 && status < 300, status, text: async () => body };
}

function statsPayload(revenueMinor: number): Record<string, unknown> {
  return {
    range: { from: 0, to: 86_400_000, granularity: "daily" },
    revenue: {
      minor_units: revenueMinor,
      currency: "CNY",
      order_count: 1,
      previous_minor_units: 0,
      change_percent: null
    },
    accounts: { online: 0, total: 0 },
    pending_issues: 0,
    stopping: false,
    restore: null,
    trend: []
  };
}

function otherResponse(url: string): MockResponse {
  if (url.includes("/api/v1/accounts")) return jsonResponse(200, JSON.stringify({ items: [] }));
  if (url.includes("/api/v1/orders"))
    return jsonResponse(200, JSON.stringify({ items: [], next_cursor: null }));
  if (url.includes("/api/v1/issues")) return jsonResponse(200, JSON.stringify({ items: [] }));
  return jsonResponse(200, "{}");
}

async function flush(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
  await new Promise((r) => setTimeout(r, 0));
}

function revValue(root: HTMLElement): string {
  const node = root.querySelector(".ov-value-row span");
  return node ? (node.textContent ?? "") : "(missing)";
}

/** PageFactory 返回值是联合类型,安全调用清理函数。 */
function callDispose(dispose: unknown): void {
  if (typeof dispose === "function") (dispose as Dispose)();
}

function mountPage(): { root: HTMLElement; dispose: () => void } {
  const root = document.createElement("div");
  const dispose = overviewPage(root);
  return { root, dispose: () => callDispose(dispose) };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("统计加载守卫(005/T022,Constitution IV 过期响应)", () => {
  it("旧范围迟到响应不覆盖新范围数据", async () => {
    let deferredResolve: ((r: MockResponse) => void) | null = null;
    const initialPending = new Promise<MockResponse>((resolve) => {
      deferredResolve = resolve;
    });
    let statsCall = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/stats/overview")) {
        statsCall += 1;
        if (statsCall === 1) return initialPending; // 初始 7天内 请求挂起
        return jsonResponse(200, JSON.stringify(statsPayload(500))); // "今天"立即返回
      }
      return otherResponse(url);
    });
    vi.stubGlobal("fetch", fetchMock);

    const { root, dispose } = mountPage();
    await flush();
    const todayChip = [...root.querySelectorAll(".range-chips .chip")].find(
      (c) => c.textContent === "今天"
    );
    expect(todayChip).toBeDefined();
    (todayChip as HTMLElement).dispatchEvent(new Event("click"));
    await flush();
    expect(revValue(root)).toBe("¥5.00");
    // 旧范围(7天内)响应迟到:守卫必须丢弃,不得覆盖
    deferredResolve!(jsonResponse(200, JSON.stringify(statsPayload(9900))));
    await flush();
    expect(revValue(root)).toBe("¥5.00");
    dispose();
  });
});

describe("统计查询失败占位(005/T027,US2-AC4)", () => {
  it("失败显示错误占位、统计卡保留上次值;重试恢复", async () => {
    let fail = true;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/stats/overview")) {
        if (fail) {
          return jsonResponse(
            500,
            JSON.stringify({ error: { code: "persistence_unavailable", message: "统计不可用" } })
          );
        }
        return jsonResponse(200, JSON.stringify(statsPayload(700)));
      }
      return otherResponse(url);
    });
    vi.stubGlobal("fetch", fetchMock);

    const { root, dispose } = mountPage();
    await flush();
    const trend = root.querySelector(".trend-content");
    expect(trend?.textContent).toContain("统计暂时不可用");
    expect(trend?.querySelector("button")?.textContent).toBe("重试");
    // 失败时不得闪 0,保留占位
    expect(revValue(root)).toBe("—");

    fail = false;
    (trend!.querySelector("button") as HTMLButtonElement).click();
    await flush();
    expect(root.querySelector(".trend-content")?.textContent).not.toContain("统计暂时不可用");
    expect(revValue(root)).toBe("¥7.00");
    dispose();
  });
});
