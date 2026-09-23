/** 居中弹窗(T014,研究 D7):叠层至多一层可交互、Esc 关闭、
 *  关闭焦点回归触发元素、脏检查确认(键盘可用性边界/FR-005)。 */
import { el } from "../shared/dom";
import { btn } from "./dom";

export interface ModalOptions {
  title: string;
  /** 返回 true 表示有未保存修改,关闭前弹确认。 */
  isDirty?: () => boolean;
  /** 自定义关闭前确认(默认浏览器 confirm;界面统一走确认弹窗的场景由调用方接管)。 */
  confirmDiscard?: () => boolean;
}

export interface ModalHandle {
  close(force?: boolean): void;
  body: HTMLElement;
}

export function openModal(opener: HTMLElement | null, opts: ModalOptions): ModalHandle {
  const openerDoc = opener ?? (document.activeElement as HTMLElement | null);
  const prevOverlay = document.querySelector(".overlay");
  if (prevOverlay !== null) prevOverlay.remove(); // 叠层至多一层

  const body = el("div", { class: "modal-body" });
  const closeBtn = el("button", { class: "btn btn-ghost btn-sm", "aria-label": "关闭" }, "✕");
  const modal = el(
    "div",
    { class: "modal", role: "dialog", "aria-modal": "true", "aria-label": opts.title },
    el("div", { class: "modal-head-row" }, el("h2", { class: "modal-title" }, opts.title), closeBtn),
    body
  );
  const overlay = el("div", { class: "overlay" }, modal);
  document.body.append(overlay);
  const focusable = modal.querySelector<HTMLElement>("input, select, textarea, button:not([aria-label='关闭'])");
  (focusable ?? closeBtn).focus();

  const doClose = (): void => {
    document.removeEventListener("keydown", onKeydown);
    overlay.remove();
    openerDoc?.focus();
  };
  const tryClose = (): void => {
    if (opts.isDirty?.()) {
      const ok = opts.confirmDiscard ? opts.confirmDiscard() : window.confirm("有未保存的修改,确定放弃?");
      if (!ok) return;
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

  closeBtn.addEventListener("click", tryClose);
  overlay.addEventListener("mousedown", (ev) => {
    if (ev.target === overlay) tryClose();
  });
  document.addEventListener("keydown", onKeydown);

  return { close, body };
}

/** 确认/信息弹窗(FR-005 破坏性操作统一入口):影响说明 + 确认/取消。
 *  onConfirm 缺省时为纯信息展示(如验证指引)。 */
export function confirmDialog(opts: {
  title: string;
  message: string | Node;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm?: () => void | Promise<void>;
}): void {
  const opener = document.activeElement as HTMLElement | null;
  const handle = openModal(opener, { title: opts.title });
  const confirmBtn = btn(opts.confirmLabel ?? "确认", { variant: opts.danger === true ? "danger" : "primary" });
  const cancelBtn = btn("取消", { variant: "secondary" });
  confirmBtn.addEventListener("click", async () => {
    confirmBtn.disabled = true;
    try {
      await opts.onConfirm?.();
    } finally {
      handle.close();
    }
  });
  cancelBtn.addEventListener("click", () => handle.close());
  handle.body.append(
    el("div", { class: "confirm-message" }, opts.message),
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:16px;" }, cancelBtn, confirmBtn)
  );
}
