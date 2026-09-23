/** 订单页视图模型(US5/T039):筛选状态、游标分页、人工动作显示可用性。 */
import { isRecord } from "../../shared/contracts";

export interface OrderRow {
  id: string;
  accountId: string;
  platformOrderId: string;
  itemId: string | null;
  paidAt: string | null;
  quantity: number | null;
  amountMinor: number | null;
  currency: string | null;
  tradeType: string;
  platformState: string;
  deliveryState: string | null;
  confirmationState: string | null;
}

export function parseOrderRow(v: unknown): OrderRow {
  if (!isRecord(v)) throw new Error("订单格式错误");
  // 列表契约为嵌套金额对象:{ amount: { minor_units, currency } | null }
  const amount = v["amount"];
  const amt = isRecord(amount) ? amount : null;
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    platformOrderId: String(v["platform_order_id"] ?? ""),
    itemId: typeof v["item_id"] === "string" ? v["item_id"] : null,
    paidAt: typeof v["paid_at"] === "string" ? v["paid_at"] : null,
    quantity: typeof v["quantity"] === "number" ? v["quantity"] : null,
    amountMinor: typeof amt?.["minor_units"] === "number" ? (amt["minor_units"] as number) : null,
    currency: typeof amt?.["currency"] === "string" ? (amt["currency"] as string) : null,
    tradeType: String(v["trade_type"] ?? "unknown"),
    platformState: String(v["platform_state"] ?? "unknown"),
    deliveryState: typeof v["delivery_state"] === "string" ? v["delivery_state"] : null,
    confirmationState: typeof v["confirmation_state"] === "string" ? v["confirmation_state"] : null
  };
}

export interface OrderFilters {
  orderId: string;
  accountId: string;
  contentState: string;
  platformState: string;
  from: string;
  to: string;
}

export const EMPTY_FILTERS: OrderFilters = {
  orderId: "",
  accountId: "",
  contentState: "",
  platformState: "",
  from: "",
  to: ""
};

export function filtersToQuery(f: OrderFilters, limit = 50): string {
  const parts = [`limit=${limit}`];
  if (f.orderId.trim() !== "") parts.push(`platform_order_id=${encodeURIComponent(f.orderId.trim())}`);
  if (f.accountId !== "") parts.push(`account_id=${encodeURIComponent(f.accountId)}`);
  if (f.platformState !== "") parts.push(`platform_state=${encodeURIComponent(f.platformState)}`);
  if (f.from !== "") parts.push(`from=${encodeURIComponent(f.from)}`);
  if (f.to !== "") parts.push(`to=${encodeURIComponent(f.to)}`);
  return `/api/v1/orders?${parts.join("&")}`;
  // 说明:内容状态(内容轴)不在列表查询参数中(001 契约);
  // 界面按内容状态过滤为客户端二次过滤,详情仍以服务端为准。
}

export function isContentStateFilterable(f: OrderFilters): boolean {
  return f.contentState !== "";
}

/** 客户端内容轴二次过滤(列表页 UI 便利;判定不改变服务端数据)。 */
export function applyContentStateFilter(rows: OrderRow[], contentState: string): OrderRow[] {
  if (contentState === "") return rows;
  return rows.filter((r) => r.deliveryState === contentState);
}

export type ManualAction = "resend" | "mark_received" | "terminate" | "takeover";

/** 人工动作提交体(T064/FR-022):补发携带重复风险确认,
 *  接管携带历史范围确认(服务端 manual_api 强制校验,缺失即拒);
 *  其余动作仅原因。确认流中的勾选按动作语义映射到对应旗标。 */
export function manualActionBody(
  action: ManualAction,
  reason: string,
  ackChecked: boolean
): Record<string, unknown> {
  const body: Record<string, unknown> = { reason };
  if (action === "resend") {
    body["acknowledge_duplicate_risk"] = ackChecked;
  }
  if (action === "takeover") {
    body["acknowledge_historical_scope"] = ackChecked;
  }
  return body;
}

export interface ActionAvailability {
  enabled: boolean;
  reason: string;
}

/** 人工动作显示可用性(FR-017):界面仅按展示状态做粗粒度启停并注明原因;
 *  服务端 guard 仍是权威,提交错误在确认流中如实呈现。 */
export function actionAvailability(action: ManualAction, deliveryState: string | null, confirmationState: string | null): ActionAvailability {
  const d = deliveryState ?? "无交付记录";
  switch (action) {
    case "resend":
      return ["needs_review", "definitely_not_sent", "unknown"].includes(deliveryState ?? "")
        ? { enabled: true, reason: "" }
        : { enabled: false, reason: `当前内容状态「${d}」不在可补发范围(仅等待人工/确定未发送/结果未知)` };
    case "mark_received":
      return ["failed", "unknown"].includes(confirmationState ?? "")
        ? { enabled: true, reason: "" }
        : { enabled: false, reason: `平台确认「${confirmationState ?? "无"}」无需人工确认收到` };
    case "terminate":
      return deliveryState === "terminated"
        ? { enabled: false, reason: "交付已终止" }
        : { enabled: true, reason: "" };
    case "takeover":
      return deliveryState === "terminated"
        ? { enabled: false, reason: "交付已终止" }
        : { enabled: true, reason: "" };
  }
}
