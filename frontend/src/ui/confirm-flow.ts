/** 统一确认流(US6/T045,FR-005/FR-018):影响说明 + 原因必填(改变交付结果的操作)
 *  + 高危风险勾选 + 提交中锁定 + 成功反馈。 */
import { el } from "../shared/dom";
import { openModal } from "./modal";
import { btn } from "./dom";

export interface ConfirmFlowOptions {
  title: string;
  impact: string | Node;
  /** 改变交付结果的操作一律原因必填(FR-018)。 */
  requireReason: boolean;
  /** 高危操作必须勾选风险确认(如补发可能重复发送)。 */
  requireRiskAck: boolean;
  riskText?: string;
  confirmLabel: string;
  danger?: boolean;
  submit: (reason: string, riskConfirmed: boolean) => Promise<void>;
  onSuccess?: () => void;
}

export function openConfirmFlow(opts: ConfirmFlowOptions): void {
  const opener = document.activeElement as HTMLElement | null;
  const handle = openModal(opener, { title: opts.title });

  const reasonInput = el("input", { type: "text", placeholder: "原因(必填)" }) as HTMLInputElement;
  const riskCheck = el("input", { type: "checkbox" });
  const errLine = el("div", { class: "field-error" }, "");
  const confirmBtn = btn(opts.confirmLabel, { variant: opts.danger === true ? "danger" : "primary" });
  const cancelBtn = btn("取消", { variant: "secondary" });

  const validate = (): void => {
    let ok = true;
    errLine.textContent = "";
    if (opts.requireReason && reasonInput.value.trim() === "") {
      errLine.textContent = "必须填写原因";
      ok = false;
    }
    if (opts.requireRiskAck && !riskCheck.checked) {
      errLine.textContent = "必须勾选风险确认";
      ok = false;
    }
    confirmBtn.disabled = !ok;
  };
  reasonInput.addEventListener("input", validate);
  riskCheck.addEventListener("change", validate);
  validate();

  confirmBtn.addEventListener("click", async () => {
    confirmBtn.disabled = true;
    confirmBtn.textContent = "提交中…";
    try {
      await opts.submit(reasonInput.value.trim(), riskCheck.checked);
      handle.close(true);
      opts.onSuccess?.();
    } catch (e) {
      errLine.textContent = `失败:${e instanceof Error ? e.message : String(e)}`;
      confirmBtn.textContent = opts.confirmLabel;
      confirmBtn.disabled = false;
    }
  });
  cancelBtn.addEventListener("click", () => handle.close(true));

  const reasonRow =
    opts.requireReason || opts.requireRiskAck
      ? el(
          "div",
          {},
          opts.requireReason
            ? el("div", { class: "field", style: "margin-top:12px;" }, el("label", {}, "操作原因"), reasonInput)
            : null,
          opts.requireRiskAck
            ? el(
                "label",
                { class: "check-row" },
                riskCheck,
                opts.riskText ?? "我已知晓本操作的风险"
              )
            : null,
          errLine
        )
      : null;

  handle.body.append(
    el("div", { class: "confirm-message" }, opts.impact),
    reasonRow ?? el("div", { style: "height:8px;" }),
    el("div", { style: "display:flex;gap:12px;justify-content:flex-end;margin-top:16px;" }, cancelBtn, confirmBtn)
  );
}
