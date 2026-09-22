import type { PageFactory } from "../../app/page";
import { el, errorMessage } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord } from "../../shared/contracts";

const STATE_LABELS: Record<string, string> = {
  open: "待处理",
  resolved: "已解决",
  terminated: "已终止"
};

const KIND_LABELS: Record<string, string> = {
  delivery_unknown: "发送结果未知",
  delivery_rejected: "平台拒绝",
  delivery_retry_exhausted: "重试预算耗尽",
  delivery_ineligible: "资格不满足"
};

export const issuesPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "待处理"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  try {
    const raw = await httpGet("/api/v1/issues");
    const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (items.length === 0) {
      status.textContent = "当前没有待处理事项。未知结果与失败会持久出现在这里,直到人工处理。";
      return;
    }
    status.textContent = "";
    const table = el("table", { class: "list" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "类别"),
        el("th", {}, "原因"),
        el("th", {}, "订单"),
        el("th", {}, "状态"),
        el("th", {}, "出现时间")
      )
    );
    for (const raw of items) {
      if (!isRecord(raw)) continue;
      const kind = String(raw["category"] ?? "");
      table.append(
        el(
          "tr",
          {},
          el("td", {}, KIND_LABELS[kind] ?? kind),
          el("td", {}, String(raw["reason"] ?? "")),
          el("td", {}, raw["order_id"] ? String(raw["order_id"]).slice(0, 14) + "…" : "—"),
          el("td", {}, STATE_LABELS[String(raw["state"] ?? "open")] ?? String(raw["state"])),
          el("td", {}, String(raw["created_at"] ?? ""))
        )
      );
    }
    root.append(table);
    root.append(
      el(
        "p",
        { class: "muted" },
        "人工动作(补发/确认收到/终止/接管)在订单详情页执行;需要原因与风险确认。"
      )
    );
  } catch (e) {
    status.textContent = `待处理加载失败:${errorMessage(e)}`;
  }
};
