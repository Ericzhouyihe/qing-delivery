/** 自动化规则页(007 T040):三页签(交易自动化/关键词回复/账号默认回复),
 *  顶部账号选择器(未选禁用主按钮)+ 刷新;/rules?account=&item= 深链预选账号
 *  并在新建规则时预填商品(商品列表页「关联发货规则」跨页跳转入口)。
 *  "算"在 model.ts(纯函数);本页装配 + 既有 API;本地即时过滤不重新请求(006 模式)。
 *  契约事实:自动化规则无删除端点(仅创建/更新),规则卡不提供删除入口(SC-008 零虚标)。 */
import { el } from "../../shared/dom";
import { ApiHttpError, httpGet, httpPut, httpRequest } from "../../shared/http";
import { parsePage, parseCardPool, errorHint } from "../../shared/contracts";
import { badgeEl } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock, type AsyncBlockController } from "../../ui/states";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { confirmDialog } from "../../ui/modal";
import { icon } from "../../ui/icons";
import { parseItem, type AccountRef, type ProductItem } from "../catalog/products";
import { parseTemplate } from "../templates/model";
import type { RuleFormRefOption } from "../catalog/rule-editor";
import { openRuleEditor } from "./editor";
import { openDefaultReplyEditor, openReplyRuleEditor } from "./replies";
import {
  actionSummary,
  buildRefIndex,
  filterReplyRules,
  filterRules,
  parseDefaultReply,
  parseReplyRule,
  parseRuleList,
  rebuildPayloadForToggle,
  replyKindBadge,
  ruleWarnings,
  scopeLabel,
  triggerBadge,
  type DefaultReplyDto,
  type ReplyRuleDto,
  type RuleDto,
  type RuleRefIndex
} from "./model";
import type { PageFactory } from "../../app/page";

/** 已知错误码集(决定是否拼接后端原始信息;与 contracts ERROR_HINTS 收录范围一致)。 */
const ERROR_HINTED = new Set([
  "stock_insufficient",
  "referenced_resource",
  "payload_too_large",
  "reply_limit_reached",
  "credential_change_failed"
]);

const apiErrorMessage = (e: unknown): string => {
  if (e instanceof ApiHttpError) {
    const code = e.body?.code;
    const hint = code !== undefined && ERROR_HINTED.has(code) ? errorHint(code, e.message) : "";
    return hint.length > 0 ? `${hint}(${e.message})` : e.message;
  }
  return e instanceof Error ? e.message : String(e);
};

type TabKey = "trade" | "keyword" | "default";

const TABS: ReadonlyArray<{ key: TabKey; label: string }> = [
  { key: "trade", label: "交易自动化" },
  { key: "keyword", label: "关键词回复" },
  { key: "default", label: "账号默认回复" }
];

async function fetchAccounts(): Promise<AccountRef[]> {
  const raw = await httpGet("/api/v1/accounts");
  const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return items.map((a) => ({
    id: String((a as Record<string, unknown>)["id"] ?? ""),
    displayName: String((a as Record<string, unknown>)["display_name"] ?? "")
  }));
}

