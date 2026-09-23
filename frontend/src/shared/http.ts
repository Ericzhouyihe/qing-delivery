import { session } from "../app/session";
import type { ApiErrorBody } from "./contracts";

export class ApiHttpError extends Error {
  readonly status: number;
  readonly body: ApiErrorBody | null;

  constructor(status: number, body: ApiErrorBody | null) {
    super(body?.message ?? `HTTP ${status}`);
    this.status = status;
    this.body = body;
  }
}

export interface RequestOptions {
  method?: string;
  body?: unknown;
  signal?: AbortSignal;
}

/** 统一 HTTP 入口：变更请求带 CSRF；读取可取消；错误信封映射为 ApiHttpError。
 *  纯读取超时/页面切换取消 fetch 不等于取消服务端任务（契约 §1）。
 */
export async function httpRequest<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const method = opts.method ?? "GET";
  const headers: Record<string, string> = { Accept: "application/json" };
  if (method !== "GET" && method !== "HEAD") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = session.csrfToken;
    headers["X-Idempotency-Key"] = crypto.randomUUID();
  }
  const res = await fetch(path, {
    method,
    headers,
    body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
    signal: opts.signal,
    credentials: "same-origin"
  });
  // 会话过期(操作中途 401/403):广播给路由回落登录并记忆目标页(US8-2)。
  // 登录表单自身的 401 不在此列(session.authenticated 为 false)。
  if ((res.status === 401 || res.status === 403) && session.authenticated) {
    document.dispatchEvent(new CustomEvent("qing:session-expired"));
  }
  if (res.status === 204) {
    return undefined as T;
  }
  const text = await res.text();
  const payload: unknown = text.length > 0 ? (JSON.parse(text) as unknown) : null;
  if (!res.ok) {
    const body = payload as Record<string, unknown> | null;
    const errBody = body?.["error"] as ApiErrorBody | undefined;
    throw new ApiHttpError(res.status, errBody ?? null);
  }
  return payload as T;
}

export function httpGet<T>(path: string, signal?: AbortSignal): Promise<T> {
  return httpRequest<T>(path, { method: "GET", signal });
}

export function httpPost<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  return httpRequest<T>(path, { method: "POST", body, signal });
}

export function httpPut<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
  return httpRequest<T>(path, { method: "PUT", body, signal });
}
