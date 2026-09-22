import type { PageFactory } from "../../app/page";
import { el, errorMessage } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord, parsePage } from "../../shared/contracts";

interface OrderRow {
  id: string;
  account_id: string;
  platform_order_id: string;
  paid_at: string | null;
  quantity: number | null;
  trade_type: string;
  platform_state: string;
  delivery_state: string | null;
  confirmation_state: string | null;
}

interface OrderDetail {
  summary: OrderRow;
  delivery: {
    id: string | null;
    state: string | null;
    raw_content_state: string | null;
    review_state: string | null;
    confirmation_state: string | null;
    accepted_evidence: string;
    automatic_retries_used: number;
    attempts: Array<{ id: string; kind: string; sequence: number; state: string }>;
  } | null;
}

/** 双状态分开呈现:内容交付与平台确认是两条独立结果轴(FR-016)。 */
const DELIVERY_LABELS: Record<string, string> = {
  awaiting_verification: "待核验",
  ready: "待发送",
  sending: "发送中",
  delivered: "内容已交付",
  definitely_not_sent: "确定未发送",
  unknown: "结果未知",
  needs_review: "等待人工",
  terminated: "已终止"
};

const CONFIRM_LABELS: Record<string, string> = {
  disabled: "确认未开启",
  pending: "确认待执行",
  dispatching: "确认中",
  succeeded: "平台已确认",
  failed: "确认失败",
  unknown: "确认未知",
  terminated: "确认已终止"
};

function parseOrder(v: unknown): OrderRow {
  if (!isRecord(v)) throw new Error("订单格式错误");
  const r = v;
  return {
    id: String(r["id"]),
    account_id: String(r["account_id"]),
    platform_order_id: String(r["platform_order_id"]),
    paid_at: typeof r["paid_at"] === "string" ? r["paid_at"] : null,
    quantity: typeof r["quantity"] === "number" ? r["quantity"] : null,
    trade_type: String(r["trade_type"] ?? "unknown"),
    platform_state: String(r["platform_state"] ?? "unknown"),
    delivery_state: typeof r["delivery_state"] === "string" ? r["delivery_state"] : null,
    confirmation_state:
      typeof r["confirmation_state"] === "string" ? r["confirmation_state"] : null
  };
}

export const ordersPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "订单"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  const detail = el("div", {});
  root.append(detail);

  try {
    const raw = await httpGet("/api/v1/orders?limit=50");
    const page = parsePage(raw, parseOrder);
    status.textContent =
      page.items.length === 0 ? "暂无订单;账号接入与规则配置完成后,付款订单会出现在这里。" : "";

    const table = el("table", { class: "list" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "订单号"),
        el("th", {}, "付款时间"),
        el("th", {}, "数量"),
        el("th", {}, "平台状态"),
        el("th", {}, "内容交付"),
        el("th", {}, "平台确认")
      )
    );
    for (const o of page.items) {
      const link = el("a", { href: `/orders/${o.id}` }, o.platform_order_id);
      link.addEventListener("click", async (ev) => {
        ev.preventDefault();
        await showDetail(detail, o);
      });
      table.append(
        el(
          "tr",
          {},
          el("td", {}, link),
          el("td", {}, o.paid_at ?? "—"),
          el("td", {}, o.quantity === null ? "未知" : String(o.quantity)),
          el("td", {}, o.platform_state),
          el("td", {}, o.delivery_state ? (DELIVERY_LABELS[o.delivery_state] ?? o.delivery_state) : "—"),
          el(
            "td",
            {},
            o.confirmation_state ? (CONFIRM_LABELS[o.confirmation_state] ?? o.confirmation_state) : "—"
          )
        )
      );
    }
    root.insertBefore(table, status.nextSibling);
  } catch (e) {
    status.textContent = `订单加载失败:${errorMessage(e)}`;
  }
};

async function showDetail(container: HTMLElement, summary: OrderRow): Promise<void> {
  container.replaceChildren(el("p", { class: "muted" }, "加载详情…"));
  try {
    const raw = await httpGet(`/api/v1/accounts/${summary.account_id}/orders/${summary.id}`);
    const d = raw as OrderDetail;
    const attempts = d.delivery?.attempts ?? [];
    container.replaceChildren(
      el("h2", {}, `订单 ${summary.platform_order_id}`),
      el(
        "p",
        {},
        `内容交付:${d.delivery?.state ? (DELIVERY_LABELS[d.delivery.state] ?? d.delivery.state) : "—"}(底层分类:${
          d.delivery?.raw_content_state ?? "无"
        },证据:${d.delivery?.accepted_evidence ?? "none"})`
      ),
      el(
        "p",
        {},
        `平台确认:${
          d.delivery?.confirmation_state
            ? (CONFIRM_LABELS[d.delivery.confirmation_state] ?? d.delivery.confirmation_state)
            : "—"
        };自动重试已用 ${d.delivery?.automatic_retries_used ?? 0} 次`
      ),
      el("h3", {}, "执行时间线"),
      attempts.length === 0
        ? el("p", { class: "muted" }, "暂无尝试记录")
        : el(
            "table",
            { class: "list" },
            el("tr", {}, el("th", {}, "#"), el("th", {}, "动作"), el("th", {}, "状态")),
            ...attempts.map((a) =>
              el(
                "tr",
                {},
                el("td", {}, String(a.sequence)),
                el("td", {}, a.kind),
                el("td", {}, a.state)
              )
            )
          )
    );
  } catch (e) {
    container.replaceChildren(el("p", { class: "error" }, `详情加载失败:${errorMessage(e)}`));
  }
}