export const rulesPage: PageFactory = (root) => {
  // ---- 深链:/rules?account=&item=(商品列表页「关联发货规则」跳入)----
  const params = new URLSearchParams(location.search);
  let accountId = params.get("account") ?? "";
  let pendingItem: string | null = params.get("item") ?? null;
  let tab: TabKey = "trade";
  let accounts: AccountRef[] = [];
  const state: {
    items: ProductItem[];
    pools: RuleFormRefOption[];
    templates: RuleFormRefOption[];
    refs: RuleRefIndex;
    rules: RuleDto[];
    triggerCounts: Record<string, number>;
    replyRules: ReplyRuleDto[];
    defaults: DefaultReplyDto[];
  } = {
    items: [],
    pools: [],
    templates: [],
    refs: buildRefIndex({ pools: [], templates: [], items: [] }),
    rules: [],
    triggerCounts: {},
    replyRules: [],
    defaults: []
  };
  // 本地筛选(交易自动化页签)
  const tradeFilter = { trigger: "", status: "all" as "all" | "enabled" | "disabled", query: "" };
  let keywordQuery = "";

  const syncUrl = (): void => {
    const sp = new URLSearchParams();
    if (accountId !== "") {
      sp.set("account", accountId);
      if (pendingItem !== null && pendingItem !== "") sp.set("item", pendingItem);
    }
    const q = sp.toString();
    history.replaceState(null, "", q === "" ? "/rules" : `/rules?${q}`);
  };

  // ---- 页头 ----
  const head = el(
    "div",
    { class: "page-head" },
    el("h1", { class: "page-head-title" }, "自动化规则"),
    el(
      "p",
      { class: "page-head-sub" },
      "付款自动发货 / 超时求评 / 关键词回复 / 账号默认回复;同范围同触发仅最高优先级执行。"
    )
  );

  // ---- 账号选择器 + 刷新(未选账号禁用页签一/二的主按钮)----
  const accountSel = el("select", { "aria-label": "选择账号" }) as HTMLSelectElement;
  accountSel.append(el("option", { value: "" }, "选择账号…"));
  const refreshBtn = el("button", { class: "btn btn-secondary btn-sm" }, "↻ 刷新");
  const accountHint = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  const accountBar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, accountSel, refreshBtn, accountHint),
    el("span", {})
  );
  accountSel.addEventListener("change", () => {
    accountId = accountSel.value;
    if (pendingItem !== null) pendingItem = null; // 换账号后原商品预填不再适用
    syncUrl();
    syncPendingHint();
    reloadActive();
  });
  refreshBtn.addEventListener("click", () => reloadActive());

  // ---- 三页签(segmented/tab 风格,参照 overview 时间范围 tablist)----
  const tabsRow = el("div", { class: "range-chips", role: "tablist", "aria-label": "规则类型页签" });
  const tabButtons = new Map<TabKey, HTMLButtonElement>();
  for (const t of TABS) {
    const b = el("button", { class: "chip", type: "button", role: "tab" }, t.label) as HTMLButtonElement;
    b.addEventListener("click", () => {
      if (tab === t.key) return;
      tab = t.key;
      syncTabs();
      reloadActive();
    });
    tabButtons.set(t.key, b);
    tabsRow.append(b);
  }
  const syncTabs = (): void => {
    for (const [key, b] of tabButtons) {
      const active = key === tab;
      b.classList.toggle("active", active);
      b.setAttribute("aria-selected", active ? "true" : "false");
    }
    tradePanel.hidden = tab !== "trade";
    keywordPanel.hidden = tab !== "keyword";
    defaultPanel.hidden = tab !== "default";
  };

  // ---- 页签一:交易自动化 ----
  const addRuleBtn = btn("新建规则", { variant: "primary" });
  addRuleBtn.prepend(icon("edit"));
  addRuleBtn.classList.add("btn-with-icon");
  const triggerSel = el("select", { "aria-label": "触发类型筛选" }) as HTMLSelectElement;
  triggerSel.append(
    el("option", { value: "" }, "全部触发类型"),
    el("option", { value: "order_paid" }, "付款后自动发货"),
    el("option", { value: "review_missing_timeout" }, "超时未评价求评价")
  );
  const statusSel = el("select", { "aria-label": "状态筛选" }) as HTMLSelectElement;
  statusSel.append(
    el("option", { value: "all" }, "全部状态"),
    el("option", { value: "enabled" }, "已启用"),
    el("option", { value: "disabled" }, "已停用")
  );
  const tradeSearch = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索商品 / 规格键 / 规则 ID",
    "aria-label": "搜索规则"
  }) as HTMLInputElement;
  const pendingHint = el("div", { class: "count-banner", style: "display:none;" }, "");
  const tradeCount = el("span", { class: "count-banner-count" });
  const tradeBanner = el(
    "div",
    { class: "count-banner" },
    tradeCount,
    el("span", { class: "count-banner-hint" }, "优先级数字越小越高;账号级规则须确认「适用于全部商品」后才执行")
  );
  const tradeToolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, triggerSel, statusSel, tradeSearch),
    el("div", { class: "toolbar-actions" }, addRuleBtn)
  );
  const tradeHost = el("div", { class: "rules-list-host" });
  const tradePanel = el("div", {}, tradeToolbar, pendingHint, tradeBanner, tradeHost);

  // ---- 页签二:关键词回复 ----
  const addReplyBtn = btn("新建关键词回复", { variant: "primary" });
  addReplyBtn.prepend(icon("edit"));
  addReplyBtn.classList.add("btn-with-icon");
  const keywordSearch = el("input", {
    class: "search-input",
    type: "search",
    placeholder: "搜索关键词 / 商品",
    "aria-label": "搜索关键词回复"
  }) as HTMLInputElement;
  const keywordToolbar = el(
    "div",
    { class: "account-toolbar" },
    el("div", { class: "filter-bar", style: "margin-bottom:0;" }, keywordSearch),
    el("div", { class: "toolbar-actions" }, addReplyBtn)
  );
  const keywordHost = el("div", { class: "rules-list-host" });
  const keywordPanel = el("div", {}, keywordToolbar, keywordHost);

  // ---- 页签三:账号默认回复(每账号一张卡)----
  const defaultListHost = el("div", { class: "rules-list-host" });
  const defaultPanel = el(
    "div",
    {},
    el(
      "div",
      { class: "count-banner" },
      el("span", { class: "count-banner-count" }, "兜底回复"),
      el(
        "span",
        { class: "count-banner-hint" },
        "买家普通消息未命中关键词(且未开启 AI)时回复;只回复一次按买家记;绝不触发发货"
      )
    ),
    defaultListHost
  );

  root.append(head, accountBar, tabsRow, tradePanel, keywordPanel, defaultPanel);
  syncTabs();

  // ---- 异步块(懒建;每页签一个)----
  const blocks = new Map<TabKey, AsyncBlockController>();
  const ensureBlock = (key: TabKey, host: HTMLElement, load: () => Promise<HTMLElement | null>): AsyncBlockController => {
    const existing = blocks.get(key);
    if (existing !== undefined) return existing;
    const block = asyncBlock(host, load, {
      skeletonLines: 6,
      empty: () =>
        el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "⚡"),
          el("div", {}, key === "trade" ? "该账号暂无自动化规则" : key === "keyword" ? "该账号暂无关键词回复" : "暂无账号"),
          el("div", { class: "muted" }, key === "trade" ? "点击「新建规则」配置付款发货或超时求评" : key === "keyword" ? "点击「新建关键词回复」配置包含匹配自动回复" : "请先在「账号」页接入闲鱼账号")
        )
    });
    blocks.set(key, block);
    return block;
  };

  const reloadActive = (): void => {
    if (tab === "trade") {
      if (accountId === "") {
        tradeHost.replaceChildren(
          el(
            "div",
            { class: "empty-state" },
            el("div", { class: "empty-ico" }, "◉"),
            el("div", {}, "请先选择账号"),
            el("div", { class: "muted" }, "交易自动化规则按账号管理;选择账号后展示规则卡列表")
          )
        );
        tradeCount.textContent = "";
        tradeBanner.style.display = "none";
        return;
      }
      tradeBanner.style.display = "";
      ensureBlock("trade", tradeHost, loadTrade).reload();
    } else if (tab === "keyword") {
      if (accountId === "") {
        keywordHost.replaceChildren(
          el(
            "div",
            { class: "empty-state" },
            el("div", { class: "empty-ico" }, "◉"),
            el("div", {}, "请先选择账号"),
            el("div", { class: "muted" }, "关键词回复按账号管理;选择账号后展示列表")
          )
        );
        return;
      }
      ensureBlock("keyword", keywordHost, loadKeyword).reload();
    } else {
      ensureBlock("default", defaultListHost, loadDefaults).reload();
    }
    syncMainButtons();
  };

  const syncMainButtons = (): void => {
    if (accountId === "") {
      addRuleBtn.disabled = true;
      addRuleBtn.title = "请先选择账号";
      addReplyBtn.disabled = true;
      addReplyBtn.title = "请先选择账号";
    } else {
      addRuleBtn.disabled = false;
      addRuleBtn.title = "";
      addReplyBtn.disabled = false;
      addReplyBtn.title = "";
    }
  };

  const syncPendingHint = (): void => {
    if (accountId === "" || pendingItem === null || pendingItem === "") {
      pendingHint.style.display = "none";
      return;
    }
    const item = state.items.find((i) => i.id === pendingItem);
    pendingHint.style.display = "";
    pendingHint.replaceChildren(
      el("span", { class: "count-banner-count" }, "商品定位"),
      el(
        "span",
        { class: "count-banner-hint" },
        `已从商品列表跳入:点击「新建规则」将为「${item?.title.slice(0, 18) ?? pendingItem.slice(0, 12)}」预填范围(使用一次后清除)`
      )
    );
  };

  // ---- 数据装载 ----
  const loadItems = async (): Promise<ProductItem[]> => {
    const raw = await httpGet(`/api/v1/accounts/${accountId}/items?limit=100`);
    const arr = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    return arr.map(parseItem);
  };

  const loadRefs = async (): Promise<void> => {
    const [poolsRaw, tplRaw] = await Promise.all([
      httpGet("/api/v1/card-pools").catch(() => null),
      httpGet("/api/v1/delivery-templates").catch(() => null)
    ]);
    state.pools =
      poolsRaw === null
        ? []
        : (parsePage(poolsRaw, parseCardPool).items)
            .filter((p) => p.kind === "data")
            .map((p) => ({ id: p.id, name: p.name, enabled: p.enabled }));
    state.templates =
      tplRaw === null
        ? []
        : parsePage(tplRaw, parseTemplate)
            .items.map((t) => ({ id: t.id, name: t.name, enabled: t.enabled }));
    state.refs = buildRefIndex({
      pools: state.pools,
      templates: state.templates,
      items: state.items
    });
  };

  const deps = () => ({
    items: state.items,
    pools: state.pools,
    templates: state.templates,
    refs: state.refs
  });

  const toggleRule = (opener: HTMLElement, rule: RuleDto): void => {
    const payload = rebuildPayloadForToggle(rule, !rule.enabled);
    if (payload === null) {
      confirmDialog({
        title: "无法快捷切换",
        message: "固定文字规则不回读既有正文,无法在列表快捷启停;请打开编辑器重新输入内容后切换(整体替换)。",
        confirmLabel: "知道了"
      });
      return;
    }
    void httpPut(`/api/v1/accounts/${accountId}/rules/${rule.id}`, payload)
      .then(() => reloadActive())
      .catch((e: unknown) => {
        confirmDialog({ title: "操作失败", message: apiErrorMessage(e), confirmLabel: "知道了" });
      });
  };

  const deleteRule = (opener: HTMLElement, rule: RuleDto): void => {
    void opener;
    openConfirmFlow({
      title: "删除自动化规则",
      impact: `将删除规则「${scopeLabel(rule, state.refs)}」(优先级 ${rule.priority})。被历史交付引用的规则会被服务端拒绝,以保留审计链。`,
      requireReason: true,
      requireRiskAck: false,
      danger: true,
      confirmLabel: "删除",
      submit: () =>
        httpRequest<void>(`/api/v1/accounts/${accountId}/rules/${rule.id}?expected_version=${rule.version}`, {
          method: "DELETE"
        }),
      onSuccess: () => reloadActive()
    });
  };

  const ruleCard = (rule: RuleDto): HTMLElement => {
    const editBtn = el("button", { class: "icon-btn", type: "button", title: "编辑规则" });
    editBtn.append(icon("edit"));
    editBtn.addEventListener("click", () => {
      openRuleEditor(editBtn, accountId, deps(), rule, null, () => reloadActive());
    });
    const toggleBtn = btn(rule.enabled ? "停用" : "启用", { variant: rule.enabled ? "secondary" : "primary" });
    if (rebuildPayloadForToggle(rule, !rule.enabled) === null) {
      toggleBtn.disabled = true;
      toggleBtn.title = "固定文字规则不回读正文,请在编辑器中整体替换时切换";
    } else {
      toggleBtn.addEventListener("click", () => toggleRule(toggleBtn, rule));
    }
    const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除 ${scopeLabel(rule, state.refs)}` });
    delBtn.append(icon("trash"));
    delBtn.addEventListener("click", () => deleteRule(delBtn, rule));
    const warnings = ruleWarnings(rule);
    return el(
      "article",
      { class: "rule-card" },
      el(
        "div",
        { class: "tpl-card-head" },
        el("div", { class: "tpl-badge-pair" },
          badgeEl(triggerBadge(rule.triggerType), rule.triggerType),
          badgeEl(rule.enabled ? { tone: "normal", label: "已启用" } : { tone: "neutral", label: "已停用" })),
        el("div", { style: "display:flex;gap:6px;flex:none;align-items:center;" }, toggleBtn, editBtn, delBtn)
      ),
      el("strong", { class: "rule-card-scope", title: scopeLabel(rule, state.refs) }, scopeLabel(rule, state.refs)),
      el(
        "div",
        { class: "rule-card-meta muted" },
        `优先级 ${rule.priority}(越小越高) · v${rule.version}${rule.itemId === "" ? " · 账号级" : ""}`
      ),
      el(
        "ul",
        { class: "rule-summary" },
        ...actionSummary(rule, state.refs).map((line) => el("li", {}, line))
      ),
      warnings.length > 0
        ? el(
            "div",
            { class: "rule-warn" },
            ...warnings.map((w) =>
              badgeEl(
                w.kind === "needs_confirmation"
                  ? { tone: "warning", label: w.label }
                  : { tone: "danger", label: w.label },
                w.label
              )
            )
          )
        : null
    );
  };

  const buildTradeList = (): HTMLElement | null => {
    const filtered = filterRules(state.rules, { ...tradeFilter }, state.refs);
    tradeCount.textContent = `显示 ${filtered.length} / ${state.rules.length} 条`;
    if (filtered.length === 0) {
      if (state.rules.length === 0) return null;
      const clear = btn("清空筛选", { variant: "secondary" });
      clear.addEventListener("click", () => {
        tradeFilter.trigger = "";
        tradeFilter.status = "all";
        tradeFilter.query = "";
        triggerSel.value = "";
        statusSel.value = "all";
        tradeSearch.value = "";
        rerenderTradeLocal();
      });
      return el(
        "div",
        { class: "no-match" },
        el("div", { class: "empty-ico" }, "⚡"),
        el("div", {}, "无筛选结果"),
        el("div", { class: "muted" }, "当前触发类型/状态/关键词下没有规则"),
        clear
      );
    }
    return el("div", { class: "rule-grid" }, ...filtered.map(ruleCard));
  };

  /** 本地即时过滤:仅当网格/无匹配块在场时原位重建。 */
  let tradeInflight = 0;
  const rerenderTradeLocal = (): void => {
    if (tradeInflight > 0) return;
    const slot = tradeHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    if (current.classList.contains("rule-grid") || current.classList.contains("no-match")) {
      const node = buildTradeList();
      if (node !== null) slot.replaceChildren(node);
    }
  };

  const loadTrade = async (): Promise<HTMLElement | null> => {
    tradeInflight++;
    try {
      const [items, rulesRaw] = await Promise.all([
        loadItems(),
        httpGet(`/api/v1/accounts/${accountId}/rules?limit=100`)
      ]);
      state.items = items;
      const parsed = parseRuleList(rulesRaw);
      state.rules = parsed.rules;
      state.triggerCounts = parsed.triggerCounts;
      await loadRefs();
      syncPendingHint();
      const node = buildTradeList();
      if (node !== null) return node;
      const counts = state.triggerCounts;
      tradeCount.textContent = `暂无规则${Object.keys(counts).length > 0 ? `(历史计数:${Object.entries(counts).map(([k, n]) => `${triggerBadge(k).label} ${n}`).join(" · ")})` : ""}`;
      return null;
    } finally {
      tradeInflight--;
    }
  };

  const keywordCardActions = (row: ReplyRuleDto): HTMLElement => {
    const editBtn = el("button", { class: "icon-btn", type: "button", title: `编辑 ${row.keyword}` });
    editBtn.append(icon("edit"));
    editBtn.addEventListener("click", () => {
      openReplyRuleEditor(editBtn, accountId, state.items, row, () => reloadActive());
    });
    const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除 ${row.keyword}` });
    delBtn.append(icon("trash"));
    delBtn.addEventListener("click", () => {
      openConfirmFlow({
        title: `删除关键词「${row.keyword}」?`,
        impact: el(
          "div",
          {},
          el("p", { class: "error-text", style: "margin:0 0 6px;font-weight:600;" }, "此操作无法撤销"),
          el("p", { class: "muted", style: "margin:0;" }, "删除后买家消息包含该关键词不再自动回复;不影响订单与交付。")
        ),
        requireReason: false,
        requireRiskAck: false,
        confirmLabel: "确认删除",
        danger: true,
        submit: async () => {
          try {
            await httpRequest<void>(`/api/v1/accounts/${accountId}/reply-rules/${row.id}`, { method: "DELETE" });
          } catch (e) {
            throw new Error(apiErrorMessage(e));
          }
        },
        onSuccess: () => reloadActive()
      });
    });
    return el("div", { style: "display:flex;gap:4px;" }, editBtn, delBtn);
  };

  const buildKeywordList = (): HTMLElement | null => {
    const filtered = filterReplyRules(state.replyRules, keywordQuery, state.refs);
    if (filtered.length === 0) return null;
    const table = el("table", { class: "data" });
    table.append(
      el("tr", {},
        el("th", {}, "关键词"),
        el("th", {}, "关联范围"),
        el("th", {}, "回复类型"),
        el("th", {}, "内容预览"),
        el("th", {}, "状态"),
        el("th", {}, "操作"))
    );
    for (const row of filtered) {
      const scope =
        row.itemIds.length === 0
          ? "账号级"
          : row.itemIds.map((id) => state.refs.itemTitle(id) ?? id).join("、");
      const preview = row.replyKind === "image" ? row.replyImageUrl : row.replyText;
      table.append(
        el("tr", {},
          el("td", {}, badgeEl({ tone: "normal", label: row.keyword.slice(0, 16) }, row.keyword)),
          el("td", { title: scope }, scope.length > 16 ? `${scope.slice(0, 16)}…` : scope),
          el("td", {}, badgeEl(replyKindBadge(row.replyKind), row.replyKind)),
          el("td", { class: "muted" }, preview === null || preview === "" ? "—" : (preview.length > 18 ? `${preview.slice(0, 18)}…` : preview)),
          el("td", {}, badgeEl(row.enabled ? { tone: "normal", label: "已启用" } : { tone: "neutral", label: "已停用" })),
          el("td", {}, keywordCardActions(row)))
      );
    }
    return table;
  };

  const loadKeyword = async (): Promise<HTMLElement | null> => {
    const [items, raw] = await Promise.all([
      loadItems(),
      httpGet(`/api/v1/accounts/${accountId}/reply-rules`)
    ]);
    state.items = items;
    await loadRefs();
    const arr = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    state.replyRules = arr.map(parseReplyRule);
    return buildKeywordList();
  };

  const defaultCard = (reply: DefaultReplyDto, label: string): HTMLElement => {
    const editBtn = btn("编辑", { variant: "secondary" });
    editBtn.addEventListener("click", () => {
      openDefaultReplyEditor(editBtn, label, reply.accountId, reply, () => reloadActive());
    });
    const preview =
      reply.replyText !== null && reply.replyText !== ""
        ? reply.replyText.replace(/\s+/g, " ").slice(0, 40)
        : reply.replyImageUrl !== null && reply.replyImageUrl !== ""
          ? `图片:${reply.replyImageUrl.slice(0, 32)}…`
          : "未配置内容";
    return el(
      "article",
      { class: "rule-card" },
      el(
        "div",
        { class: "tpl-card-head" },
        el("strong", { class: "rule-card-scope" }, label),
        editBtn
      ),
      el(
        "div",
        { class: "tpl-badge-pair" },
        badgeEl(reply.enabled ? { tone: "normal", label: "已启用" } : { tone: "neutral", label: "已停用" }),
        badgeEl(reply.replyOnce ? { tone: "info", label: "只回复一次" } : { tone: "neutral", label: "每次都回复" })
      ),
      el("div", { class: "rule-summary" }, el("div", { title: reply.replyText ?? "" }, `内容预览:${preview}`)),
      reply.recentRecords.length > 0
        ? el("div", { class: "muted", style: "font-size:var(--text-xs);" }, `最近回复 ${reply.recentRecords.length} 条(只回复一次的依据;清空后恢复回复)`)
        : el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "暂无回复记录")
    );
  };

  const loadDefaults = async (): Promise<HTMLElement | null> => {
    if (accounts.length === 0) return null;
    const replies = await Promise.all(
      accounts.map(async (a) => {
        try {
          return parseDefaultReply(await httpGet(`/api/v1/accounts/${a.id}/default-reply`));
        } catch {
          return null; // 单账号失败不拖垮整页;编辑保存时如实报错
        }
      })
    );
    state.defaults = replies.filter((r): r is DefaultReplyDto => r !== null);
    if (state.defaults.length === 0 && accounts.length > 0) {
      const labels = new Map(accounts.map((a) => [a.id, a.displayName]));
      return el(
        "div",
        { class: "rule-grid" },
        ...accounts.map((a) =>
          defaultCard({ accountId: a.id, enabled: false, replyText: null, replyImageUrl: null, replyOnce: true, updatedAt: "", recentRecords: [] }, labels.get(a.id) ?? a.id)
        )
      );
    }
    const labels = new Map(accounts.map((a) => [a.id, a.displayName]));
    return el(
      "div",
      { class: "rule-grid" },
      ...state.defaults.map((r) => defaultCard(r, labels.get(r.accountId) ?? r.accountId))
    );
  };

  // ---- 工具栏事件 ----
  triggerSel.addEventListener("change", () => {
    tradeFilter.trigger = triggerSel.value;
    rerenderTradeLocal();
  });
  statusSel.addEventListener("change", () => {
    tradeFilter.status = statusSel.value === "enabled" || statusSel.value === "disabled" ? statusSel.value : "all";
    rerenderTradeLocal();
  });
  tradeSearch.addEventListener("input", () => {
    tradeFilter.query = tradeSearch.value;
    rerenderTradeLocal();
  });
  /** 关键词页签本地即时过滤(表格/无匹配块在场时原位重建)。 */
  const rerenderKeywordLocal = (): void => {
    const slot = keywordHost.querySelector(":scope > .async-slot");
    if (slot === null) return;
    const current = slot.firstElementChild;
    if (current === null) return;
    if (!current.classList.contains("data") && !current.classList.contains("no-match")) return;
    const node = buildKeywordList();
    if (node !== null) {
      slot.replaceChildren(node);
    } else if (state.replyRules.length > 0) {
      // 筛选无匹配轻空态(与总数为 0 的引导空态区分,一步清空)
      const clear = btn("清空筛选", { variant: "secondary" });
      clear.addEventListener("click", () => {
        keywordSearch.value = "";
        keywordQuery = "";
        rerenderKeywordLocal();
      });
      slot.replaceChildren(
        el("div", { class: "no-match" },
          el("div", { class: "empty-ico" }, "🔍"),
          el("div", {}, "无筛选结果"),
          el("div", { class: "muted" }, "当前关键词下没有匹配的回复规则"),
          clear)
      );
    }
  };
  keywordSearch.addEventListener("input", () => {
    keywordQuery = keywordSearch.value;
    rerenderKeywordLocal();
  });
  addRuleBtn.addEventListener("click", () => {
    if (accountId === "") return;
    const preselect = pendingItem;
    pendingItem = null; // 预填一次即消费
    syncUrl();
    syncPendingHint();
    openRuleEditor(addRuleBtn, accountId, deps(), null, preselect, () => reloadActive());
  });
  addReplyBtn.addEventListener("click", () => {
    if (accountId === "") return;
    openReplyRuleEditor(addReplyBtn, accountId, state.items, null, () => reloadActive());
  });

  // ---- 初始化:账号装载(深链预选)→ 激活当前页签 ----
  const boot = async (): Promise<void> => {
    try {
      accounts = await fetchAccounts();
    } catch (e) {
      accountHint.textContent = `账号加载失败:${apiErrorMessage(e)};点击刷新重试`;
      accountHint.className = "error-text";
      return;
    }
    if (accounts.length === 0) {
      accountHint.textContent = "暂无账号;请先在「账号」页接入";
      reloadActive();
      return;
    }
    for (const a of accounts) {
      accountSel.append(el("option", { value: a.id }, a.displayName));
    }
    if (accountId !== "" && accounts.some((a) => a.id === accountId)) {
      accountSel.value = accountId;
    } else {
      accountId = "";
      pendingItem = null;
      accountSel.value = "";
    }
    accountHint.textContent = "规则按账号管理;深链 /rules?account=&item= 由商品列表「关联发货规则」生成";
    syncUrl();
    syncMainButtons();
    reloadActive();
  };
  void boot();

  return () => {
    for (const b of blocks.values()) b.dispose();
  };
};
