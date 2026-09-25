/** 概览视图模型(T022/T026):纯派生逻辑,与 DOM 解耦以便行为测试。 */
import type { DashboardSummary, StatsOverview } from "../../shared/contracts";

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

// ===== 运营统计(005,T009):预设范围与四卡视图模型 =====

const DAY_MS = 86_400_000;

/** 预设范围键;语义为自然日对齐(澄清 Q2):7天内=含今天共 7 个自然日。 */
export type RangePreset = "today" | "yesterday" | "3d" | "7d" | "30d";

export const PRESET_OPTIONS: Array<{ key: RangePreset; label: string }> = [
  { key: "today", label: "今天" },
  { key: "yesterday", label: "昨天" },
  { key: "3d", label: "三天内" },
  { key: "7d", label: "7天内" },
  { key: "30d", label: "一个月内" }
];

export const DEFAULT_PRESET: RangePreset = "7d";

/** 本地时区自然日 00:00(epoch ms);按日历回退 daysBack 天,跨月/跨年安全。 */
export function localDayStart(nowMs: number, daysBack = 0): number {
  const d = new Date(nowMs);
  if (daysBack > 0) d.setDate(d.getDate() - daysBack);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 预设 → [from, to) 半开区间(本地自然日对齐);服务端按同口径复核。 */
export function resolvePresetRange(preset: RangePreset, nowMs: number): { from: number; to: number } {
  const today = localDayStart(nowMs);
  switch (preset) {
    case "today":
      return { from: today, to: today + DAY_MS };
    case "yesterday":
      return { from: localDayStart(nowMs, 1), to: today };
    case "3d":
      return { from: localDayStart(nowMs, 2), to: today + DAY_MS };
    case "7d":
      return { from: localDayStart(nowMs, 6), to: today + DAY_MS };
    case "30d":
      return { from: localDayStart(nowMs, 29), to: today + DAY_MS };
  }
}

/** 自定义区间跨度上限:92 个自然日(与后端 FR-009 同规则)。 */
export const CUSTOM_MAX_DAYS = 92;

/** 解析 YYYY-MM-DD 为本地自然日 00:00;格式或日期非法(如 13 月)返回 null。 */
export function parseLocalDay(s: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  const dt = new Date(y, mo - 1, d);
  if (dt.getFullYear() !== y || dt.getMonth() !== mo - 1 || dt.getDate() !== d) return null;
  return dt.getTime();
}

/** 自定义区间(含两端自然日):合法返回 [from, to);倒置、非法或跨度超过 92 天返回 null。
 *  返回 null 时调用方提示且不得发起请求(FR-009)。 */
export function resolveCustomRange(startDay: string, endDay: string): { from: number; to: number } | null {
  const from = parseLocalDay(startDay);
  const toDay = parseLocalDay(endDay);
  if (from === null || toDay === null || toDay < from) return null;
  const dayCount = Math.round((toDay - from) / DAY_MS) + 1;
  if (dayCount > CUSTOM_MAX_DAYS) return null;
  return { from, to: toDay + DAY_MS };
}

/** 分单位金额格式化:minor units → ¥x.xx。 */
export function formatYuan(minorUnits: number): string {
  return `¥${(minorUnits / 100).toFixed(2)}`;
}

/** 五卡视图模型(派生自 StatsOverview,与 DOM 解耦以便行为测试)。
 *  007 T018:第 5 卡"库存卡密余量" = 全部启用批量组可用余量之和。 */
export interface OverviewCards {
  revenueText: string;
  /** null = 前区间无数据 → 徽标隐藏(FR-003),不得显示 0% 冒充。 */
  changePercent: number | null;
  changeText: string;
  changeTrend: "up" | "down" | "flat";
  accountsOnline: number;
  accountsTotal: number;
  orderCount: number;
  pending: number;
  /** null = 服务未提供 stock 字段(旧版本)→ 界面显示"—"不闪 0(契约降级)。 */
  stockAvailable: number | null;
}

export function deriveCards(s: StatsOverview): OverviewCards {
  const changePercent = s.revenue.change_percent;
  return {
    revenueText: formatYuan(s.revenue.minor_units),
    changePercent,
    changeText: changePercent === null ? "" : `${changePercent > 0 ? "+" : ""}${changePercent}%`,
    changeTrend:
      changePercent === null || changePercent === 0 ? "flat" : changePercent > 0 ? "up" : "down",
    accountsOnline: s.accounts.online,
    accountsTotal: s.accounts.total,
    orderCount: s.revenue.order_count,
    pending: s.pending_issues,
    stockAvailable: s.stock?.available_total ?? null
  };
}
