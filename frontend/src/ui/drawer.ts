/** 侧滑抽屉(T015,研究 D7):Esc/遮罩关闭走脏确认、焦点管理(键盘可用性边界)。 */
import { el } from "../shared/dom";

export interface DrawerOptions {
  title: string;
  /** 返回 true 表示有未保存修改,关闭前确认。 */
  isDirty?: () => boolean;
}

export interface DrawerHandle {
  close(force?: boolean): void;
  body: HTMLElement;
  foot: HTMLElement;
}

export function openDrawer(opener: HTMLElement | null, opts: DrawerOptions): DrawerHandle {
  const openerDoc = opener ?? (document.activeElement as HTMLElement | null);

  const body = el("div", { class: "drawer-body" });
  const foot = el("div", { class: "drawer-foot" });
  const closeBtn = el("button", { class: "btn btn-ghost btn-sm", "aria-label": "关闭" }, "✕");
  const panel = el(
    "div",
    { class: "drawer", role: "dialog", "aria-modal": "true", "aria-label": opts.title },
    el("div", { class: "drawer-head", style: "padding:16px 20px;border-bottom:1px solid var(--border);" },
      el("h2", { class: "modal-title", style: "margin:0;" }, opts.title), closeBtn),
    body,
    foot
  );
  const overlay = el("div", { class: "drawer-overlay" });
  overlay.append(panel);
  document.body.append(overlay);
  const focusable = panel.querySelector<HTMLElement>("input, select, textarea");
  window.setTimeout(() => (focusable ?? closeBtn).focus(), 0);

  const doClose = (): void => {
    document.removeEventListener("keydown", onKeydown);
    overlay.remove();
    openerDoc?.focus();
  };
  const tryClose = (): void => {
    if (opts.isDirty?.()) {
      if (!window.confirm("有未保存的修改,确定放弃?")) return;
    }
    doClose();
  };

  const onKeydown = (ev: KeyboardEvent): void => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      tryClose();
    }
  };

  const close = (force = false): void => {
    if (!force && opts.isDirty?.()) {
      tryClose();
      return;
    }
    doClose();
  };

  closeBtn.addEventListener("click", () => tryClose());
  overlay.addEventListener("mousedown", (ev) => {
    if (ev.target === overlay) tryClose();
  });
  document.addEventListener("keydown", onKeydown);

  return { close, body, foot };
}
