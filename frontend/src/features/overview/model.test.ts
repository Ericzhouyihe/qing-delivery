import { describe, expect, it } from "vitest";
import {
  PRESET_OPTIONS,
  accountSortPriority,
  deriveCards,
  deriveStats,
  formatYuan,
  localDayStart,
  resolveCustomRange,
  resolvePresetRange,
  sortQuickAccounts,
  type QuickAccount,
  type RangePreset
} from "./model";
import type { DashboardSummary, StatsOverview } from "../../shared/contracts";

function dash(over: Partial<DashboardSummary> = {}): DashboardSummary {
  return {
    accounts: [
      { id: "a1", display_name: "正常店", connection_state: "online", run_enabled: true, auto_delivery_enabled: true },
      { id: "a2", display_name: "掉线店", connection_state: "offline", run_enabled: true, auto_delivery_enabled: false }
    ],
    open_issue_count: 2,
    active_job_count: 0,
    persistence: "healthy",
    stopping: false,
    restore: null,
    ...over
  };
}

describe("deriveStats 统计派生(T026)", () => {
  it("在线/总数与待处理计数", () => {
    const s = deriveStats(dash());
    expect(s.accountsOnline).toBe(1);
    expect(s.accountsTotal).toBe(2);
    expect(s.pending).toBe(2);
    expect(s.restoreActive).toBe(false);
    expect(s.stopping).toBe(false);
  });

  it("增量字段缺失 → null(降级为统计不可用,不显示 0 冒充)", () => {
    const s = deriveStats(dash());
    expect(s.ordersToday).toBeNull();
    expect(s.deliveredToday).toBeNull();
  });

  it("增量字段存在时取值;恢复横幅条件", () => {
    const s = deriveStats(
      dash({
        orders_today: 12,
        delivered_today: 11,
        restore: { state: "quarantined", quarantine_started_at: null, unresolved_count: 3, restore_epoch: 1 }
      })
    );
    expect(s.ordersToday).toBe(12);
    expect(s.deliveredToday).toBe(11);
    expect(s.restoreActive).toBe(true);
    expect(s.restoreUnresolved).toBe(3);
  });
});

describe("账号速览排序(US2-3)", () => {
  const acc = (name: string, state: string): QuickAccount => ({
    id: name,
    displayName: name,
    connectionState: state,
    runEnabled: true,
    autoDelivery: false,
    monitoringSince: null
  });

  it("异常(授权失效/需验证)最前,连接中次之,在线/暂停靠后", () => {
    const sorted = sortQuickAccounts([
      acc("在线店", "online"),
      acc("失效店", "authorization_expired"),
      acc("暂停店", "paused"),
      acc("连接店", "connecting"),
      acc("需验店", "verification_required")
    ]);
    expect(sorted.map((a) => a.displayName)).toEqual(["失效店", "需验店", "连接店", "在线店", "暂停店"]);
  });

  it("优先级数值随类别单调", () => {
    expect(accountSortPriority("authorization_expired")).toBeLessThan(accountSortPriority("connecting"));
    expect(accountSortPriority("connecting")).toBeLessThan(accountSortPriority("online"));
    expect(accountSortPriority("paused")).toBeGreaterThanOrEqual(accountSortPriority("online"));
  });
});

describe("resolvePresetRange 自然日对齐(005/T009,澄清 Q2)", () => {
  const ymd = (ms: number): [number, number, number] => {
    const d = new Date(ms);
    return [d.getFullYear(), d.getMonth(), d.getDate()];
  };

  it("跨年边界:1 月 1 日的昨天 = 去年 12 月 31 日全天", () => {
    const now = new Date(2026, 0, 1, 13, 45).getTime();
    const r = resolvePresetRange("yesterday", now);
    expect(ymd(r.from)).toEqual([2025, 11, 31]);
    expect(new Date(r.from).getHours()).toBe(0);
    expect(ymd(r.to)).toEqual([2026, 0, 1]);
    expect(new Date(r.to).getHours()).toBe(0);
  });

  it("跨月边界:1 月 31 日的 7天内 = 1 月 25 日起 7 个自然日", () => {
    const now = new Date(2026, 0, 31, 22, 0).getTime();
    const r = resolvePresetRange("7d", now);
    expect(ymd(r.from)).toEqual([2026, 0, 25]);
    expect(ymd(r.to)).toEqual([2026, 1, 1]);
    expect((r.to - r.from) / 86_400_000).toBe(7);
  });

  it("今天 = 当日 00:00 起至次日 00:00", () => {
    const now = new Date(2026, 8, 25, 9, 30).getTime();
    const r = resolvePresetRange("today", now);
    expect(ymd(r.from)).toEqual([2026, 8, 25]);
    expect(ymd(r.to)).toEqual([2026, 8, 26]);
    expect(r.to - r.from).toBe(86_400_000);
  });

  it("三天内/一个月内为含今天的滚动自然日窗口", () => {
    const now = new Date(2026, 2, 15, 18, 0).getTime();
    const r3 = resolvePresetRange("3d", now);
    expect(ymd(r3.from)).toEqual([2026, 2, 13]);
    expect(ymd(r3.to)).toEqual([2026, 2, 16]);
    const r30 = resolvePresetRange("30d", now);
    expect(ymd(r30.from)).toEqual([2026, 1, 14]);
    expect((r30.to - r30.from) / 86_400_000).toBe(30);
  });

  it("localDayStart 在任意时刻都落到当日零点", () => {
    expect(new Date(localDayStart(new Date(2026, 4, 9, 23, 59, 59).getTime())).getHours()).toBe(0);
    expect(new Date(localDayStart(new Date(2026, 4, 9, 0, 0, 1).getTime())).getDate()).toBe(9);
  });
});

