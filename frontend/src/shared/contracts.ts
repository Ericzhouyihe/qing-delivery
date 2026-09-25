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

/** 007 新增错误码 → 面向用户的提示文案;未收录的码走"未知错误安全降级"。 */
export const ERROR_HINTS: Record<string, string> = {
  stock_insufficient: "卡密库存不足,已转待人工处理,请补充库存",
  referenced_resource: "该资源正被规则或模板引用,请先解除引用",
  payload_too_large: "上传内容超出大小限制",
  reply_limit_reached: "快捷回复已达上限(50 条)",
  credential_change_failed: "凭据修改失败:当前密码不正确或新值不合法",
};

export function errorHint(code: string | undefined, fallback: string): string {
  if (code && ERROR_HINTS[code]) return ERROR_HINTS[code];
  return fallback;
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
  return typeof v === "object" && v !== null && !Array.isArray(v);
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

/** 运营统计概览(005,contracts/stats-api.md)。
 *  change_percent 为 null 表示前区间无数据(界面隐藏徽标,不显示 0%);
 *  trend 非数组时降级为空数组,由界面显示"统计不可用"占位。 */
export interface StatsOverview {
  range: { from: number; to: number; granularity: "hourly" | "daily" };
  revenue: {
    minor_units: number;
    currency: string;
    order_count: number;
    previous_minor_units: number;
    change_percent: number | null;
  };
  accounts: { online: number; total: number };
  pending_issues: number;
  /** 007 T014/T018:库存卡密余量(全部启用批量组可用余量之和)。
   *  可选字段:旧版本服务缺失时为 undefined,界面显示"—"不闪 0(契约降级)。 */
  stock?: { available_total: number };
  stopping: boolean;
  restore: {
    state: string;
    quarantine_started_at: string | null;
    unresolved_count: number;
    restore_epoch: number;
  } | null;
  trend: Array<{ bucket_start: number; minor_units: number; order_count: number }>;
}

export function parseStatsOverview(v: unknown): StatsOverview {
  if (!isRecord(v)) throw new Error("统计响应格式错误");
  const rangeRaw = isRecord(v["range"]) ? v["range"] : {};
  const revenueRaw = isRecord(v["revenue"]) ? v["revenue"] : {};
  const accountsRaw = isRecord(v["accounts"]) ? v["accounts"] : {};
  const trendRaw = Array.isArray(v["trend"]) ? v["trend"] : [];
  const restoreRaw = v["restore"];
  // 007:stock 缺失/形状不对 → 字段不设值(undefined),界面降级不闪 0
  const stockRaw = isRecord(v["stock"]) ? v["stock"] : null;
  const out: StatsOverview = {
    range: {
      from: typeof rangeRaw["from"] === "number" ? rangeRaw["from"] : 0,
      to: typeof rangeRaw["to"] === "number" ? rangeRaw["to"] : 0,
      granularity: rangeRaw["granularity"] === "daily" ? "daily" : "hourly"
    },
    revenue: {
      minor_units: typeof revenueRaw["minor_units"] === "number" ? revenueRaw["minor_units"] : 0,
      currency: String(revenueRaw["currency"] ?? "CNY"),
      order_count: typeof revenueRaw["order_count"] === "number" ? revenueRaw["order_count"] : 0,
      previous_minor_units:
        typeof revenueRaw["previous_minor_units"] === "number"
          ? revenueRaw["previous_minor_units"]
          : 0,
      change_percent:
        typeof revenueRaw["change_percent"] === "number" ? revenueRaw["change_percent"] : null
    },
    accounts: {
      online: typeof accountsRaw["online"] === "number" ? accountsRaw["online"] : 0,
      total: typeof accountsRaw["total"] === "number" ? accountsRaw["total"] : 0
    },
    pending_issues: typeof v["pending_issues"] === "number" ? v["pending_issues"] : 0,
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
          restore_epoch:
            typeof restoreRaw["restore_epoch"] === "number" ? restoreRaw["restore_epoch"] : 0
        }
      : null,
    trend: trendRaw.map((t): StatsOverview["trend"][number] => {
      const r = isRecord(t) ? t : {};
      return {
        bucket_start: typeof r["bucket_start"] === "number" ? r["bucket_start"] : 0,
        minor_units: typeof r["minor_units"] === "number" ? r["minor_units"] : 0,
        order_count: typeof r["order_count"] === "number" ? r["order_count"] : 0
      };
    })
  };
  if (stockRaw !== null && typeof stockRaw["available_total"] === "number") {
    out.stock = { available_total: stockRaw["available_total"] };
  }
  return out;
}

