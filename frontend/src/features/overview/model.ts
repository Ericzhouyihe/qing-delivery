/** 概览视图模型(T022/T026):纯派生逻辑,与 DOM 解耦以便行为测试。 */
import type { DashboardSummary } from "../../shared/contracts";

export interface OverviewStats {
  accountsOnline: number;
  accountsTotal: number;
  /** null = 服务端未提供(旧版本),界面显示"统计不可用"而非 0(契约降级)。 */
  ordersToday: number | null;
  deliveredToday: number | null;
  pending: number;
  restoreActive: boolean;
  restoreUnresolved: number;
  stopping: boolean;
}

export function deriveStats(d: DashboardSummary): OverviewStats {
  return {
    accountsOnline: d.accounts.filter((a) => a.connection_state === "online").length,
    accountsTotal: d.accounts.length,
    ordersToday: d.orders_today ?? null,
    deliveredToday: d.delivered_today ?? null,
    pending: d.open_issue_count,
    restoreActive: d.restore !== null,
    restoreUnresolved: d.restore?.unresolved_count ?? 0,
    stopping: d.stopping
  };
}

/** 账号速览排序:异常(授权失效/需验证)最前,其次连接中,其余按名称稳定排序(US2-3)。 */
export function accountSortPriority(connectionState: string): number {
  switch (connectionState) {
    case "authorization_expired":
    case "verification_required":
      return 0;
    case "connecting":
      return 1;
    case "online":
      return 2;
    default:
      return 3;
  }
}

export interface QuickAccount {
  id: string;
  displayName: string;
  connectionState: string;
  runEnabled: boolean;
  autoDelivery: boolean;
  monitoringSince: string | null;
}

export function sortQuickAccounts(list: QuickAccount[]): QuickAccount[] {
  return [...list].sort((a, b) => {
    const p = accountSortPriority(a.connectionState) - accountSortPriority(b.connectionState);
    return p !== 0 ? p : a.displayName.localeCompare(b.displayName);
  });
}
