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
