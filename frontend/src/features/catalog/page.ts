/** 商品与发货规则页(US4):商品区 + 规则区,共享账号选择;
 *  点击商品/规则打开编辑抽屉(T034/T035 接线)。 */
import { el } from "../../shared/dom";
import { httpGet } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { badgeEl, ruleStateBadge } from "../../ui/badge";
import { renderProducts, parseItem, wireSyncButton, type AccountRef, type ProductItem } from "./products";
import { openRuleEditor, type ExistingRule } from "./rule-editor";
import { truncate } from "../../ui/dom";
import type { PageFactory } from "../../app/page";

interface RuleRow {
  id: string;
  accountId: string;
  itemId: string;
  skuKey: string;
  enabled: boolean;
  version: number;
}

function parseRule(v: unknown): RuleRow {
  if (!isRecord(v)) throw new Error("规则格式错误");
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    itemId: String(v["item_id"] ?? ""),
    skuKey: String(v["sku_key"] ?? ""),
    enabled: v["enabled"] === true,
    version: typeof v["version"] === "number" ? v["version"] : 1
  };
}

async function fetchAccounts(): Promise<AccountRef[]> {
  const raw = await httpGet("/api/v1/accounts");
  const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return items.map((a) => ({
    id: String((a as Record<string, unknown>)["id"] ?? ""),
    displayName: String((a as Record<string, unknown>)["display_name"] ?? "")
  }));
}

async function fetchItems(accountId: string): Promise<ProductItem[]> {
  const raw = await httpGet(`/api/v1/accounts/${accountId}/items`);
  const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return items.map(parseItem);
}

async function fetchRules(accountId: string): Promise<RuleRow[]> {
  const raw = await httpGet(`/api/v1/accounts/${accountId}/rules`);
  const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
  return items.map(parseRule);
}

export const catalogPage: PageFactory = (root) => {
  const productsBox = el("section", { class: "card" }, el("h2", { class: "card-title" }, "商品"));
  const rulesBox = el("section", { class: "card" }, el("h2", { class: "card-title" }, "发货规则"));
  const syncRow = el("div", { style: "display:flex;gap:12px;align-items:center;margin-bottom:12px;" });
  const syncBtn = btn("⟳ 从平台同步", { variant: "secondary" });
  const syncFeedback = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  syncRow.append(syncBtn, syncFeedback);

  const state: { accounts: AccountRef[]; items: ProductItem[]; rules: RuleRow[] } = {
    accounts: [],
    items: [],
    rules: []
  };

  const openRule = (item: ProductItem): void => {
    const account = state.accounts.find((a) => a.id === item.accountId);
    if (account === undefined) return;
    const existingRule = state.rules.find((r) => r.itemId === item.id && r.skuKey === "") ?? null;
    const existing: ExistingRule | null =
      existingRule === null
        ? null
        : { id: existingRule.id, version: existingRule.version, enabled: existingRule.enabled };
    openRuleEditor(null, account.id, item, existing, state.items, {
      onSaved: () => reloadRules()
    });
  };

  const renderRulesTable = (): void => {
    const box = rulesBox.querySelector(".rules-slot") ?? el("div", { class: "rules-slot" });
    rulesBox.append(box);
    box.replaceChildren();
    if (state.rules.length === 0) {
      box.append(
        el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "📜"),
          el("div", {}, "暂无规则;在商品列表点击「配置规则」为规格组合设置固定内容")
        )
      );
      return;
    }
    const byItem = new Map(state.items.map((i) => [i.id, i]));
    const table = el("table", { class: "data" });
    table.append(
      el("tr", {}, el("th", {}, "商品"), el("th", {}, "规格键"), el("th", {}, "状态"), el("th", {}, "版本"))
    );
    for (const r of state.rules) {
      const item = byItem.get(r.itemId);
      const tr = el(
        "tr",
        { class: "row-click" },
        el("td", {}, truncate(item?.title ?? r.itemId, 20)),
        el("td", { class: "mono" }, r.skuKey === "" ? "默认规格" : r.skuKey),
        el("td", {}, badgeEl(ruleStateBadge(r.enabled ? "configured" : "disabled"))),
        el("td", {}, `v${r.version}`)
      );
      tr.addEventListener("click", () => {
        if (item !== undefined) openRule(item);
      });
      table.append(tr);
    }
    box.append(table);
  };

  const reloadRules = (): void => {
    void (async () => {
      try {
        for (const a of state.accounts) {
          const rules = await fetchRules(a.id);
          // 多账号合并展示(规则行内含账号维度;当前 UI 以首个账号为主的单账号场景为主)
          state.rules = state.rules.filter((r) => r.accountId !== a.id).concat(rules);
        }
        renderRulesTable();
      } catch {
        renderRulesTable();
      }
    })();
  };

  const block = asyncBlock(
    root,
    async () => {
      state.accounts = await fetchAccounts();
      if (state.accounts.length === 0) {
        return el(
          "div",
          { class: "empty-state" },
          el("div", { class: "empty-ico" }, "🔗"),
          el("div", {}, "请先在「账号」页接入闲鱼账号,再同步商品")
        );
      }
      state.items = [];
      for (const a of state.accounts) {
        state.items = state.items.concat(await fetchItems(a.id));
      }
      wireSyncButton(syncBtn, state.accounts[0]!.id, syncFeedback, () => block.reload());

      const productsSlot = el("div", {});
      if (state.items.length === 0) {
        productsSlot.append(
          el(
            "div",
            { class: "empty-state" },
            el("div", { class: "empty-ico" }, "📦"),
            el("div", {}, "暂无商品记录;点击「从平台同步」后在此展示")
          )
        );
      } else {
        renderProducts(productsSlot, state.items, state.accounts, openRule);
      }
      productsBox.append(syncRow, productsSlot);
      rulesBox.append(el("div", { class: "rules-slot" }));
      reloadRules();
      return el("div", {}, productsBox, rulesBox);
    },
    { skeletonLines: 8 }
  );

  return () => block.dispose();
};
