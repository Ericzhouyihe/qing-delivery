/** 卡密库存页(007 T016):页头/类型筛选+名称搜索/计数横幅/四类型表格/
 *  编辑抽屉(类型字段)/批量导入/引用保护删除。
 *  "算"在 model.ts(纯函数),本文件只装配与调用既有 API(宪章 II);
 *  本地即时过滤不重新请求(006 账号页模式)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPut, httpRequest } from "../../shared/http";
import { errorHint, parseCardPool, parsePage, type CardPoolDto } from "../../shared/contracts";
import { badgeEl, type BadgeSpec } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { confirmDialog } from "../../ui/modal";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { icon } from "../../ui/icons";
import {
  KIND_FILTER_OPTIONS,
  KIND_LABELS,
  KIND_TONES,
  contentCellText,
  filterCardPools,
  type KindFilter
} from "./model";
import { openCardPoolEditor, openImportModal } from "./editor";
import type { PageFactory } from "../../app/page";

export const cardsPage: PageFactory = (root) => {
  let kindFilter: KindFilter = "all";
  let query = "";
  let cache: CardPoolDto[] | null = null;
  let inflight = 0;

  /** 计数横幅:显示 X = 过滤后可见数,Y = 总组数。 */
  const countEl = el("span", { class: "count-banner-count" });
  const banner = el(
    "div",
    { class: "count-banner" },
    countEl,
    el(
      "span",
      { class: "count-banner-hint" },
      "四类卡密组:批量卡密(分状态库存)/ 文本 / 图片 / API;卡密明文永不回显"
    )
  );
  const syncBanner = (): void => {
    const total = cache?.length ?? 0;
    const shown = cache === null ? 0 : filterCardPools(cache, kindFilter, query).length;
    countEl.textContent = `显示 ${shown} / ${total} 组`;
  };

  /** 筛选无匹配轻空态:与总数为 0 的引导空态区分,提供一步清空(006 模式)。 */
  const noMatchEl = (): HTMLElement => {
    const clear = btn("清空筛选", { variant: "secondary" });
    clear.addEventListener("click", () => {
      searchInput.value = "";
      query = "";
      kindSelect.value = "all";
      kindFilter = "all";
      rerenderLocal();
    });
    return el(
      "div",
      { class: "no-match" },
      el("div", { class: "empty-ico" }, "▤"),
      el("div", {}, "无筛选结果"),
      el("div", { class: "muted" }, "当前类型或名称关键词下没有卡密组"),
      clear
    );
  };

  /** 已知错误码集(决定是否拼接后端原始信息;与 contracts ERROR_HINTS 收录范围一致)。 */
  const ERROR_HINTED = new Set([
    "stock_insufficient",
    "referenced_resource",
    "payload_too_large",
    "reply_limit_reached",
    "credential_change_failed"
  ]);

  /** ApiHttpError → 用户可读信息:已知码映射 errorHint 并附后端细节,未知码用原始 message。 */
  const apiErrorMessage = (e: unknown): string => {
    if (e instanceof ApiHttpError) {
      const code = e.body?.code;
      const hint = code !== undefined && ERROR_HINTED.has(code) ? errorHint(code, e.message) : "";
      return hint.length > 0 ? `${hint}(${e.message})` : e.message;
    }
    return e instanceof Error ? e.message : String(e);
  };

  const kindBadgeSpec = (kind: CardPoolDto["kind"]): BadgeSpec => ({
    tone: KIND_TONES[kind],
    label: KIND_LABELS[kind]
  });

  /** 组 ID 一键复制(审计/规则绑定引用用)。 */
  const idCell = (pool: CardPoolDto): HTMLElement => {
    const short = pool.id.length > 14 ? `${pool.id.slice(0, 14)}…` : pool.id;
    const b = el("button", { class: "btn btn-ghost btn-sm mono", type: "button", title: `${pool.id}(点击复制)` }, short);
    b.addEventListener("click", () => {
      void navigator.clipboard.writeText(pool.id).then(
        () => {
          b.textContent = "已复制";
          window.setTimeout(() => (b.textContent = short), 1500);
        },
        () => {
          b.textContent = "复制失败";
          window.setTimeout(() => (b.textContent = short), 1500);
        }
      );
    });
    return b;
  };

  /** 启用开关:立即生效(乐观锁整组更新;内容字段不随开关重发,不会被清空)。 */
  const enabledCell = (pool: CardPoolDto): HTMLElement => {
    const sw = el("input", { type: "checkbox", role: "switch", "aria-label": `启用 ${pool.name}` }) as HTMLInputElement;
    sw.checked = pool.enabled;
    sw.addEventListener("change", async () => {
      const enabling = sw.checked;
      sw.disabled = true;
      try {
        await httpPut(`/api/v1/card-pools/${pool.id}`, {
          name: pool.name,
          kind: pool.kind,
          enabled: enabling,
          delay_seconds: pool.delay_seconds,
          description: pool.description,
          entries: [],
          expected_version: pool.version
        });
        blockRef?.reload();
      } catch (e) {
        sw.checked = !enabling; // 失败回滚视觉状态
        confirmDialog({ title: "操作失败", message: apiErrorMessage(e), confirmLabel: "知道了" });
      } finally {
        sw.disabled = false;
      }
    });
    return sw;
  };

  /** 删除:破坏性确认流;被引用时后端 409 referenced_resource,提示先解除引用。 */
  const openDelete = (pool: CardPoolDto): void => {
    openConfirmFlow({
      title: `删除卡密组「${pool.name}」?`,
      impact: el(
        "div",
        {},
        el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作无法撤销"),
        el("p", { class: "muted", style: "margin:0;" },
          "删除后该组全部卡密条目与配置将被清除。若该组仍被发货规则或规则变体引用,删除将被拒绝(",
          el("span", { class: "mono" }, "referenced_resource"),
          "),需先在规则中解除引用。")
      ),
      requireReason: false,
      requireRiskAck: false,
      confirmLabel: "确认删除",
      danger: true,
      submit: async () => {
        try {
          await httpRequest<void>(`/api/v1/card-pools/${pool.id}`, { method: "DELETE" });
        } catch (e) {
          throw new Error(apiErrorMessage(e));
        }
      },
      onSuccess: () => blockRef?.reload()
    });
  };

  const buildTable = (): HTMLElement | null => {
    if (cache === null || cache.length === 0) return null;
    const items = filterCardPools(cache, kindFilter, query);
    syncBanner();
    if (items.length === 0) return noMatchEl();
    const table = el("table", { class: "data" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "名称"),
        el("th", {}, "组 ID"),
        el("th", {}, "类型"),
        el("th", {}, "内容 / 库存"),
        el("th", {}, "描述"),
        el("th", {}, "启用"),
        el("th", {}, "操作")
      )
    );
    for (const pool of items) {
      const content = contentCellText(pool);
      const contentCell = el(
        "td",
        {},
        el("div", {}, content.main),
        ...(content.detail !== null ? [el("div", { class: "muted", style: "font-size:var(--text-xs);" }, content.detail)] : [])
      );
      const editBtn = el("button", { class: "icon-btn", type: "button", title: "编辑" });
      editBtn.append(icon("edit"));
      editBtn.addEventListener("click", () => {
        openCardPoolEditor(editBtn, pool, { onSaved: () => blockRef?.reload() });
      });
      const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除 ${pool.name}` });
      delBtn.append(icon("trash"));
      delBtn.addEventListener("click", () => openDelete(pool));
      table.append(
        el(
          "tr",
          {},
          el("td", {}, el("strong", {}, pool.name)),
          el("td", {}, idCell(pool)),
          el("td", {}, badgeEl(kindBadgeSpec(pool.kind), pool.kind)),
          contentCell,
          el(
            "td",
            { class: "muted" },
            pool.description.length > 0 ? el("span", { class: "truncate", title: pool.description }, pool.description.slice(0, 24) + (pool.description.length > 24 ? "…" : "")) : "—"
          ),
          el("td", {}, enabledCell(pool)),
          el("td", {}, el("div", { style: "display:flex;gap:4px;" }, editBtn, delBtn))
        )
      );
    }
    return table;
  };

  const load = async (): Promise<HTMLElement | null> => {
    inflight++;
    try {
      const page = parsePage(await httpGet("/api/v1/card-pools"), parseCardPool);
      cache = page.items;
      syncBanner();
      return buildTable();
    } finally {
      inflight--;
    }
  };

  /** 本地即时过滤:不重新请求,仅当表格/无匹配块在场时原位重建。 */
  const rerenderLocal = (): void => {
    syncBanner();
    if (inflight > 0) return;
    const slot = listHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    if (current.classList.contains("data") || current.classList.contains("no-match")) {
      const node = buildTable();
      if (node !== null) slot.replaceChildren(node);
    }
  };

  const emptyState = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "▤"),
      el("div", {}, "暂无卡密配置"),
      el("div", { class: "muted" }, "点击「添加新卡密」创建批量库存/文本/图片/API 组,或用「批量导入」上传表格")
    );

  // ---- 页头 + 工具栏(类型下拉 + 名称搜索居左;主/次按钮居右)----
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "卡密库存"),
    el("p", { class: "page-head-sub" }, "管理四类发货内容:批量卡密按条预留扣减,文本/图片固定发送,API 组实时取卡。")
  );
  const kindSelect = el("select", { "aria-label": "类型筛选" }) as HTMLSelectElement;
  for (const opt of KIND_FILTER_OPTIONS) {
    const o = el("option", { value: opt.value }, opt.label) as HTMLOptionElement;
    kindSelect.append(o);
  }
  kindSelect.addEventListener("change", () => {
    kindFilter = kindSelect.value as KindFilter;
    rerenderLocal();
  });
  const searchInput = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索名称",
    "aria-label": "搜索卡密组名称"
  }) as HTMLInputElement;
  searchInput.addEventListener("input", () => {
    query = searchInput.value;
    rerenderLocal();
  });
  const addBtn = btn("添加新卡密", { variant: "primary" });
  addBtn.prepend(icon("edit"));
  addBtn.classList.add("btn-with-icon");
  const importBtn = btn("批量导入", { variant: "secondary" });
  importBtn.addEventListener("click", () => {
    openImportModal(importBtn, { onImported: () => blockRef?.reload() });
  });
  addBtn.addEventListener("click", () => {
    openCardPoolEditor(addBtn, null, { onSaved: () => blockRef?.reload() });
  });

  const toolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, kindSelect, searchInput),
    el("div", { class: "toolbar-actions" }, importBtn, addBtn)
  );

  root.append(head, toolbar, banner);
  const listHost = el("div", { class: "cards-list-host" });
  root.append(listHost);

  let blockRef: ReturnType<typeof asyncBlock> | null = null;
  const block = asyncBlock(listHost, load, { skeletonLines: 6, empty: emptyState });
  blockRef = block;

  return () => block.dispose();
};
