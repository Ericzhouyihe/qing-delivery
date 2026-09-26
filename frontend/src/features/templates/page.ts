/** 发货模板页(007 T026):页头/搜索过滤+计数横幅/两列模板卡片网格/
 *  编辑抽屉(变量指南)/引用保护删除(409 referenced_resource → errorHint)。
 *  "算"在 model.ts(纯函数),本文件只装配与调用既有 API(宪章 II);
 *  本地即时过滤不重新请求(006 账号页模式)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpRequest } from "../../shared/http";
import { errorHint, parsePage } from "../../shared/contracts";
import { badgeEl, type BadgeSpec } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { icon } from "../../ui/icons";
import {
  extractKeys,
  filterTemplates,
  keysLine,
  messagePreview,
  parseTemplate,
  type TemplateDto
} from "./model";
import { openTemplateEditor } from "./editor";
import type { PageFactory } from "../../app/page";

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

const enabledBadge = (enabled: boolean): BadgeSpec =>
  enabled ? { tone: "normal", label: "已启用" } : { tone: "neutral", label: "已停用" };

export const templatesPage: PageFactory = (root) => {
  let query = "";
  let cache: TemplateDto[] | null = null;
  let inflight = 0;

  /** 计数横幅:显示 X = 过滤后可见数,Y = 总模板数。 */
  const countEl = el("span", { class: "count-banner-count" });
  const banner = el(
    "div",
    { class: "count-banner" },
    countEl,
    el(
      "span",
      { class: "count-banner-hint" },
      "多消息模板按顺序逐条发送;{{变量}} 发货时按订单渲染,卡密内容只经加密快照"
    )
  );
  const syncBanner = (): void => {
    const total = cache?.length ?? 0;
    const shown = cache === null ? 0 : filterTemplates(cache, query).length;
    countEl.textContent = `显示 ${shown} / ${total} 个`;
  };

  /** 筛选无匹配轻空态:与总数为 0 的引导空态区分,提供一步清空(006 模式)。 */
  const noMatchEl = (): HTMLElement => {
    const clear = btn("清空筛选", { variant: "secondary" });
    clear.addEventListener("click", () => {
      searchInput.value = "";
      query = "";
      rerenderLocal();
    });
    return el(
      "div",
      { class: "no-match" },
      el("div", { class: "empty-ico" }, "▣"),
      el("div", {}, "无筛选结果"),
      el("div", { class: "muted" }, "当前名称关键词下没有发货模板"),
      clear
    );
  };

  /** 删除:破坏性确认流;被规则/变体引用时后端 409 referenced_resource(errorHint 提示先解除引用)。 */
  const openDelete = (t: TemplateDto): void => {
    openConfirmFlow({
      title: `删除模板「${t.name}」?`,
      impact: el(
        "div",
        {},
        el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作无法撤销"),
        el(
          "p",
          { class: "muted", style: "margin:0;" },
          `删除后该模板的 ${t.messages.length} 条消息配置将被清除。若仍被发货规则或规则变体引用,删除将被拒绝(`,
          el("span", { class: "mono" }, "referenced_resource"),
          "),需先在规则中解除引用。"
        )
      ),
      requireReason: false,
      requireRiskAck: false,
      confirmLabel: "确认删除",
      danger: true,
      submit: async () => {
        try {
          await httpRequest<void>(`/api/v1/delivery-templates/${t.id}`, { method: "DELETE" });
        } catch (e) {
          throw new Error(apiErrorMessage(e));
        }
      },
      onSuccess: () => blockRef?.reload()
    });
  };

  /** 单个模板卡:名称/消息数与启用徽标/逐条消息预览(截断)/变量行/被引用提示/操作。 */
  const templateCard = (t: TemplateDto): HTMLElement => {
    const editBtn = el("button", { class: "icon-btn", type: "button", title: `编辑 ${t.name}` });
    editBtn.append(icon("edit"));
    editBtn.addEventListener("click", () => {
      openTemplateEditor(editBtn, t, { onSaved: () => blockRef?.reload() });
    });
    const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除 ${t.name}` });
    delBtn.append(icon("trash"));
    delBtn.addEventListener("click", () => openDelete(t));

    const keysLineText = keysLine(extractKeys(t.messages));
    return el(
      "article",
      { class: "tpl-card" },
      el(
        "div",
        { class: "tpl-card-head" },
        el("strong", { class: "tpl-card-name", title: t.name }, t.name),
        el("div", { style: "display:flex;gap:4px;flex:none;" }, editBtn, delBtn)
      ),
      el(
        "div",
        { class: "tpl-badge-pair" },
        badgeEl({ tone: "neutral", label: `${t.messages.length} 条消息` }, String(t.messages.length)),
        badgeEl(enabledBadge(t.enabled), String(t.enabled))
      ),
      el(
        "ol",
        { class: "tpl-msg-list" },
        ...t.messages.map((m) =>
          el("li", { class: "tpl-msg-preview", title: m }, messagePreview(m))
        )
      ),
      keysLineText !== null
        ? el("div", { class: "tpl-vars" }, "变量:", el("span", { class: "mono" }, keysLineText))
        : el("div", { class: "tpl-vars" }, "变量:未使用"),
      t.used_by_rules > 0
        ? el("div", { class: "tpl-card-used" }, `被 ${t.used_by_rules} 条规则引用`)
        : null
    );
  };

  const buildGrid = (): HTMLElement | null => {
    if (cache === null || cache.length === 0) return null;
    const items = filterTemplates(cache, query);
    syncBanner();
    if (items.length === 0) return noMatchEl();
    return el("div", { class: "tpl-grid" }, ...items.map(templateCard));
  };

  const load = async (): Promise<HTMLElement | null> => {
    inflight++;
    try {
      const page = parsePage(await httpGet("/api/v1/delivery-templates"), parseTemplate);
      cache = page.items;
      syncBanner();
      return buildGrid();
    } finally {
      inflight--;
    }
  };

  /** 本地即时过滤:不重新请求,仅当网格/无匹配块在场时原位重建。 */
  const rerenderLocal = (): void => {
    syncBanner();
    if (inflight > 0) return;
    const slot = listHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    if (current.classList.contains("tpl-grid") || current.classList.contains("no-match")) {
      const node = buildGrid();
      if (node !== null) slot.replaceChildren(node);
    }
  };

  const emptyState = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "▣"),
      el("div", {}, "还没有发货模板"),
      el(
        "div",
        { class: "muted" },
        "点击「新建模板」把要发的内容做成可复用的多消息模板,支持按订单渲染的变量占位符"
      )
    );

  // ---- 页头 + 工具栏(名称搜索居左;主按钮居右)----
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "发货模板"),
    el("p", { class: "page-head-sub" }, "把要发的内容做成可复用的多消息模板,变量按订单渲染。")
  );
  const searchInput = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索名称",
    "aria-label": "搜索发货模板名称"
  }) as HTMLInputElement;
  searchInput.addEventListener("input", () => {
    query = searchInput.value;
    rerenderLocal();
  });
  const addBtn = btn("新建模板", { variant: "primary" });
  addBtn.addEventListener("click", () => {
    openTemplateEditor(addBtn, null, { onSaved: () => blockRef?.reload() });
  });

  const toolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, searchInput),
    el("div", { class: "toolbar-actions" }, addBtn)
  );

  root.append(head, toolbar, banner);
  const listHost = el("div", { class: "templates-list-host" });
  root.append(listHost);

  let blockRef: ReturnType<typeof asyncBlock> | null = null;
  const block = asyncBlock(listHost, load, { skeletonLines: 6, empty: emptyState });
  blockRef = block;

  return () => block.dispose();
};
