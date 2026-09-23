/** 订单中心页(US5/T039):组合筛选 + 游标分页(翻页保留筛选)+ 详情抽屉。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord, parsePage } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { renderOrdersTable } from "./table";
import { openOrderDetail } from "./detail";
import {
  applyContentStateFilter,
  EMPTY_FILTERS,
  filtersToQuery,
  parseOrderRow,
  type OrderFilters
} from "./model";
import type { PageFactory } from "../../app/page";

const CONTENT_STATES = [
  ["", "全部内容状态"],
  ["awaiting_verification", "待核验"],
  ["ready", "待发送"],
  ["sending", "发送中"],
  ["delivered", "内容已交付"],
  ["definitely_not_sent", "确定未发送"],
  ["unknown", "结果未知"],
  ["needs_review", "等待人工"],
  ["terminated", "已终止"]
] as const;

const PLATFORM_STATES = [
  ["", "全部平台状态"],
  ["pending_ship", "待发货"],
  ["shipped", "已发货"],
  ["completed", "已完成"],
  ["refunding", "退款中"],
  ["refunded", "已退款"],
  ["canceled", "已取消"]
] as const;

export const ordersPage: PageFactory = (root) => {
  let filters: OrderFilters = { ...EMPTY_FILTERS };
  const cursorStack: Array<string | null> = [null];
  let pageNumber = 1;

  const filterBar = el("div", { class: "filter-bar" });
  const tableBox = el("div", {});
  const pager = el("div", { class: "pager" });

  const accountNames = new Map<string, string>();
  const itemTitles = new Map<string, string>();

  const buildBar = (reload: () => void): void => {
    const orderIdInput = el("input", { type: "text", placeholder: "订单号搜索", value: filters.orderId }) as HTMLInputElement;
    const accountSel = el("select", {}) as HTMLSelectElement;
    const contentSel = el("select", {}) as HTMLSelectElement;
    const platformSel = el("select", {}) as HTMLSelectElement;
    const fromInput = el("input", { type: "date", value: filters.from }) as HTMLInputElement;
    const toInput = el("input", { type: "date", value: filters.to }) as HTMLInputElement;
    const clearBtn = btn("清空筛选", { variant: "ghost" });

    accountSel.append(el("option", { value: "" }, "全部账号"));
    for (const [id, name] of accountNames) {
      accountSel.append(el("option", { value: id }, name));
    }
    accountSel.value = filters.accountId;
    for (const [value, label] of CONTENT_STATES) {
      contentSel.append(el("option", { value }, label));
    }
    contentSel.value = filters.contentState;
    for (const [value, label] of PLATFORM_STATES) {
      platformSel.append(el("option", { value }, label));
    }
    platformSel.value = filters.platformState;

    orderIdInput.addEventListener("change", () => {
      filters.orderId = orderIdInput.value;
      reload();
    });
    accountSel.addEventListener("change", () => {
      filters.accountId = accountSel.value;
      reload();
    });
    contentSel.addEventListener("change", () => {
      filters.contentState = contentSel.value;
      reload();
    });
    platformSel.addEventListener("change", () => {
      filters.platformState = platformSel.value;
      reload();
    });
    fromInput.addEventListener("change", () => {
      filters.from = fromInput.value;
      reload();
    });
    toInput.addEventListener("change", () => {
      filters.to = toInput.value;
      reload();
    });
    clearBtn.addEventListener("click", () => {
      filters = { ...EMPTY_FILTERS };
      cursorStack.length = 1;
      pageNumber = 1;
      buildBar(reload);
      reload();
    });

    filterBar.replaceChildren(
      orderIdInput,
      accountSel,
      contentSel,
      platformSel,
      el("span", { class: "muted" }, "付款时间"),
      fromInput,
      el("span", { class: "muted" }, "至"),
      toInput,
      clearBtn
    );
  };

  const block = asyncBlock(
    root,
    async () => {
      // 账号名映射(下拉 + 表格展示共用)
      try {
        const accRaw = await httpGet("/api/v1/accounts");
        const accItems = ((accRaw as Record<string, unknown>)["items"] ?? []) as unknown[];
        accountNames.clear();
        for (const a of accItems) {
          if (isRecord(a)) accountNames.set(String(a["id"]), String(a["display_name"] ?? a["id"]));
        }
      } catch {
        // 账号映射失败不阻塞订单列表
      }

      const cursor = cursorStack[cursorStack.length - 1] ?? null;
      const query = filtersToQuery(filters) + (cursor === null ? "" : `&cursor=${encodeURIComponent(cursor)}`);
      const raw = await httpGet(query);
      const page = parsePage(raw, parseOrderRow);
      const rows = applyContentStateFilter(page.items, filters.contentState);
      if (rows.length === 0 && page.items.length === 0) {
        pager.replaceChildren();
        return el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "🧾"),
          el("div", {}, "没有符合条件的订单"),
          el("div", { class: "muted" }, "调整筛选条件,或等待新付款订单")
        );
      }

      const reload = (): void => {
        cursorStack.length = 1;
        pageNumber = 1;
        block.reload();
      };
      buildBar(reload);

      const prevBtn = btn("← 上一页", { variant: "secondary" });
      const nextBtn = btn("下一页 →", { variant: "secondary" });
      prevBtn.disabled = cursorStack.length <= 1;
      nextBtn.disabled = page.next_cursor === null;
      prevBtn.addEventListener("click", () => {
        if (cursorStack.length > 1) {
          cursorStack.pop();
          pageNumber -= 1;
          block.reload();
        }
      });
      nextBtn.addEventListener("click", () => {
        if (page.next_cursor !== null) {
          cursorStack.push(page.next_cursor);
          pageNumber += 1;
          block.reload();
        }
      });
      pager.replaceChildren(prevBtn, el("span", {}, `第 ${pageNumber} 页`), nextBtn);

      const table = renderOrdersTable(rows, accountNames, itemTitles, (row) => {
        openOrderDetail(row, () => block.reload());
      });
      return el("div", {}, filterBar, table, pager);
    },
    { skeletonLines: 8 }
  );

  // 概览下钻:聚焦指定订单(US2-2/SC-203)
  const focus = new URLSearchParams(location.search).get("focus");
  if (focus !== null) {
    history.replaceState(null, "", "/orders");
    void (async () => {
      try {
        const raw = await httpGet(`/api/v1/orders?limit=50`);
        const page = parsePage(raw, parseOrderRow);
        const row = page.items.find((o) => o.id === focus);
        if (row !== undefined) {
          openOrderDetail(row, () => block.reload());
        }
      } catch {
        // 聚焦失败不阻塞列表
      }
    })();
  }

  return () => block.dispose();
};