// ===== 卡密库存(007,contracts §1;T015)=====

export type CardPoolKind = "data" | "text" | "image" | "api";

/** data 组分状态库存计数(transport summary_json 的 stock 键)。 */
export interface StockSummary {
  available: number;
  reserved: number;
  used: number;
}

/** api 组脱敏摘要:headers/params/body 只回"是否已配置",值永不回显。 */
export interface CardPoolApiConfigSummary {
  url: string;
  method: string;
  timeout_ms: number;
  content_type: string | null;
  response_path: string;
  retry_enabled: boolean;
  headers_configured: boolean;
  params_configured: boolean;
  body_configured: boolean;
}

export interface CardPoolDto {
  id: string;
  name: string;
  kind: CardPoolKind;
  enabled: boolean;
  delay_seconds: number;
  description: string;
  version: number;
  created_at: string;
  updated_at: string;
  /** 仅 data 组返回;缺失(undefined)时界面按"计数不可用"降级。 */
  stock?: StockSummary;
  /** 仅 text/image 组返回:固定内容是否已配置(明文永不回显)。 */
  content_set?: boolean;
  /** 仅 api 组返回。 */
  api_config?: CardPoolApiConfigSummary;
}

/** 只校验消费字段,缺失降级(与账号/统计 parse 同模式);非对象抛错。 */
export function parseCardPool(v: unknown): CardPoolDto {
  if (!isRecord(v)) throw new Error("卡密组格式错误");
  const kindRaw = String(v["kind"] ?? "");
  const kind: CardPoolKind =
    kindRaw === "data" || kindRaw === "text" || kindRaw === "image" || kindRaw === "api"
      ? kindRaw
      : "data";
  const stockRaw = isRecord(v["stock"]) ? v["stock"] : null;
  const apiRaw = isRecord(v["api_config"]) ? v["api_config"] : null;
  const out: CardPoolDto = {
    id: String(v["id"] ?? ""),
    name: String(v["name"] ?? ""),
    kind,
    enabled: v["enabled"] === true,
    delay_seconds: typeof v["delay_seconds"] === "number" ? v["delay_seconds"] : 0,
    description: String(v["description"] ?? ""),
    version: typeof v["version"] === "number" ? v["version"] : 1,
    created_at: String(v["created_at"] ?? ""),
    updated_at: String(v["updated_at"] ?? "")
  };
  if (stockRaw !== null) {
    out.stock = {
      available: typeof stockRaw["available"] === "number" ? stockRaw["available"] : 0,
      reserved: typeof stockRaw["reserved"] === "number" ? stockRaw["reserved"] : 0,
      used: typeof stockRaw["used"] === "number" ? stockRaw["used"] : 0
    };
  }
  if (typeof v["content_set"] === "boolean") out.content_set = v["content_set"];
  if (apiRaw !== null) {
    out.api_config = {
      url: String(apiRaw["url"] ?? ""),
      method: String(apiRaw["method"] ?? "GET"),
      timeout_ms: typeof apiRaw["timeout_ms"] === "number" ? apiRaw["timeout_ms"] : 10_000,
      content_type: typeof apiRaw["content_type"] === "string" ? apiRaw["content_type"] : null,
      response_path: String(apiRaw["response_path"] ?? ""),
      retry_enabled: apiRaw["retry_enabled"] === true,
      headers_configured: apiRaw["headers_configured"] === true,
      params_configured: apiRaw["params_configured"] === true,
      body_configured: apiRaw["body_configured"] === true
    };
  }
  return out;
}
