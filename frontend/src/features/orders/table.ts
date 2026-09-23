/** 订单表格(US5/T040):双轴状态标签 + 人工接管标识 + 长文本截断。 */
import { el } from "../../shared/dom";
import { statusPairEl, badgeEl } from "../../ui/badge";
import { truncate } from "../../ui/dom";
import { formatAbsolute, formatRelative } from "../../ui/time";
import type { OrderRow } from "./model";

export function renderOrdersTable(
  rows: OrderRow[],
  accountNames: Map<string, string>,
  itemTitles: Map<string, string>,
  onOpenDetail: (row: OrderRow) => void
): HTMLElement {
  const table = el("table", { class: "data" });
  table.append(
    el(
      "tr",
      {},
      el("th", {}, "付款时间"),
      el("th", {}, "订单号"),
      el("th", {}, "账号"),
      el("th", {}, "商品"),
      el("th", {}, "金额"),
      el("th", {}, "状态")
    )
  );
  for (const o of rows) {
    const tr = el(
      "tr",
      { class: "row-click" },
      el("td", { class: "muted", title: o.paidAt === null ? "" : formatAbsolute(o.paidAt) }, o.paidAt ? formatRelative(o.paidAt) : "—"),
      el("td", { class: "mono" }, truncate(o.platformOrderId, 16)),
      el("td", {}, accountNames.get(o.accountId) ?? o.accountId.slice(0, 8)),
      el("td", {}, o.itemId === null ? el("span", { class: "muted" }, "—") : truncate(itemTitles.get(o.itemId) ?? o.itemId, 16)),
      el("td", { class: "num" }, o.amountMinor === null ? "—" : `${(o.amountMinor / 100).toFixed(2)}`),
      el("td", {}, statusPairEl(o.deliveryState, o.confirmationState))
    );
    tr.addEventListener("click", () => onOpenDetail(o));
    table.append(tr);
  }
  return table;
}
