import { describe, expect, it } from "vitest";
import { parseCardPool, parseStatsOverview } from "./contracts";

describe("parseStatsOverview 契约解析(005/T008)", () => {
  it("正常载荷完整解析", () => {
    const v = parseStatsOverview({
      range: { from: 100, to: 200, granularity: "daily" },
      revenue: {
        minor_units: 12300,
        currency: "CNY",
        order_count: 12,
        previous_minor_units: 600,
        change_percent: 200
      },
      accounts: { online: 1, total: 2 },
      pending_issues: 4,
      trend: [{ bucket_start: 100, minor_units: 100, order_count: 1 }]
    });
    expect(v.range.granularity).toBe("daily");
    expect(v.revenue.minor_units).toBe(12300);
    expect(v.revenue.change_percent).toBe(200);
    expect(v.accounts).toEqual({ online: 1, total: 2 });
    expect(v.pending_issues).toBe(4);
    expect(v.trend).toHaveLength(1);
  });

  it("change_percent 为 null 合法(前区间无数据)", () => {
    const v = parseStatsOverview({
      revenue: { minor_units: 0, change_percent: null },
      trend: []
    });
    expect(v.revenue.change_percent).toBeNull();
  });

  it("字段缺失降级为默认值,不抛错", () => {
    const v = parseStatsOverview({});
    expect(v.revenue.minor_units).toBe(0);
    expect(v.revenue.change_percent).toBeNull();
    expect(v.accounts.total).toBe(0);
    expect(v.pending_issues).toBe(0);
    expect(v.trend).toEqual([]);
    expect(v.range.granularity).toBe("hourly");
  });

  it("trend 非数组降级为空数组;元素非对象逐字段兜底", () => {
    const v = parseStatsOverview({ trend: "bad", pending_issues: "x" });
    expect(v.trend).toEqual([]);
    const w = parseStatsOverview({ trend: ["bad", { bucket_start: 5 }] });
    expect(w.trend).toEqual([{ bucket_start: 0, minor_units: 0, order_count: 0 }, { bucket_start: 5, minor_units: 0, order_count: 0 }]);
  });

  it("非对象输入抛错", () => {
    expect(() => parseStatsOverview("nope")).toThrow();
    expect(() => parseStatsOverview(null)).toThrow();
  });
});

describe("stats stock 可选字段(007 T014/T018,缺失不闪 0)", () => {
  it("缺失或形状不对 → 字段不设值(undefined)", () => {
    expect(parseStatsOverview({}).stock).toBeUndefined();
    expect(parseStatsOverview({ stock: null }).stock).toBeUndefined();
    expect(parseStatsOverview({ stock: { available_total: "x" } }).stock).toBeUndefined();
  });

  it("存在且为数字 → 取值", () => {
    expect(parseStatsOverview({ stock: { available_total: 12 } }).stock).toEqual({ available_total: 12 });
  });
});

describe("parseCardPool 卡密组契约解析(007 T015)", () => {
  it("正常载荷完整解析(transport summary_json 形状)", () => {
    const v = parseCardPool({
      id: "cpl-1",
      name: "网课组",
      kind: "data",
      enabled: true,
      delay_seconds: 30,
      description: "主推",
      version: 7,
      created_at: "t1",
      updated_at: "t2",
      stock: { available: 3, reserved: 1, used: 2 },
      content_set: null,
      api_config: {
        url: "https://api.example.com/card",
        method: "POST",
        timeout_ms: 5000,
        content_type: "application/json",
        response_path: "data.card",
        retry_enabled: true,
        headers_configured: true,
        params_configured: false,
        body_configured: true
      }
    });
    expect(v.stock).toEqual({ available: 3, reserved: 1, used: 2 });
    expect(v.api_config?.headers_configured).toBe(true);
    expect(v.kind).toBe("data");
    expect(v.version).toBe(7);
  });

  it("可选字段缺失降级:stock/content_set/api_config 不设值", () => {
    const v = parseCardPool({ id: "cpl-2", name: "文本组", kind: "text", content_set: true });
    expect(v.stock).toBeUndefined();
    expect(v.api_config).toBeUndefined();
    expect(v.content_set).toBe(true);
    expect(v.enabled).toBe(false);
    expect(v.version).toBe(1);
  });

  it("kind 未知降级为 data;非对象抛错", () => {
    expect(parseCardPool({ kind: "voice" }).kind).toBe("data");
    expect(() => parseCardPool("nope")).toThrow();
    expect(() => parseCardPool(null)).toThrow();
  });
});
