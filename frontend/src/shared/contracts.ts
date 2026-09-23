/** 与 Rust transport DTO 对齐的契约类型 + 最小运行时校验。
 *  宪章 IV：前后端契约必须有明确类型与运行时输入校验。
 */

export interface SessionResponse {
  initialized: boolean;
  authenticated: boolean;
  csrf_token: string;
  administrator?: AdministratorSummary;
}

export interface AdministratorSummary {
  id: string;
  display_name: string;
}

export interface ApiErrorBody {
  code: string;
  message: string;
  request_id: string;
  retryable: boolean;
  field_errors?: FieldError[];
  current_version?: number;
  job_id?: string;
}

export interface FieldError {
  field: string;
  code: string;
  message: string;
}

export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

export interface JobSummary {
  id: string;
  kind: string;
  state: JobState;
  version: number;
  created_at: string;
  updated_at: string;
  target_id?: string;
}

export type JobState =
  | "queued"
  | "running"
  | "cancel_requested"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "needs_review";

export interface AcceptedOperation {
  operation_id: string;
  job: JobSummary;
}

/** 运行时校验：只断言本页实际消费的字段，不做全量复制。 */
export function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

export function requireString(v: unknown, field: string): string {
  if (typeof v !== "string" || v.length === 0) {
    throw new Error(`字段 ${field} 应为非空字符串`);
  }
  return v;
}

export function parseSessionResponse(v: unknown): SessionResponse {
  if (!isRecord(v)) throw new Error("会话响应格式错误");
  if (typeof v["initialized"] !== "boolean" || typeof v["authenticated"] !== "boolean") {
    throw new Error("会话响应缺少布尔状态");
  }
  const csrf = requireString(v["csrf_token"], "csrf_token");
  const out: SessionResponse = {
    initialized: v["initialized"],
    authenticated: v["authenticated"],
    csrf_token: csrf
  };
  const admin = v["administrator"];
  if (isRecord(admin)) {
    out.administrator = {
      id: requireString(admin["id"], "administrator.id"),
      display_name: requireString(admin["display_name"], "administrator.display_name")
    };
  }
  return out;
}

export function parsePage<T>(v: unknown, parseItem: (raw: unknown) => T): Page<T> {
  if (!isRecord(v) || !Array.isArray(v["items"])) throw new Error("分页响应格式错误");
  const next = v["next_cursor"];
  return {
    items: v["items"].map((it) => parseItem(it)),
    next_cursor: typeof next === "string" ? next : null
  };
}

/** dashboard 摘要(T075 既有 + 002 增量字段,contracts/dashboard-api.md)。
 *  orders_today/delivered_today 为 002 新增:旧版本服务缺失时界面降级为"统计不可用",
 *  不得显示 0 冒充真实计数(契约不变量)。 */
export interface DashboardSummary {
  accounts: Array<{
    id: string;
    display_name: string;
    connection_state: string;
    run_enabled: boolean;
    auto_delivery_enabled: boolean;
  }>;
  open_issue_count: number;
  active_job_count: number;
  persistence: string;
  stopping: boolean;
  restore: {
    state: string;
    quarantine_started_at: string | null;
    unresolved_count: number;
    restore_epoch: number;
  } | null;
  orders_today?: number;
  delivered_today?: number;
}

export function parseDashboardSummary(v: unknown): DashboardSummary {
  if (!isRecord(v)) throw new Error("摘要响应格式错误");
  const accountsRaw = Array.isArray(v["accounts"]) ? v["accounts"] : [];
  const restoreRaw = v["restore"];
  const out: DashboardSummary = {
    accounts: accountsRaw.map((a): DashboardSummary["accounts"][number] => {
      const r = a as Record<string, unknown>;
      return {
        id: String(r["id"] ?? ""),
        display_name: String(r["display_name"] ?? ""),
        connection_state: String(r["connection_state"] ?? "paused"),
        run_enabled: r["run_enabled"] === true,
        auto_delivery_enabled: r["auto_delivery_enabled"] === true
      };
    }),
    open_issue_count: typeof v["open_issue_count"] === "number" ? v["open_issue_count"] : 0,
    active_job_count: typeof v["active_job_count"] === "number" ? v["active_job_count"] : 0,
    persistence: String(v["persistence"] ?? "unknown"),
    stopping: v["stopping"] === true,
    restore: isRecord(restoreRaw)
      ? {
          state: String(restoreRaw["state"] ?? "quarantined"),
          quarantine_started_at:
            typeof restoreRaw["quarantine_started_at"] === "string"
              ? restoreRaw["quarantine_started_at"]
              : null,
          unresolved_count:
            typeof restoreRaw["unresolved_count"] === "number" ? restoreRaw["unresolved_count"] : 0,
          restore_epoch: typeof restoreRaw["restore_epoch"] === "number" ? restoreRaw["restore_epoch"] : 0
        }
      : null
  };
  if (typeof v["orders_today"] === "number") out.orders_today = v["orders_today"];
  if (typeof v["delivered_today"] === "number") out.delivered_today = v["delivered_today"];
  return out;
}
