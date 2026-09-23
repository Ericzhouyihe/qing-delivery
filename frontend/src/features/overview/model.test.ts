import { describe, expect, it } from "vitest";
import { accountSortPriority, deriveStats, sortQuickAccounts, type QuickAccount } from "./model";
import type { DashboardSummary } from "../../shared/contracts";

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
