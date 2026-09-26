/** 订单中心页(US5/T039):组合筛选 + 游标分页(翻页保留筛选)+ 详情抽屉;
 *  US6/T071 增一键同步(order_sync 任务轮询反馈 + 中途取消)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord, parsePage } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { confirmDialog } from "../../ui/modal";
import { renderOrdersTable } from "./table";
import { openOrderDetail } from "./detail";
import {
  applyContentStateFilter,
  EMPTY_FILTERS,
  filtersToQuery,
  parseOrderRow,
  parseSyncReports,
  syncFailedDetail,
  syncReportLine,
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

  // 一键同步工具栏(US6/T071):与筛选栏同级;置于异步块外,列表重载不打断任务反馈
  const syncBar = el("div", { class: "filter-bar" });
  const syncBtn = btn("一键同步订单", { variant: "primary" });
  const syncCancelBtn = btn("取消任务", { variant: "danger" });
  const syncFeedback = el("div", { class: "muted", style: "font-size:var(--text-sm);" });
  syncCancelBtn.style.display = "none";
  syncBar.append(syncBtn, syncCancelBtn, syncFeedback);
  root.append(syncBar);

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

  // 一键同步接线(US6/T071):确认 → POST /orders/syncs → 轮询任务至终态 →
  // 逐账号摘要(失败明细悬浮)+ 完成后刷新列表;cancel_allowed 时可中途取消。
  let syncStopped = false;
  let syncJobId: string | null = null;
  let syncJobVersion = 0;

  const reloadOrders = (): void => {
    cursorStack.length = 1;
    pageNumber = 1;
    block.reload();
  };

  const sleep = (ms: number): Promise<void> => new Promise((resolve) => { window.setTimeout(resolve, ms); });

  const syncAccountRows = (resultRef: string | null): HTMLElement[] =>
    parseSyncReports(resultRef).map((r) => {
      const failed = syncFailedDetail(r);
      const attrs: Record<string, string> = { class: "muted", style: "font-size:var(--text-xs);" };
      if (failed !== "") attrs["title"] = failed;
      return el("div", attrs, syncReportLine(r, accountNames.get(r.accountId) ?? r.accountId));
    });

  const resetSyncUi = (): void => {
    syncBtn.disabled = false;
    syncBtn.textContent = "一键同步订单";
    syncCancelBtn.style.display = "none";
    syncCancelBtn.disabled = false;
    syncJobId = null;
  };

  const finishSync = (tone: string, headline: string, resultRef: string | null): void => {
    syncCancelBtn.style.display = "none";
    syncFeedback.className = tone;
    const rows = syncAccountRows(resultRef);
    syncFeedback.replaceChildren(
      el("div", {}, rows.length > 0 ? `${headline}(${rows.length} 个账号)` : headline),
      ...rows
    );
    resetSyncUi();
    reloadOrders();
  };

  const pollSyncJob = async (jobId: string): Promise<void> => {
    for (let i = 0; i < 150; i++) {
      await sleep(2_000);
      if (syncStopped) return;
      let j: Record<string, unknown>;
      try {
        j = (await httpGet(`/api/v1/jobs/${jobId}`)) as Record<string, unknown>;
      } catch {
        continue; // 单次查询失败不终止轮询
      }
      const summary = isRecord(j["summary"]) ? (j["summary"] as Record<string, unknown>) : {};
      const state = String(summary["state"] ?? "running");
      if (typeof summary["version"] === "number") syncJobVersion = summary["version"];
      const resultRef = typeof summary["result_ref"] === "string" ? summary["result_ref"] : null;
      const safeError = typeof summary["safe_error"] === "string" ? summary["safe_error"] : null;
      if (state === "succeeded") {
        finishSync("tone-normal", "同步完成", resultRef);
        return;
      }
      if (state === "cancelled") {
        // 取消意图在账号边界兑现,任务仍写回已处理账号的结果(服务端 T068)
        finishSync("tone-warning", "已取消,已处理部分保留", resultRef);
        return;
      }
      if (state === "failed" || state === "needs_review") {
        finishSync("error-text", `同步失败:${safeError ?? `任务异常结束(${state})`}`, resultRef);
        return;
      }
      // 进行中/已请求取消:展示阶段;cancel_allowed 时露出取消按钮
      syncFeedback.className = "muted";
      const stage = String(j["stage"] ?? "");
      const note =
        state === "cancel_requested"
          ? "已请求取消,等待任务在账号边界停下…"
          : `同步中…${stage === "" || stage === "null" ? "" : `(阶段:${stage})`}`;
      syncFeedback.replaceChildren(el("div", {}, note));
      if (j["cancel_allowed"] === true) syncCancelBtn.style.display = "";
    }
    syncFeedback.className = "muted";
    syncFeedback.replaceChildren(el("div", {}, "同步仍在进行(超过界面跟踪时长);稍后刷新页面查看订单。"));
    resetSyncUi();
    reloadOrders();
  };

  const startSync = async (): Promise<void> => {
    syncBtn.disabled = true;
    syncBtn.textContent = "同步中…";
    syncCancelBtn.disabled = false;
    syncFeedback.className = "muted";
    syncFeedback.replaceChildren(el("div", {}, "已发起同步,正在按账号对账(窗口默认 7 天)…"));
    try {
      const res = (await httpPost("/api/v1/orders/syncs", {})) as Record<string, unknown>;
      const job = isRecord(res["job"]) ? (res["job"] as Record<string, unknown>) : {};
      syncJobId = typeof job["id"] === "string" ? job["id"] : "";
      if (syncJobId === "") throw new Error("服务未返回任务句柄");
      await pollSyncJob(syncJobId);
    } catch (e) {
      syncFeedback.className = "error-text";
      syncFeedback.replaceChildren(el("div", {}, `同步不可用:${e instanceof Error ? e.message : String(e)}`));
      resetSyncUi();
    }
  };

  syncBtn.addEventListener("click", () => {
    confirmDialog({
      title: "一键同步订单",
      message:
        "将按账号逐个与平台全量对账:补建缺失订单、恢复停滞交付并修正状态。同步窗口为最近 7 天;离线账号自动跳过并标注。任务进行中可随时取消,已处理账号的结果保留。",
      confirmLabel: "开始同步",
      onConfirm: () => {
        void startSync();
      }
    });
  });

  syncCancelBtn.addEventListener("click", async () => {
    if (syncJobId === null) return;
    syncCancelBtn.disabled = true;
    try {
      await httpPost(`/api/v1/jobs/${syncJobId}/cancel`, { expected_version: syncJobVersion, reason: "用户取消" });
      syncFeedback.replaceChildren(el("div", {}, "已请求取消,等待任务在账号边界停下…"));
    } catch (e) {
      syncCancelBtn.disabled = false;
      syncFeedback.replaceChildren(
        el("div", { class: "error-text" }, `取消失败:${e instanceof Error ? e.message : String(e)}`)
      );
    }
  });

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

  return () => {
    syncStopped = true; // 页面切换终止任务轮询(宪章 III 资源取向)
    block.dispose();
  };
};