describe("四卡视图模型与格式化(005/T009)", () => {
  const overview = (over: Partial<StatsOverview> = {}): StatsOverview => ({
    range: { from: 0, to: 7 * 86_400_000, granularity: "daily" },
    revenue: {
      minor_units: 4350,
      currency: "CNY",
      order_count: 4,
      previous_minor_units: 600,
      change_percent: 625
    },
    accounts: { online: 1, total: 2 },
    pending_issues: 4,
    stopping: false,
    restore: null,
    trend: [],
    ...over
  });

  it("金额格式化与徽标方向", () => {
    const c = deriveCards(overview());
    expect(c.revenueText).toBe("¥43.50");
    expect(c.changeText).toBe("+625%");
    expect(c.changeTrend).toBe("up");
    expect(c.accountsTotal).toBe(2);
    expect(c.pending).toBe(4);
  });

  it("负增长与零增长", () => {
    expect(deriveCards(overview({ revenue: { minor_units: 0, currency: "CNY", order_count: 0, previous_minor_units: 100, change_percent: -100 } as StatsOverview["revenue"] })).changeText).toBe("-100%");
    const flat = deriveCards(overview({ revenue: { minor_units: 500, currency: "CNY", order_count: 1, previous_minor_units: 500, change_percent: 0 } as StatsOverview["revenue"] }));
    expect(flat.changeText).toBe("0%");
    expect(flat.changeTrend).toBe("flat");
  });

  it("change_percent 为 null → 徽标隐藏,不显示 0%(FR-003)", () => {
    const c = deriveCards(overview({ revenue: { minor_units: 0, currency: "CNY", order_count: 0, previous_minor_units: 0, change_percent: null } as StatsOverview["revenue"] }));
    expect(c.changePercent).toBeNull();
    expect(c.changeText).toBe("");
    expect(c.changeTrend).toBe("flat");
  });

  it("formatYuan 保留两位小数", () => {
    expect(formatYuan(0)).toBe("¥0.00");
    expect(formatYuan(1)).toBe("¥0.01");
    expect(formatYuan(12345)).toBe("¥123.45");
  });
});

describe("预设选项完整性(默认 7天内)", () => {
  it("五个预设含今天/昨天/三天/7天/一个月", () => {
    const keys: RangePreset[] = ["today", "yesterday", "3d", "7d", "30d"];
    expect(PRESET_OPTIONS.map((p) => p.key)).toEqual(keys);
  });
});

describe("resolveCustomRange 自定义区间(005/T016,FR-009)", () => {
  const ymd2 = (ms: number): [number, number, number] => {
    const d = new Date(ms);
    return [d.getFullYear(), d.getMonth(), d.getDate()];
  };

  it("同一天 = 该自然日 00:00 至次日 00:00", () => {
    const r = resolveCustomRange("2026-05-09", "2026-05-09");
    expect(r).not.toBeNull();
    expect(ymd2(r!.from)).toEqual([2026, 4, 9]);
    expect(ymd2(r!.to)).toEqual([2026, 4, 10]);
    expect(r!.to - r!.from).toBe(86_400_000);
  });

  it("跨两周正常;跨月/跨年边界正确", () => {
    const r = resolveCustomRange("2025-12-28", "2026-01-03");
    expect(ymd2(r!.from)).toEqual([2025, 11, 28]);
    expect(ymd2(r!.to)).toEqual([2026, 0, 4]);
  });

  it("恰 92 个自然日通过;93 天拒绝", () => {
    // 2026-01-01 起含首尾共 92 天 → 结束日为 2026-04-02
    expect(resolveCustomRange("2026-01-01", "2026-04-02")).not.toBeNull();
    expect(resolveCustomRange("2026-01-01", "2026-04-03")).toBeNull();
  });

  it("结束早于起始拒绝;非法日期串拒绝", () => {
    expect(resolveCustomRange("2026-04-02", "2026-04-01")).toBeNull();
    expect(resolveCustomRange("2026-13-01", "2026-13-02")).toBeNull();
    expect(resolveCustomRange("abc", "2026-04-01")).toBeNull();
    expect(resolveCustomRange("", "")).toBeNull();
  });
});
