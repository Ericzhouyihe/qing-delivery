/** 状态语义色映射(T018,FR-004/FR-021/研究 D6):全局唯一 tone 表。
 *  枚举值与 001 线上契约一致(transport 的 DTO 映射);未收录值一律
 *  渲染为中性"未知状态"并保留原始值悬浮,不得崩溃或空白(SC-205)。 */
import { el } from "../shared/dom";

export type Tone = "normal" | "warning" | "danger" | "neutral" | "info";

export interface BadgeSpec {
  tone: Tone;
  label: string;
}

const UNKNOWN: BadgeSpec = { tone: "neutral", label: "未知状态" };

function spec(map: Record<string, BadgeSpec>, value: string): BadgeSpec {
  return map[value] ?? UNKNOWN;
}

/** 账号连接状态(accounts/dashboard DTO)。 */
const ACCOUNT: Record<string, BadgeSpec> = {
  connecting: { tone: "info", label: "连接中" },
  online: { tone: "normal", label: "在线" },
  offline: { tone: "neutral", label: "离线" },
  authorization_expired: { tone: "warning", label: "授权失效" },
  verification_required: { tone: "warning", label: "需验证" },
  paused: { tone: "neutral", label: "已暂停" }
};
export function accountBadge(state: string): BadgeSpec {
  return spec(ACCOUNT, state);
}

/** 内容交付轴(展示态:map_delivery_state 的输出)。 */
const DELIVERY: Record<string, BadgeSpec> = {
  awaiting_verification: { tone: "info", label: "待核验" },
  ready: { tone: "info", label: "待发送" },
  sending: { tone: "info", label: "发送中" },
  delivered: { tone: "normal", label: "内容已交付" },
  definitely_not_sent: { tone: "warning", label: "确定未发送" },
  unknown: { tone: "danger", label: "结果未知" },
  needs_review: { tone: "warning", label: "等待人工" },
  terminated: { tone: "neutral", label: "已终止" }
};
export function deliveryBadge(state: string): BadgeSpec {
  return spec(DELIVERY, state);
}

/** 审核轴(review_state)。 */
const REVIEW: Record<string, BadgeSpec> = {
  none: { tone: "neutral", label: "无审核" },
  required: { tone: "warning", label: "待人工审核" },
  resolved: { tone: "normal", label: "审核已处理" }
};
export function reviewBadge(state: string): BadgeSpec {
  return spec(REVIEW, state);
}

/** 平台确认轴(展示态:map_confirmation 的输出)。 */
const CONFIRMATION: Record<string, BadgeSpec> = {
  disabled: { tone: "neutral", label: "确认未开启" },
  pending: { tone: "info", label: "确认待执行" },
  dispatching: { tone: "info", label: "确认中" },
  succeeded: { tone: "normal", label: "平台已确认" },
  failed: { tone: "danger", label: "确认失败" },
  unknown: { tone: "danger", label: "确认未知" },
  terminated: { tone: "neutral", label: "确认已终止" }
};
export function confirmationBadge(state: string): BadgeSpec {
  return spec(CONFIRMATION, state);
}

/** 扫码会话状态。 */
const QR: Record<string, BadgeSpec> = {
  awaiting_scan: { tone: "info", label: "等待扫码" },
  awaiting_authorization: { tone: "info", label: "已扫码,等待授权" },
  authorized: { tone: "normal", label: "已授权" },
  verification_required: { tone: "warning", label: "需要平台验证" },
  expired: { tone: "neutral", label: "已超时" },
  cancelled: { tone: "neutral", label: "已取消" },
  failed: { tone: "danger", label: "失败" }
};
export function qrBadge(state: string): BadgeSpec {
  return spec(QR, state);
}

/** 商品状态与规则配置状态。 */
const ITEM_STATUS: Record<string, BadgeSpec> = {
  on_sale: { tone: "normal", label: "在售" },
  off_sale: { tone: "neutral", label: "已下架" },
  unknown: { tone: "neutral", label: "未知" }
};
export function itemStatusBadge(state: string): BadgeSpec {
  return spec(ITEM_STATUS, state);
}

const RULE_STATE: Record<string, BadgeSpec> = {
  configured: { tone: "normal", label: "已配置" },
  missing: { tone: "neutral", label: "未配置" },
  disabled: { tone: "neutral", label: "已禁用" },
  conflict: { tone: "danger", label: "冲突" }
};
export function ruleStateBadge(state: string): BadgeSpec {
  return spec(RULE_STATE, state);
}

/** 待处理事项状态。 */
const ISSUE: Record<string, BadgeSpec> = {
  open: { tone: "warning", label: "待处理" },
  resolved: { tone: "normal", label: "已解决" },
  terminated: { tone: "neutral", label: "已终止" }
};
export function issueStateBadge(state: string): BadgeSpec {
  return spec(ISSUE, state);
}

/** 渲染徽章元素;未知值保留原始值悬浮(FR-021)。 */
export function badgeEl(b: BadgeSpec, rawValue?: string): HTMLElement {
  const node = el("span", { class: `badge badge-${b.tone}` }, b.label);
  if (b === UNKNOWN && rawValue !== undefined) {
    node.title = `原始状态:${rawValue}`;
  }
  return node;
}

/** 双轴状态标签(US5/FR-016):内容状态为主、审核/确认为副。 */
export function statusPairEl(
  delivery: string | null,
  secondary: string | null,
  secondaryKind: "review" | "confirmation" = "confirmation"
): HTMLElement {
  const box = el("span", { class: "badge-pair" });
  box.append(badgeEl(deliveryBadge(delivery ?? "terminated"), delivery ?? undefined));
  if (secondary !== null && secondary !== "none" && secondary !== "disabled") {
    const b = secondaryKind === "review" ? reviewBadge(secondary) : confirmationBadge(secondary);
    box.append(badgeEl(b, secondary));
  }
  return box;
}
