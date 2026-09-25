/** 账号页展示层模型(006,数据模型 §2):过滤/计数/解析等纯函数,"算"与"摆"分离。
 *  页面装配(page.ts)只调用本模块与既有 API,业务规则仍在后端(宪章 II)。 */
import { isRecord } from "../../shared/contracts";

export interface AccountCard {
  id: string;
  platformUserId: string;
  avatarUrl: string | null;
  remark: string | null;
  displayName: string;
  connectionState: string;
  runEnabled: boolean;
  autoDelivery: boolean;
  autoConfirm: boolean;
  controlVersion: number;
  monitoringSince: string | null;
}

export function parseAccountCard(v: unknown): AccountCard {
  if (!isRecord(v)) throw new Error("账号格式错误");
  const c = (v["control"] ?? {}) as Record<string, unknown>;
  return {
    id: String(v["id"] ?? ""),
    platformUserId: String(v["platform_user_id"] ?? ""),
    avatarUrl: typeof v["avatar_url"] === "string" ? v["avatar_url"] : null,
    remark: typeof v["remark"] === "string" ? v["remark"] : null,
    displayName: String(v["display_name"] ?? ""),
    connectionState: String(v["connection_state"] ?? "paused"),
    runEnabled: c["run_enabled"] === true,
    autoDelivery: c["auto_delivery_enabled"] === true,
    autoConfirm: c["auto_confirm_enabled"] === true,
    controlVersion: typeof v["control_version"] === "number" ? v["control_version"] : 1,
    monitoringSince: typeof v["monitoring_since"] === "string" ? v["monitoring_since"] : null
  };
}

/** 搜索过滤(US1/FR-003):昵称/备注/账号 ID 不区分大小写子串匹配;空 query 返回全部。 */
export function filterAccounts(items: readonly AccountCard[], query: string): AccountCard[] {
  const q = query.trim().toLowerCase();
  if (q.length === 0) return [...items];
  return items.filter((a) =>
    [a.displayName, a.remark ?? "", a.platformUserId].some((field) => field.toLowerCase().includes(q))
  );
}

/** 横幅计数(US1/FR-004):X=过滤后可见数,Y=账号总数。 */
export function viewCounts(shown: number, total: number): { shown: number; total: number } {
  return { shown, total };
}

/** 异常优先级(003 US3 语义迁移):需验证/授权失效 > 连接中 > 在线 > 其余。 */
function priority(state: string): number {
  switch (state) {
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

/** 异常置顶排序(US2/FR-006):同级按昵称稳定排序;返回新数组,不改输入。 */
export function sortAccounts(items: readonly AccountCard[]): AccountCard[] {
  return [...items].sort((a, b) => {
    const p = priority(a.connectionState) - priority(b.connectionState);
    return p !== 0 ? p : a.displayName.localeCompare(b.displayName);
  });
}

/** 能力/状态徽标(US2/FR-008):只映射系统真实具备的能力,不虚标参考图中的
 *  未实现功能(AI、自动评价、每日擦亮);连接状态徽章另由 accountBadge 呈现。 */
export interface CapabilityTag {
  id: "needs-verify" | "auto-delivery" | "auto-confirm";
  label: string;
  tone: "normal" | "warning" | "info" | "neutral";
}

export function deriveCapabilityTags(a: AccountCard): CapabilityTag[] {
  const tags: CapabilityTag[] = [];
  if (a.connectionState === "verification_required" || a.connectionState === "authorization_expired") {
    tags.push({ id: "needs-verify", label: "需要验证", tone: "warning" });
  }
  if (a.autoDelivery) tags.push({ id: "auto-delivery", label: "自动发货", tone: "normal" });
  if (a.autoConfirm) tags.push({ id: "auto-confirm", label: "自动平台确认", tone: "normal" });
  return tags;
}

/** 视图偏好(US3/FR-010,契约 §3):沿用 004 的 localStorage 键,未知旧值回退 row。 */
export type AccountViewMode = "row" | "card";

const VIEW_KEY = "qing-account-view";

export function readViewPreference(): AccountViewMode {
  try {
    return localStorage.getItem(VIEW_KEY) === "card" ? "card" : "row";
  } catch {
    return "row"; // 隐私模式等 localStorage 不可用场景
  }
}

export function writeViewPreference(v: AccountViewMode): void {
  try {
    localStorage.setItem(VIEW_KEY, v);
  } catch {
    /* 写入失败不阻断交互,仅本次会话内生效 */
  }
}
