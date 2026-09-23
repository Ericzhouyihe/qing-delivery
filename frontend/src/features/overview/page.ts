/** 概览工作台(US2,T022-T025):统计卡下钻、恢复横幅、账号速览、
 *  最近订单/待处理、快捷动作;30s 可见性轮询 + 手动刷新(FR-006/FR-008)。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord, parseDashboardSummary, parsePage, type DashboardSummary } from "../../shared/contracts";
import { statCard } from "../../ui/dom";
import { badgeEl, statusPairEl, accountBadge } from "../../ui/badge";
import { asyncBlock } from "../../ui/states";
import { startPoll } from "../../ui/poll";
import { formatAbsolute, formatRelative, startTicker } from "../../ui/time";
import { setPendingIssues } from "../../app/store";
import { deriveStats, sortQuickAccounts, type QuickAccount } from "./model";
import type { PageFactory } from "../../app/page";

interface RecentOrder {
  id: string;
  platform_order_id: string;
  paid_at: string | null;
  amount_minor: number | null;
  currency: string | null;
  delivery_state: string | null;
  confirmation_state: string | null;
}

interface RecentIssue {
  id: string;
  kind: string;
  order_id: string | null;
  state: string;
  created_at: string;
}

const ISSUE_KIND_LABELS: Record<string, string> = {
  delivery_unknown: "发送结果未知",
  delivery_rejected: "平台拒绝",
  delivery_retry_exhausted: "重试预算耗尽",
  delivery_ineligible: "资格不满足",
  restore_review: "恢复核对",
  token_expired: "令牌过期"
};

function parseOrder(v: unknown): RecentOrder {
  if (!isRecord(v)) throw new Error("订单格式错误");
  return {
    id: String(v["id"] ?? ""),
    platform_order_id: String(v["platform_order_id"] ?? ""),
    paid_at: typeof v["paid_at"] === "string" ? v["paid_at"] : null,
    amount_minor: (() => { const a = v["amount"]; return isRecord(a) && typeof a["minor_units"] === "number" ? (a["minor_units"] as number) : null; })(),
    currency: (() => { const a = v["amount"]; return isRecord(a) && typeof a["currency"] === "string" ? (a["currency"] as string) : null; })(),
    delivery_state: typeof v["delivery_state"] === "string" ? v["delivery_state"] : null,
    confirmation_state: typeof v["confirmation_state"] === "string" ? v["confirmation_state"] : null
  };
}

function parseAccount(v: unknown): QuickAccount {
  if (!isRecord(v)) throw new Error("账号格式错误");
  return {
    id: String(v["id"] ?? ""),
    displayName: String(v["display_name"] ?? ""),
    connectionState: String(v["connection_state"] ?? "paused"),
    runEnabled: v["run_enabled"] === true,
    autoDelivery: v["auto_delivery_enabled"] === true,
    monitoringSince: typeof v["monitoring_since"] === "string" ? v["monitoring_since"] : null
  };
}

function parseIssue(v: unknown): RecentIssue {
  if (!isRecord(v)) throw new Error("事项格式错误");
  return {
    id: String(v["id"] ?? ""),
    kind: String(v["kind"] ?? ""),
    order_id: v["order_id"] === null || v["order_id"] === undefined ? null : String(v["order_id"]),
    state: String(v["state"] ?? "open"),
    created_at: String(v["created_at"] ?? "")
  };
}

export const overviewPage: PageFactory = (root) => {
  const bannerArea = el("div", {});
  const statArea = el("div", { class: "stat-grid" });
  const actionsArea = el("div", { class: "quick-actions", style: "margin-bottom:16px;" });
  const quickView = el("div", {});
  const ordersView = el("div", {});
  const issuesView = el("div", {});
  const refreshBtn = el("button", { class: "btn btn-secondary btn-sm" }, "↻ 刷新");
  const refreshHint = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");

  root.append(bannerArea, statArea, actionsArea, refreshBtn, refreshHint);
  root.append(
    el(
      "div",
      { style: "display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px;align-items:start;" },
      el("section", { class: "card" }, el("h2", { class: "card-title" }, "账号状态速览"), quickView),
      el("section", { class: "card" }, el("h2", { class: "card-title" }, "最近待处理"), issuesView)
    ),
    el("section", { class: "card" }, el("h2", { class: "card-title" }, "最近订单"), ordersView)
  );

  // ---- 统计卡:固定四张,数值节点原地更新,轮询不重建(US2-6 无布局跳变) ----
  const onlineValue = el("span", {}, "—");
  const ordersValue = el("span", {}, "—");
  const deliveredValue = el("span", {}, "—");
  const pendingValue = el("span", {}, "—");
  statArea.append(
    statCard("在线账号 / 总数", onlineValue, { href: "/accounts", sub: "账号管理" }),
    statCard("今日订单", ordersValue, { href: "/orders", sub: "本地时区自然日" }),
    statCard("今日已自动交付", deliveredValue, { href: "/orders", sub: "本地时区自然日" }),
    statCard("待人工处理", pendingValue, { href: "/issues", tone: "warning", sub: "待处理工作台" })
  );
  const pendingCard = statArea.lastElementChild as HTMLElement;
  const emptyQuickView = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "👤"),
      el("div", {}, "还没有账号"),
      el("a", { class: "btn btn-primary btn-sm", href: "/accounts" }, "去扫码接入")
    );
  const emptyIssuesView = (): HTMLElement =>
    el("div", { class: "empty-state" }, el("div", { class: "empty-ico" }, "✅"), el("div", {}, "全部处理完毕"));
  const emptyOrdersView = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "🧾"),
      el("div", {}, "暂无订单;规则配置完成后,付款订单会出现在这里")
    );

  actionsArea.append(
    el("a", { class: "btn btn-primary btn-sm", href: "/accounts" }, "＋ 扫码接入账号"),
    el("a", { class: "btn btn-secondary btn-sm", href: "/catalog" }, "⟳ 同步商品"),
    el("a", { class: "btn btn-secondary btn-sm", href: "/orders" }, "⇄ 打开订单中心")
  );

  const renderStats = (d: DashboardSummary): void => {
    const s = deriveStats(d);
    onlineValue.textContent = `${s.accountsOnline} / ${s.accountsTotal}`;
    if (s.ordersToday === null) {
      ordersValue.textContent = "—";
      ordersValue.title = "统计不可用(服务版本过旧)";
    } else {
      ordersValue.textContent = String(s.ordersToday);
      ordersValue.title = "";
    }
    if (s.deliveredToday === null) {
      deliveredValue.textContent = "—";
      deliveredValue.title = "统计不可用(服务版本过旧)";
    } else {
      deliveredValue.textContent = String(s.deliveredToday);
      deliveredValue.title = "";
    }
    pendingValue.textContent = String(s.pending);
    pendingCard.classList.toggle("tone-warning", s.pending > 0);
    setPendingIssues(s.pending);

    // 横幅(US2/FR-007):恢复隔离置顶,服务停止次之
    bannerArea.replaceChildren();
    if (s.restoreActive) {
      const go = el("a", { href: "/issues" }, "去核对 →");
      bannerArea.append(
        el(
          "div",
          { class: "banner banner-warning", role: "alert" },
          el("strong", {}, "恢复隔离中"),
          el("span", {}, `待核对 ${s.restoreUnresolved} 笔;逐单核对完成后才恢复正常处理`),
          go
        )
      );
    }
    if (s.stopping) {
      bannerArea.append(
        el("div", { class: "banner banner-danger", role: "alert" }, el("strong", {}, "服务正在停止"), el("span", {}, "不再接受新任务"))
      );
    }
  };

  const loadDashboard = async (): Promise<void> => {
    renderStats(parseDashboardSummary(await httpGet("/api/v1/dashboard")));
  };

  const loadQuickView = (): void => {
    quickBlock.reload();
  };
  const quickBlock = asyncBlock(quickView, async () => {
      const raw = await httpGet("/api/v1/accounts");
      const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
      const accounts = sortQuickAccounts(items.map(parseAccount));
      if (accounts.length === 0) {
        return null; // 空态由 asyncBlock 兜底
      }
      const table = el("table", { class: "data" });
      table.append(el("tr", {}, el("th", {}, "账号"), el("th", {}, "状态"), el("th", {}, "监控起点")));
      const timeCells: Array<[HTMLElement, string]> = [];
      for (const a of accounts) {
        const timeCell = el("td", { class: "muted" }, a.monitoringSince ? formatRelative(a.monitoringSince) : "未启用");
        if (a.monitoringSince !== null) timeCells.push([timeCell, a.monitoringSince]);
        table.append(
          el(
            "tr",
            {},
            el("td", {}, el("strong", {}, a.displayName), el("div", { class: "muted", style: "font-size:var(--text-xs);" },
              a.autoDelivery ? "自动交付开" : "自动交付关")),
            el("td", {}, badgeEl(accountBadge(a.connectionState), a.connectionState)),
            timeCell
          )
        );
      }
      stopTicker = startTicker(() => {
        for (const [cell, iso] of timeCells) cell.textContent = formatRelative(iso);
      }, 30_000);
      return table;
    }, { empty: emptyQuickView });

  const loadOrders = (): void => {
    ordersBlock.reload();
  };
  const ordersBlock = asyncBlock(ordersView, async () => {
      const raw = await httpGet("/api/v1/orders?limit=10");
      const page = parsePage(raw, parseOrder);
      if (page.items.length === 0) {
        return null;
      }
      const table = el("table", { class: "data" });
      table.append(el("tr", {}, el("th", {}, "时间"), el("th", {}, "订单号"), el("th", {}, "金额"), el("th", {}, "状态")));
      for (const o of page.items) {
        const tr = el(
          "tr",
          { class: "row-click" },
          el("td", { class: "muted", title: o.paid_at ? formatAbsolute(o.paid_at) : "" }, o.paid_at ? formatRelative(o.paid_at) : "—"),
          el("td", {}, o.platform_order_id),
          el("td", { class: "num" }, o.amount_minor === null ? "—" : `${(o.amount_minor / 100).toFixed(2)}`),
          el("td", {}, statusPairEl(o.delivery_state, o.confirmation_state))
        );
        // 下钻订单中心并聚焦该单(US2-2/SC-203)
        tr.addEventListener("click", () => {
          history.pushState(null, "", `/orders?focus=${o.id}`);
          window.dispatchEvent(new PopStateEvent("popstate"));
        });
        table.append(tr);
      }
      return table;
    }, { empty: emptyOrdersView });

  const loadIssues = (): void => {
    issuesBlock.reload();
  };
  const issuesBlock = asyncBlock(issuesView, async () => {
      const raw = await httpGet("/api/v1/issues?state=open&limit=5");
      const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
      const issues = items.map(parseIssue);
      if (issues.length === 0) {
        return null;
      }
      const list = el("ul", { class: "timeline" });
      for (const it of issues) {
        list.append(
          el(
            "li",
            { class: "tl-warning" },
            el("div", {}, el("strong", {}, ISSUE_KIND_LABELS[it.kind] ?? it.kind)),
            el("div", { class: "tl-time" }, `${formatRelative(it.created_at)} · 订单 ${it.order_id?.slice(0, 12) ?? "—"}…`)
          )
        );
      }
      const more = el("a", { href: "/issues" }, "查看全部待处理 →");
      return el("div", {}, list, more);
    }, { empty: emptyIssuesView });

  let stopTicker: (() => void) | null = null;
  const poll = startPoll(loadDashboard, 30_000);
  loadQuickView();
  loadOrders();
  loadIssues();
  refreshBtn.addEventListener("click", () => {
    void loadDashboard();
    loadQuickView();
    loadOrders();
    loadIssues();
  });
  refreshHint.textContent = "概览每 30 秒自动刷新(页面可见时)";

  return () => {
    poll.stop();
    stopTicker?.();
    quickBlock.dispose();
    ordersBlock.dispose();
    issuesBlock.dispose();
  };
};
