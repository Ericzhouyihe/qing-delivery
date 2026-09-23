/** 认证门控(US8/T053/T054,FR-020):品牌化初始化/登录卡片、行内错误、
 *  登录中按钮态;会话过期回落由 router 记忆目标页,重登成功返回原页面。
 *  登录/初始化携带匿名预会话 CSRF(bootstrapSession 已存入 session.csrfToken);
 *  401/403 时刷新预会话令牌重试一次(预会话过期兜底)。 */
import { el, errorMessage } from "../shared/dom";
import { ApiHttpError, httpPost } from "../shared/http";
import type { SessionResponse } from "../shared/contracts";
import { applySession, bootstrapSession, session } from "./session";
import { consumeReturnTo } from "./router";
import workspaceImage from "../assets/auth-workspace.png";

const MIN_PASSWORD = 12;

export interface AuthValidation {
  ok: boolean;
  /** 行内错误(空串 = 通过);字段级错误就地展示。 */
  error: string;
}

/** 初始化表单校验(US8-1):长度 ≥12、两次一致。纯函数供测试。 */
export function validateInit(pw: string, pw2: string): AuthValidation {
  if (pw.length < MIN_PASSWORD) {
    return { ok: false, error: `密码至少 ${MIN_PASSWORD} 个字符(当前 ${pw.length})` };
  }
  if (pw !== pw2) {
    return { ok: false, error: "两次输入的密码不一致" };
  }
  return { ok: true, error: "" };
}

function strengthHint(pw: string): { label: string; cls: string } {
  if (pw.length === 0) return { label: "", cls: "" };
  let classes = 0;
  if (/[a-z]/.test(pw) && /[A-Z]/.test(pw)) classes += 1;
  if (/\d/.test(pw)) classes += 1;
  if (/[^a-zA-Z0-9]/.test(pw)) classes += 1;
  const score = pw.length >= 16 ? classes + 1 : classes;
  if (pw.length < MIN_PASSWORD || score <= 1) return { label: "弱", cls: "tone-danger" };
  if (score === 2) return { label: "中", cls: "tone-warning" };
  return { label: "强", cls: "tone-normal" };
}

