import type { PageFactory } from "../../app/page";
import { el, errorMessage } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";

/** 设置页:恢复核对(T084)。总开关不能清除逐单隔离;决定需原因与风险确认。 */
export const settingsPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "设置"));
  root.append(el("h2", {}, "恢复核对"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  const list = el("div", {});
  root.append(list);

  try {
    const summary = (await httpGet("/api/v1/restore")) as Record<string, unknown> | null;
    if (summary === null || !isRecord(summary)) {
      status.textContent = "当前没有恢复隔离。备份恢复后,这里会列出需要逐单核对的订单。";
      return;
    }
    status.textContent = `恢复隔离中:待核对 ${String(summary["unresolved_count"] ?? "?")} 笔(账号已全部暂停;核对不解除新订单的正常处理)。`;
    const raw = await httpGet("/api/v1/restore/reviews?state=unresolved");
    const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (items.length === 0) {
      list.append(el("p", { class: "muted" }, "没有未核对事项。"));
      return;
    }
    for (const item of items) {
      if (!isRecord(item)) continue;
      const card = el("div", { class: "preview" });
      const id = String(item["id"]);
      card.append(
        el("strong", {}, `${String(item["kind"])} · 订单 ${String(item["order_id"] ?? "—").slice(0, 14)}…`),
        el("p", { class: "muted" }, "received=确认已收到(需风险确认);approved_not_sent=确认未发送;terminated=终止旧路径")
      );
      const reason = el("input", { type: "text", placeholder: "原因(必填)" });
      const risk = el("input", { type: "checkbox" });
      const btn = el("button", {}, "提交决定");
      const result = el("span", { class: "muted" }, "");
      btn.addEventListener("click", async () => {
        result.textContent = "提交中…";
        const decision = risk.checked ? "received" : "approved_not_sent";
        try {
          await httpPost(`/api/v1/restore/reviews/${id}/decisions`, {
            expected_version: 1,
            decision,
            reason: reason.value || "界面核对",
            evidence_note: "管理页核对",
            acknowledge_duplicate_risk: risk.checked,
          });
          result.textContent = "已记录";
          card.remove();
        } catch (e) {
          result.textContent = `失败:${errorMessage(e)}`;
        }
      });
      card.append(reason, el("label", {}, " 确认已收到(风险确认)"), risk, btn, result);
      list.append(card);
    }
  } catch (e) {
    status.textContent = `恢复状态加载失败:${errorMessage(e)}`;
  }
};
