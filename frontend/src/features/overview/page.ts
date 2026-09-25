/** 运营概览(005 改版):标题区+系统状态指示、时间范围筛选(自然日对齐,默认 7天内)、
 *  四张统计卡(区间营收+对比徽标/活跃账号/订单数/待人工处理)、营收趋势区;
 *  既有运营区块(账号速览/最近待处理/最近订单)保留在统计区下方(FR-014)。
 *  30s 可见性轮询 + 手动刷新按当前范围(FR-013);数值与图表原地更新(FR-015)。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord, parsePage, parseStatsOverview, type StatsOverview } from "../../shared/contracts";
import { badgeEl, statusPairEl, accountBadge } from "../../ui/badge";
import { renderTrendChart } from "../../ui/chart";
import { asyncBlock } from "../../ui/states";
import { startPoll } from "../../ui/poll";
import { formatAbsolute, formatRelative, startTicker } from "../../ui/time";
import { setPendingIssues } from "../../app/store";
import {
  DEFAULT_PRESET,
  PRESET_OPTIONS,
  deriveCards,
  resolveCustomRange,
  resolvePresetRange,
  sortQuickAccounts,
  type QuickAccount,
  type RangePreset
} from "./model";
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
  // ---- 标题区(FR-001):标题 + 副文案 + 系统运行状态指示 ----
  const statusChip = el("span", { class: "ov-status" }, el("span", { class: "dot" }), el("span", { class: "ov-status-text" }, "—"));
  const header = el(
    "div",
    { class: "ov-header" },
    el(
      "div",
      {},
      el("h1", { class: "ov-title" }, "运营概览"),
      el("p", { class: "ov-sub muted" }, "欢迎回来,以下是店铺的实时经营数据。")
    ),
    statusChip
  );

  const bannerArea = el("div", {});

  // ---- 时间范围筛选(FR-002/FR-009):预设自然日对齐,默认 7天内;自定义起止日期 ----
  const chipsRow = el("div", { class: "range-chips", role: "tablist" });
  const chipButtons = new Map<RangePreset, HTMLButtonElement>();
  let currentPreset: RangePreset = DEFAULT_PRESET;
  /** 非空表示自定义区间生效(FR-009);预设点击即清除。 */
  let customRange: { from: number; to: number } | null = null;

  const customStart = el("input", { type: "date", "aria-label": "开始日期" }) as HTMLInputElement;
  const customEnd = el("input", { type: "date", "aria-label": "结束日期" }) as HTMLInputElement;
  const customError = el("span", { class: "range-error" }, "");
  const customPanel = el(
    "span",
    { class: "range-custom" },
    el("span", { class: "muted" }, "起"),
    customStart,
    el("span", { class: "muted" }, "止"),
    customEnd,
    el("button", { class: "btn btn-primary btn-sm", type: "button" }, "应用"),
    el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "取消"),
    customError
  );
  customPanel.hidden = true;
  const [applyBtn, cancelBtn] = customPanel.querySelectorAll("button");

  const customChip = el("button", { class: "chip", type: "button", role: "tab" }, "自定义") as HTMLButtonElement;
  for (const opt of PRESET_OPTIONS) {
    const btn = el("button", { class: "chip", type: "button", role: "tab" }, opt.label) as HTMLButtonElement;
    btn.addEventListener("click", () => {
      const changed = currentPreset !== opt.key || customRange !== null;
      currentPreset = opt.key;
      customRange = null;
      customPanel.hidden = true;
      syncChipActive();
      if (changed) void loadStats();
    });
    chipButtons.set(opt.key, btn);
    chipsRow.append(btn);
  }
  chipsRow.append(customChip, customPanel);

  const syncChipActive = (): void => {
    for (const [key, btn] of chipButtons) {
      const active = customRange === null && key === currentPreset;
      btn.classList.toggle("active", active);
      btn.setAttribute("aria-selected", active ? "true" : "false");
    }
    customChip.classList.toggle("active", customRange !== null);
    customChip.setAttribute("aria-selected", customRange !== null ? "true" : "false");
  };
  syncChipActive();

  customChip.addEventListener("click", () => {
    customPanel.hidden = !customPanel.hidden;
    customError.textContent = "";
    if (!customPanel.hidden && customStart.value === "" && customEnd.value === "") {
      const { from } = resolvePresetRange(DEFAULT_PRESET, Date.now());
      const d = new Date(from);
      const iso = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      customStart.value = iso;
      customEnd.value = iso;
    }
  });
  applyBtn?.addEventListener("click", () => {
    const range = resolveCustomRange(customStart.value, customEnd.value);
    if (range === null) {
      // 拦截:提示且不发起请求、不改变当前展示(FR-009)
      customError.textContent = "区间无效:结束不得早于开始,跨度最大 92 天";
      return;
    }
    customError.textContent = "";
    customRange = range;
    customPanel.hidden = true;
    syncChipActive();
    void loadStats();
  });
  cancelBtn?.addEventListener("click", () => {
    customError.textContent = "";
    customPanel.hidden = true;
  });

  // ---- 四张统计卡(FR-003~006):数值节点原地更新,轮询不重建 ----
  const statArea = el("div", { class: "stat-grid ov-grid" });
  const revValue = el("span", {}, "—");
  const revBadge = el("span", { class: "ov-badge" });
  revBadge.hidden = true;
  const revValueRow = el("div", { class: "stat-value ov-value-row" }, revValue, revBadge);
  const acctValue = el("span", {}, "—");
  const orderValue = el("span", {}, "—");
  const pendingValue = el("span", {}, "—");
  const statCard = (
    icon: string,
    label: string,
    valueNode: HTMLElement,
    opts: { href?: string; sub?: string } = {}
  ): HTMLElement => {
    const card = el("div", { class: "stat-card ov-card" });
    card.append(el("div", { class: "stat-label" }, el("span", { class: "ov-ico", "aria-hidden": "true" }, icon), label));
    card.append(valueNode);
    if (opts.sub !== undefined) card.append(el("div", { class: "stat-sub muted" }, opts.sub));
    if (opts.href !== undefined) {
      const link = el("a", { class: "stat-link", href: opts.href }, "查看 →");
      card.append(link);
      card.classList.add("stat-clickable");
    }
    return card;
  };
  const revenueCard = statCard("¥", "累计营收", revValueRow, { sub: "所选区间 · CNY" });
  const accountsCard = statCard("👥", "活跃账号 / 总数", el("div", { class: "stat-value" }, acctValue), {
    href: "/accounts",
    sub: "在线 / 总数"
  });
  const ordersCard = statCard("🛒", "订单数", el("div", { class: "stat-value" }, orderValue), {
    href: "/orders",
    sub: "所选区间付款订单"
  });
  const pendingCard = statCard("🚩", "待人工处理", el("div", { class: "stat-value" }, pendingValue), {
    href: "/issues",
    sub: "待处理工作台"
  });
  // 007 T018:第 5 张统计卡"库存卡密余量";value span 原地更新,轮询不重建 DOM
  const stockValue = el("span", {}, "—");
  const stockCard = statCard("▤", "库存卡密余量", el("div", { class: "stat-value" }, stockValue), {
    href: "/cards",
    sub: "全部启用批量组"
  });
  statArea.append(revenueCard, accountsCard, ordersCard, pendingCard, stockCard);

  // ---- 营收趋势区(FR-007/FR-008):图表由 renderTrend 渲染 ----
  const trendContent = el("div", { class: "trend-content" }, el("p", { class: "muted" }, "加载中…"));
  const trendSection = el(
    "section",
    { class: "card trend-card" },
    el("h2", { class: "card-title" }, "营收趋势分析"),
    el("p", { class: "trend-sub muted" }, "所选时间范围内的销售额走势"),
    trendContent
  );

  const refreshBtn = el("button", { class: "btn btn-secondary btn-sm" }, "↻ 刷新");
  const refreshHint = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  const actionsRow = el(
    "div",
    { class: "ov-toolbar" },
    refreshBtn,
    refreshHint,
    el("span", { class: "ov-toolbar-spacer" }),
    el("a", { class: "btn btn-primary btn-sm", href: "/accounts" }, "＋ 扫码接入账号"),
    el("a", { class: "btn btn-secondary btn-sm", href: "/catalog" }, "⟳ 同步商品"),
    el("a", { class: "btn btn-secondary btn-sm", href: "/orders" }, "⇄ 打开订单中心")
  );

  const quickView = el("div", {});
  const ordersView = el("div", {});
  const issuesView = el("div", {});
  root.append(
    header,
    bannerArea,
    chipsRow,
    statArea,
    trendSection,
    actionsRow,
    el(
      "div",
      { style: "display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px;align-items:start;" },
      el("section", { class: "card" }, el("h2", { class: "card-title" }, "账号状态速览"), quickView),
      el("section", { class: "card" }, el("h2", { class: "card-title" }, "最近待处理"), issuesView)
    ),
    el("section", { class: "card" }, el("h2", { class: "card-title" }, "最近订单"), ordersView)
  );

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

  // ---- 统计渲染:四卡 + 状态指示 + 告警横幅(恢复隔离置顶,停止次之) ----
  const renderBanner = (s: StatsOverview): void => {
    bannerArea.replaceChildren();
    if (s.restore) {
      bannerArea.append(
        el(
          "div",
          { class: "banner banner-warning", role: "alert" },
          el("strong", {}, "恢复隔离中"),
          el("span", {}, `待核对 ${s.restore.unresolved_count} 笔;逐单核对完成后才恢复正常处理`),
          el("a", { href: "/issues" }, "去核对 →")
        )
      );
    }
    if (s.stopping) {
      bannerArea.append(
        el("div", { class: "banner banner-danger", role: "alert" }, el("strong", {}, "服务正在停止"), el("span", {}, "不再接受新任务"))
      );
    }
  };

  const renderStatus = (s: StatsOverview): void => {
    const textEl = statusChip.querySelector(".ov-status-text") as HTMLElement;
    statusChip.classList.remove("state-stopping", "state-restore");
    if (s.stopping) {
      statusChip.classList.add("state-stopping");
      textEl.textContent = "服务正在停止";
    } else if (s.restore) {
      statusChip.classList.add("state-restore");
      textEl.textContent = `恢复隔离中 · 待核对 ${s.restore.unresolved_count} 笔`;
    } else {
      textEl.textContent = "系统正常运行";
    }
  };

  const renderCards = (s: StatsOverview): void => {
    const c = deriveCards(s);
    revValue.textContent = c.revenueText;
    if (c.changePercent === null) {
      revBadge.hidden = true;
    } else {
      revBadge.hidden = false;
      revBadge.textContent = c.changeText;
      revBadge.classList.remove("trend-up", "trend-down", "trend-flat");
      revBadge.classList.add(`trend-${c.changeTrend}`);
    }
    acctValue.textContent = `${c.accountsOnline} / ${c.accountsTotal}`;
    orderValue.textContent = String(c.orderCount);
    pendingValue.textContent = String(c.pending);
    pendingCard.classList.toggle("tone-warning-card", c.pending > 0);
    // 库存余量:缺失显示"—"不闪 0;余量为 0 → warning 强调(复用既有卡片 tone 类)
    if (c.stockAvailable === null) {
      stockValue.textContent = "—";
      stockCard.classList.remove("tone-warning-card");
    } else {
      stockValue.textContent = String(c.stockAvailable);
      stockCard.classList.toggle("tone-warning-card", c.stockAvailable === 0);
    }
    setPendingIssues(c.pending);
  };

  const renderTrend = (s: StatsOverview): void => {
    // 原地替换图表内容,布局不跳动(FR-015);空态语义(FR-008)
    trendContent.replaceChildren();
    const hasData = s.trend.some((p) => p.minor_units > 0);
    if (!hasData) {
      trendContent.append(
        el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "🛒"),
          el("div", {}, "暂无营收数据"),
          el("div", { class: "muted" }, "所选时间范围内暂无订单记录")
        )
      );
      return;
    }
    trendContent.append(
      renderTrendChart(
        s.trend.map((p) => ({
          bucketStart: p.bucket_start,
          minorUnits: p.minor_units,
          orderCount: p.order_count,
          granularity: s.range.granularity
        }))
      )
    );
  };

  const renderStats = (s: StatsOverview): void => {
    renderCards(s);
    renderStatus(s);
    renderBanner(s);
    renderTrend(s);
  };

  const currentQuery = (): string => {
    // 自定义生效时按自定义区间,否则按当前预设(轮询/刷新均保持所选,FR-013)
    const { from, to } = customRange ?? resolvePresetRange(currentPreset, Date.now());
    return `/api/v1/stats/overview?from=${from}&to=${to}`;
  };

  // 加载序号守卫:响应返回时序号非最新则丢弃(Constitution IV「过期响应」;
  // spec Edge Case:切换范围后旧范围数据不得覆盖新范围,FR-013)。
  let statsSeq = 0;
  const loadStats = async (): Promise<void> => {
    const seq = ++statsSeq;
    const query = currentQuery();
    let data: StatsOverview;
    try {
      data = parseStatsOverview(await httpGet(query));
    } catch {
      if (seq !== statsSeq) return; // 已被更新的加载取代
      renderStatsError();
      return;
    }
    if (seq !== statsSeq) return; // 过期响应:不渲染
    renderStats(data);
  };

  /** 查询失败占位:统计卡保留上次值,趋势区提示可重试(FR-008/US2-AC4)。 */
  const renderStatsError = (): void => {
    trendContent.replaceChildren();
    const retry = el("button", { class: "btn btn-secondary btn-sm", type: "button" }, "重试");
    retry.addEventListener("click", () => void loadStats());
    trendContent.append(
      el(
        "div",
        { class: "empty-state" },
        el("div", { class: "empty-ico" }, "⚠️"),
        el("div", {}, "统计暂时不可用"),
        retry
      )
    );
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
        // 下钻订单中心并聚焦该单
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

  // ===== 既有辅助(账号速览排序)已由 ./model 提供 =====
  let stopTicker: (() => void) | null = null;
  const poll = startPoll(loadStats, 30_000);
  loadStats();
  loadQuickView();
  loadOrders();
  loadIssues();
  refreshBtn.addEventListener("click", () => {
    void loadStats();
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