export function renderAuth(root: HTMLElement, onAuthed: () => void): void {
  const card = el("div", { class: "auth-card" });
  root.append(
    el(
      "main",
      { class: "auth-wrap" },
      el(
        "header",
        { class: "auth-logo" },
        el("span", { class: "brand-mark brand-mark-lg" }, "轻"),
        el("span", { class: "auth-title" }, "轻交付")
      ),
      el(
        "section",
        { class: "auth-panel", "aria-label": "轻交付管理入口" },
        el(
          "aside",
          { class: "auth-visual" },
          el("img", { class: "auth-image", src: workspaceImage, alt: "自然光下安静明亮的办公空间" }),
          el(
            "div",
            { class: "auth-visual-copy" },
            el("span", { class: "auth-eyebrow" }, "让交付更轻松"),
            el("h2", {}, "日常交付，", el("br"), "从容有序。"),
            el("p", {}, "从订单到交付，让重复的工作自动完成。"),
            el("span", { class: "auth-visual-caption" }, "自动发货 · 本地运行 · 随时掌握")
          )
        ),
        card
      ),
      el("footer", { class: "auth-footer" }, "轻交付 · 闲鱼虚拟商品自动交付工作台")
    )
  );

  const submit = async (path: string, body: unknown, button: HTMLButtonElement): Promise<void> => {
    button.disabled = true;
    const original = button.textContent;
    button.textContent = "提交中…";
    try {
      const res = await httpPost<SessionResponse>(path, body);
      applySession(res);
      // 重登返回原页面(US8-2):过期回落时 router 记忆了目标页
      const target = consumeReturnTo();
      if (target !== null && location.pathname !== target) {
        history.pushState(null, "", target);
      }
      onAuthed();
    } catch (e) {
      const status = e instanceof ApiHttpError ? e.status : null;
      if (status === 401 || status === 403) {
        // CSRF/预会话失效:刷新令牌重试一次
        try {
          await bootstrapSession();
          const res = await httpPost<SessionResponse>(path, body);
          applySession(res);
          const target = consumeReturnTo();
          if (target !== null && location.pathname !== target) {
            history.pushState(null, "", target);
          }
          onAuthed();
          return;
        } catch (e2) {
          renderInlineError(errorMessage(e2));
          return;
        }
      }
      renderInlineError(errorMessage(e));
    } finally {
      button.disabled = false;
      button.textContent = original ?? "提交";
    }
  };

  let errorSlot: HTMLElement | null = null;
  const renderInlineError = (message: string): void => {
    if (errorSlot === null) {
      errorSlot = el("p", { class: "error-inline", role: "alert" });
      card.append(errorSlot);
    }
    errorSlot.textContent = message;
  };
  const clearInlineError = (): void => {
    if (errorSlot !== null) errorSlot.textContent = "";
  };

  if (!session.initialized) {
    // 初始化卡片(US8-1):产品标识 + 密码表单 + 安全说明
    card.append(
      el("h1", { class: "card-title" }, "开始使用轻交付"),
      el("p", { class: "auth-description" }, "设置管理员密码，开启你的交付工作台。")
    );
    const pw = el("input", {
      id: "auth-password",
      type: "password",
      placeholder: `设置管理密码(≥${MIN_PASSWORD} 字符)`,
      autocomplete: "new-password",
      "aria-label": "管理密码"
    });
    const pw2 = el("input", {
      id: "auth-password-confirm",
      type: "password",
      placeholder: "再次输入密码",
      autocomplete: "new-password",
      "aria-label": "确认密码"
    });
    const hint = el("div", { class: "muted", style: "font-size:var(--text-xs);min-height:18px;" });
    const syncHint = (): void => {
      const s = strengthHint(pw.value);
      hint.className = s.cls === "" ? "muted" : `muted ${s.cls}`;
      hint.textContent = s.label === "" ? "" : `密码强度:${s.label}`;
    };
    pw.addEventListener("input", () => {
      syncHint();
      clearInlineError();
    });
    pw2.addEventListener("input", clearInlineError);
    const create = el("button", { class: "btn btn-primary", type: "submit" }, "创建管理员");
    const doCreate = (): void => {
      const check = validateInit(pw.value, pw2.value);
      if (!check.ok) {
        renderInlineError(check.error);
        return;
      }
      void submit(
        "/api/v1/auth/initialize",
        { password: pw.value, password_confirmation: pw2.value },
        create
      );
    };
    create.addEventListener("click", doCreate);
    for (const input of [pw, pw2]) {
      input.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter") {
          ev.preventDefault();
          doCreate();
        }
      });
    }
    card.append(
      el("div", { class: "auth-field" }, el("label", { for: "auth-password" }, "管理密码"), pw),
      el("div", { class: "auth-field" }, el("label", { for: "auth-password-confirm" }, "确认密码"), pw2),
      hint,
      create,
      el(
        "div",
        { class: "auth-note muted" },
        el("p", { style: "margin:4px 0;" }, "· 数据保存在本机数据目录,不会上传"),
        el("p", { style: "margin:4px 0;" }, "· 管理密码用于登录本页面;忘记时需停机用 CLI 重置"),
        el("p", { style: "margin:4px 0;" }, "· 初始化后即可扫码接入闲鱼账号")
      )
    );
  } else {
    // 登录卡片(US8-2):错误行内提示、登录中按钮态
    card.append(
      el("h1", { class: "card-title" }, "登录你的工作台"),
      el("p", { class: "auth-description" }, "欢迎回来，输入管理密码以继续。")
    );
    const pw = el("input", {
      id: "auth-password",
      type: "password",
      placeholder: "请输入管理密码",
      autocomplete: "current-password",
      "aria-label": "管理密码"
    });
    pw.addEventListener("input", clearInlineError);
    const login = el("button", { class: "btn btn-primary", type: "submit" }, "登录工作台 →");
    const doLogin = (): void => {
      if (pw.value.length === 0) {
        renderInlineError("请输入管理密码");
        return;
      }
      void submit("/api/v1/auth/login", { password: pw.value }, login);
    };
    login.addEventListener("click", doLogin);
    pw.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        doLogin();
      }
    });
    card.append(
      el("div", { class: "auth-field" }, el("label", { for: "auth-password" }, "管理密码"), pw),
      el(
        "details",
        { class: "auth-help" },
        el("summary", {}, "忘记密码？"),
        el("p", {}, "请先停止轻交付服务，再使用 init-admin --reset 命令重置管理密码。重置会使现有会话失效，并暂停全部账号。")
      ),
      login,
      el("p", { class: "auth-note muted" }, "本地运行，数据由你掌握。")
    );
  }
}
