/** 商品列表页(007 T040):自 /catalog「商品与规则」合并页拆出的商品半区;
 *  商品同步按钮+轮询反馈(catalog/products 复用)、账号筛选/仅看已配置/搜索、
 *  「关联发货规则」按钮跨页跳转 /rules?account={aid}&item={itemId}(原页内跳转的替代)。
 *  契约事实:items 端点无价格字段(001 契约),不虚构价格列;
 *  规则配置态只有 configured/missing/conflict 徽标(无逐商品规则计数),如实呈现。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { badgeEl, itemStatusBadge, ruleStateBadge } from "../../ui/badge";
import { btn, truncate } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import {
  filterItems,
  parseItem,
  wireSyncButton,
  type AccountRef,
  type ProductItem
} from "../catalog/products";
import { rulesDeepLink } from "./model";
import type { PageFactory } from "../../app/page";

async function fetchAccounts(): Promise<AccountRef[]> {
  const raw = await httpGet("/api/v1/accounts");
  const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return items.map((a) => ({
    id: String((a as Record<string, unknown>)["id"] ?? ""),
    displayName: String((a as Record<string, unknown>)["display_name"] ?? "")
  }));
}

async function fetchItems(accountId: string): Promise<ProductItem[]> {
  const raw = await httpGet(`/api/v1/accounts/${accountId}/items?limit=100`);
  const arr = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return arr.map(parseItem);
}

export const itemsPage: PageFactory = (root) => {
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "商品列表"),
    el(
      "p",
      { class: "page-head-sub" },
      "同步在售商品目录;「关联发货规则」直达自动化规则页并定位商品。"
    )
  );

  // ---- 工具栏:账号筛选(兼同步目标)+ 仅看已配置 + 搜索 + 同步(轮询反馈)----
  const accountSel = el("select", { "aria-label": "账号筛选" }) as HTMLSelectElement;
  accountSel.append(el("option", { value: "" }, "全部账号"));
  const configured = el("input", { type: "checkbox" }) as HTMLInputElement;
  const configuredLabel = el("label", { class: "check-row", style: "margin:0;" }, configured, "仅看已配置规则");
  const keyword = el("input", { type: "text", placeholder: "搜索商品标题 / 平台 ID" }) as HTMLInputElement;
  const clearBtn = btn("清空筛选", { variant: "ghost" });
  const syncBtn = btn("⟳ 从平台同步", { variant: "secondary" });
  const syncFeedback = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  const syncHint = el(
    "span",
    { class: "muted", style: "font-size:var(--text-xs);" },
    "同步按账号执行:选择具体账号后可用"
  );

  const countEl = el("span", { class: "count-banner-count" });
  const banner = el(
    "div",
    { class: "count-banner" },
    countEl,
    el("span", { class: "count-banner-hint" }, "规则配置态来自启用规则计数:已配置 / 冲突(多条)/ 未配置")
  );

  const listHost = el("div", { class: "items-list-host" });
  const toolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, accountSel, configuredLabel, keyword, clearBtn),
    el("div", { class: "toolbar-actions" }, syncFeedback, syncBtn)
  );
  root.append(head, toolbar, banner, syncHint, listHost);

  const state: { accounts: AccountRef[]; items: ProductItem[] } = { accounts: [], items: [] };
  let inflight = 0;

  const buildList = (): HTMLElement | null => {
    const filtered = filterItems(state.items, {
      accountId: accountSel.value === "" ? null : accountSel.value,
      configuredOnly: configured.checked,
      keyword: keyword.value
    });
    const total = state.items.length;
    countEl.textContent = `显示 ${filtered.length} / ${total} 件`;
    if (filtered.length === 0) {
      if (total === 0) return null;
      return el(
        "div",
        { class: "no-match" },
        el("div", { class: "empty-ico" }, "▦"),
        el("div", {}, "无筛选结果"),
        el("div", { class: "muted" }, "当前账号/配置态/关键词下没有商品"),
        (() => {
          const c = btn("清空筛选", { variant: "secondary" });
          c.addEventListener("click", () => {
            accountSel.value = "";
            keyword.value = "";
            configured.checked = false;
            rerenderLocal();
            syncSyncTarget();
          });
          return c;
        })()
      );
    }
    const byAccount = new Map(state.accounts.map((a) => [a.id, a.displayName]));
    const table = el("table", { class: "data" });
    table.append(
      el("tr", {},
        el("th", {}, "商品"),
        el("th", {}, "平台 ID"),
        el("th", {}, "账号"),
        el("th", {}, "状态"),
        el("th", {}, "规则"),
        el("th", {}, "操作"))
    );
    for (const it of filtered) {
      const link = el(
        "a",
        { class: "btn btn-primary btn-sm", href: rulesDeepLink(it.accountId, it.id) },
        "关联发货规则"
      );
      link.title = "跳转自动化规则页,预选账号并为该商品预填新建规则";
      table.append(
        el("tr", {},
          el("td", {}, el("span", { class: "thumb" }), truncate(it.title, 20)),
          el("td", { class: "mono" }, it.platformItemId),
          el("td", { class: "muted" }, byAccount.get(it.accountId) ?? it.accountId),
          el("td", {}, badgeEl(itemStatusBadge(it.status), it.status)),
          el("td", {}, badgeEl(ruleStateBadge(it.ruleState), it.ruleState)),
          el("td", {}, link))
      );
    }
    return table;
  };

  const rerenderLocal = (): void => {
    if (inflight > 0) return;
    const slot = listHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    if (current.classList.contains("data") || current.classList.contains("no-match")) {
      const node = buildList();
      if (node !== null) slot.replaceChildren(node);
    }
  };

  /** 同步目标 = 筛选框所选账号(全部账号时禁用,不静默取首个;mock 档同步经 dev 场景)。 */
  const syncSyncTarget = (): void => {
    const target = accountSel.value;
    // 重新接线:wireSyncButton 为一次性 addEventListener,克隆替换以清旧监听
    const fresh = btn("⟳ 从平台同步", { variant: "secondary" });
    if (target === "") {
      fresh.disabled = true;
      fresh.title = "请先在筛选中选择具体账号(同步按账号创建任务)";
      syncHint.textContent = "同步按账号执行:选择具体账号后可用";
    } else {
      fresh.title = "";
      wireSyncButton(fresh, target, syncFeedback, () => block.reload());
      syncHint.textContent = `同步目标:${state.accounts.find((a) => a.id === target)?.displayName ?? target}`;
    }
    syncBtnRef.replaceWith(fresh);
    syncBtnRef = fresh;
  };
  let syncBtnRef = syncBtn;
  // 初次接线占位(未选账号禁用)
  syncSyncTarget();

  accountSel.addEventListener("change", () => {
    rerenderLocal();
    syncSyncTarget();
  });
  keyword.addEventListener("input", rerenderLocal);
  configured.addEventListener("change", rerenderLocal);
  clearBtn.addEventListener("click", () => {
    accountSel.value = "";
    keyword.value = "";
    configured.checked = false;
    rerenderLocal();
    syncSyncTarget();
  });

  const load = async (): Promise<HTMLElement | null> => {
    inflight++;
    try {
      state.accounts = await fetchAccounts();
      state.items = [];
      for (const a of state.accounts) {
        // 逐账号拉取(账号数十以内;失败即整页错误块,可重试)
        state.items = state.items.concat(await fetchItems(a.id));
      }
      accountSel.replaceChildren(el("option", { value: "" }, "全部账号"));
      for (const a of state.accounts) {
        accountSel.append(el("option", { value: a.id }, a.displayName));
      }
      const node = buildList();
      syncSyncTarget();
      return node;
    } finally {
      inflight--;
    }
  };

  const block = asyncBlock(listHost, load, {
    skeletonLines: 6,
    empty: () =>
      el(
        "div",
        { class: "empty-state" },
        el("div", { class: "empty-ico" }, "▦"),
        el("div", {}, "暂无商品记录"),
        el(
          "div",
          { class: "muted" },
          state.accounts.length === 0
            ? "请先在「账号」页接入闲鱼账号,再同步商品"
            : "在筛选中选择具体账号后点击「从平台同步」;完成后商品在此展示"
        )
      )
  });

  return () => block.dispose();
};
