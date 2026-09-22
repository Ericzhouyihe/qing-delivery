import { httpGet } from "../shared/http";
import type { SessionResponse } from "../shared/contracts";

export interface SessionState {
  initialized: boolean;
  authenticated: boolean;
  csrfToken: string;
  displayName: string | null;
}

export const session: SessionState = {
  initialized: false,
  authenticated: false,
  csrfToken: "",
  displayName: null
};

/** 启动时建立匿名预会话并取得 CSRF token；登录态变化后须重新调用。 */
export async function bootstrapSession(): Promise<void> {
  const res = await httpGet<SessionResponse>("/api/v1/auth/session");
  session.initialized = res.initialized;
  session.authenticated = res.authenticated;
  session.csrfToken = res.csrf_token;
  session.displayName = res.administrator?.display_name ?? null;
}

export function applySession(res: SessionResponse): void {
  session.initialized = res.initialized;
  session.authenticated = res.authenticated;
  session.csrfToken = res.csrf_token;
  session.displayName = res.administrator?.display_name ?? null;
}
