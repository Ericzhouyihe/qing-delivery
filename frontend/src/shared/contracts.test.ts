import { describe, expect, it } from "vitest";
import { parseStatsOverview } from "./contracts";

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
