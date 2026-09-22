import { el, errorMessage } from "../shared/dom";
import { httpPost } from "../shared/http";
import type { SessionResponse } from "../shared/contracts";
import { applySession, session } from "./session";

/** 认证门控:未初始化 → 初始化表单;已初始化未登录 → 登录表单。
 *  登录/初始化必须携带匿名预会话 CSRF(bootstrapSession 已存入 session.csrfToken)。 */
export function renderAuth(root: HTMLElement, onAuthed: () => void): void {
  root.append(el("h1", {}, "轻交付"), el("p", { class: "muted" }, "本地闲鱼自动发货管理"));
  const box = el("div", { class: "auth" });
  root.append(box);
  const msg = el("p", { class: "error" }, "");

  const submit = async (path: string, body: unknown): Promise<void> => {
    msg.textContent = "";
    try {
      const res = await httpPost<SessionResponse>(path, body);
      applySession(res);
      onAuthed();
    } catch (e) {
      msg.textContent = errorMessage(e);
    }
  };

  const field = (placeholder: string): HTMLInputElement => {
    const input = el("input", { type: "password", placeholder });
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        (input.parentElement?.querySelector("button") as HTMLButtonElement | null)?.click();
      }
    });
    return input;
  };

  if (!session.initialized) {
    box.append(el("h2", {}, "初始化管理员"));
    const pw = field("密码(至少 12 字符)");
    const pw2 = field("再次输入密码");
    const btn = el("button", {}, "创建管理员");
    btn.addEventListener("click", () => {
      if (pw.value !== pw2.value) {
        msg.textContent = "两次输入的密码不一致";
        return;
      }
      if (pw.value.length < 12) {
        msg.textContent = "密码至少需要 12 个字符";
        return;
      }
      void submit("/api/v1/auth/initialize", {
        password: pw.value,
        password_confirmation: pw2.value,
      });
    });
    box.append(pw, pw2, btn, msg);
  } else {
    box.append(el("h2", {}, "管理员登录"));
    const pw = field("密码");
    const btn = el("button", {}, "登录");
    btn.addEventListener("click", () => {
      void submit("/api/v1/auth/login", { password: pw.value });
    });
    box.append(pw, btn, msg);
  }
}
