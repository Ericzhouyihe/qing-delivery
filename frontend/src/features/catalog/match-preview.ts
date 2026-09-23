/** 匹配预览面板(US4/T036):商品/规格定位 → 服务端 match-preview 判定 →
 *  matched/none/ambiguous 三态呈现;前端零匹配算法(FR-014/澄清决定)。 */
import { el } from "../../shared/dom";
import { httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import type { ProductItem } from "./products";

export type MatchOutcome =
  | { state: "matched"; ruleId: string; ruleVersion: number }
  | { state: "none"; reason: string }
  | { state: "ambiguous"; reason: string };

export function parseMatchOutcome(v: unknown): MatchOutcome {
  if (!isRecord(v)) throw new Error("预演响应格式错误");
  const state = String(v["state"] ?? "");
  if (state === "matched") {
    return {
      state: "matched",
      ruleId: String(v["rule_id"] ?? ""),
      ruleVersion: typeof v["rule_version"] === "number" ? v["rule_version"] : 0
    };
  }
  if (state === "ambiguous") {
    return { state: "ambiguous", reason: String(v["reason"] ?? "多条启用规则命中") };
  }
  return { state: "none", reason: String(v["reason"] ?? "无启用规则命中") };
}

export function renderMatchOutcome(outcome: MatchOutcome): HTMLElement {
  if (outcome.state === "matched") {
    return el(
      "div",
      { class: "banner banner-warning", style: "margin:0;background:var(--tone-normal-bg);border-color:var(--tone-normal-fg);color:var(--tone-normal-fg);" },
      el("span", { class: "badge badge-normal" }, "命中"),
      el("span", {}, `将按规则 ${outcome.ruleId.slice(0, 12)}…(v${outcome.ruleVersion})自动发货`)
    );
  }
  if (outcome.state === "ambiguous") {
    return el(
      "div",
      { class: "banner banner-warning", role: "alert" },
      el("span", { class: "badge badge-warning" }, "冲突"),
      el("span", {}, `${outcome.reason};请先去重,否则该组合不会自动发货`)
    );
  }
  return el(
    "div",
    { class: "banner banner-warning", style: "margin:0;" },
    el("span", { class: "badge badge-neutral" }, "未命中"),
    el("span", {}, `${outcome.reason},该组合不会自动发货`)
  );
}

/** 面板:选择商品(客户端过滤已同步目录,仅定位 item_id,不参与判定)+ 规格键。 */
export function renderMatchPreview(
  container: HTMLElement,
  accountId: string,
  items: ProductItem[],
  defaults: { item?: ProductItem; skuKey?: string } = {}
): void {
  const itemSel = el("select", {}) as HTMLSelectElement;
  itemSel.append(el("option", { value: "" }, "选择商品…"));
  for (const it of items) {
    itemSel.append(el("option", { value: it.id }, it.title.slice(0, 40)));
  }
  if (defaults.item !== undefined) itemSel.value = defaults.item.id;
  const skuInput = el("input", { type: "text", placeholder: "规格键(默认规格留空)" }) as HTMLInputElement;
  if (defaults.skuKey !== undefined) skuInput.value = defaults.skuKey;
  const runBtn = btn("预览匹配", { variant: "secondary" });
  const resultBox = el("div", { style: "margin-top:12px;" }, "");

  runBtn.addEventListener("click", async () => {
    if (itemSel.value === "") {
      resultBox.replaceChildren(el("p", { class: "error-text" }, "请先选择商品"));
      return;
    }
    runBtn.disabled = true;
    resultBox.replaceChildren(el("p", { class: "muted" }, "正在由交付引擎预演…"));
    try {
      const raw = await httpPost(`/api/v1/accounts/${accountId}/rules/match-preview`, {
        item_id: itemSel.value,
        sku_key: skuInput.value
      });
      resultBox.replaceChildren(renderMatchOutcome(parseMatchOutcome(raw)));
    } catch (e) {
      resultBox.replaceChildren(
        el("p", { class: "error-text" }, `预演失败:${e instanceof Error ? e.message : String(e)}`)
      );
    } finally {
      runBtn.disabled = false;
    }
  });

  // 追加而非替换容器内容:宿主(规则抽屉)可能已有表单(US4 走查修复)
  container.append(el("div", {}, el("div", { class: "filter-bar" }, itemSel, skuInput, runBtn), resultBox));
}
