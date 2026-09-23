/** 商品区(US4/T034):表格 + 筛选(账号/仅已配置/关键字)+ 同步反馈(FR-012)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { badgeEl, itemStatusBadge, ruleStateBadge } from "../../ui/badge";
import { btn, truncate } from "../../ui/dom";

export interface ProductItem {
  id: string;
  accountId: string;
  platformItemId: string;
  title: string;
  status: string;
  skuDefinition: unknown;
  ruleState: string;
  version: number;
}

export interface AccountRef {
  id: string;
  displayName: string;
}

export function parseItem(v: unknown): ProductItem {
  if (!isRecord(v)) throw new Error("商品格式错误");
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    platformItemId: String(v["platform_item_id"] ?? ""),
    title: String(v["title"] ?? ""),
    status: String(v["status"] ?? "unknown"),
    skuDefinition: v["sku_definition"] ?? [],
    ruleState: String(v["rule_state"] ?? "missing"),
    version: typeof v["version"] === "number" ? v["version"] : 1
  };
}

export interface ProductFilter {
  accountId: string | null;
  configuredOnly: boolean;
  keyword: string;
}

export function filterItems(items: ProductItem[], f: ProductFilter): ProductItem[] {
  const kw = f.keyword.trim().toLowerCase();
  return items.filter((it) => {
    if (f.accountId !== null && it.accountId !== f.accountId) return false;
    if (f.configuredOnly && it.ruleState !== "configured") return false;
    if (kw !== "" && !it.title.toLowerCase().includes(kw) && !it.platformItemId.toLowerCase().includes(kw)) return false;
    return true;
  });
}

/** 商品筛选栏 + 表格;点击行回调打开规则编辑抽屉(US4)。 */
export function renderProducts(
  container: HTMLElement,
  items: ProductItem[],
  accounts: AccountRef[],
  onOpenRule: (item: ProductItem) => void
): void {
  const accountSel = el("select", {}) as HTMLSelectElement;
  accountSel.append(el("option", { value: "" }, "全部账号"));
  for (const a of accounts) {
    accountSel.append(el("option", { value: a.id }, a.displayName));
  }
  const configured = el("input", { type: "checkbox" });
  const configuredLabel = el("label", { class: "check-row", style: "margin:0;" }, configured, "仅看已配置规则");
  const keyword = el("input", { type: "text", placeholder: "搜索商品标题 / 平台 ID" });
  const clearBtn = btn("清空筛选", { variant: "ghost" });

  const tableBox = el("div", {});
  const apply = (): void => {
    const filtered = filterItems(items, {
      accountId: accountSel.value === "" ? null : accountSel.value,
      configuredOnly: configured.checked,
      keyword: keyword.value
    });
    tableBox.replaceChildren();
    if (filtered.length === 0) {
      tableBox.append(
        el("div", { class: "empty-state" }, el("div", { class: "empty-ico" }, "🔍"), el("div", {}, "没有符合条件的商品"))
      );
      return;
    }
    const table = el("table", { class: "data" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "商品"),
        el("th", {}, "平台 ID"),
        el("th", {}, "状态"),
        el("th", {}, "规则"),
        el("th", {}, "操作")
      )
    );
    for (const it of filtered) {
      const openBtn = btn("配置规则", { variant: "primary" });
      openBtn.addEventListener("click", () => onOpenRule(it));
      const tr = el(
        "tr",
        { class: "row-click" },
        el("td", {}, el("span", { class: "thumb" }), truncate(it.title, 20)),
        el("td", { class: "mono" }, it.platformItemId),
        el("td", {}, badgeEl(itemStatusBadge(it.status), it.status)),
        el("td", {}, badgeEl(ruleStateBadge(it.ruleState), it.ruleState)),
        el("td", {}, openBtn)
      );
      tr.addEventListener("click", (ev) => {
        if (ev.target instanceof HTMLButtonElement) return;
        onOpenRule(it);
      });
      table.append(tr);
    }
    tableBox.append(table);
  };

  accountSel.addEventListener("change", apply);
  keyword.addEventListener("input", apply);
  configured.addEventListener("change", apply);
  clearBtn.addEventListener("click", () => {
    accountSel.value = "";
    keyword.value = "";
    configured.checked = false;
    apply();
  });

  const bar = el("div", { class: "filter-bar" }, accountSel, configuredLabel, keyword, clearBtn);
  apply();
  container.replaceChildren(bar, tableBox);
}

/** 同步反馈(T034/收敛):接受 → 轮询任务至终态 → 展示结果并刷新列表;
 *  失败如实展示 safe_error(FR-011 不虚报)。 */
export function wireSyncButton(
  button: HTMLButtonElement,
  accountId: string,
  feedback: HTMLElement,
  onDone?: () => void
): void {
  button.addEventListener("click", async () => {
    button.disabled = true;
    feedback.className = "muted";
    feedback.textContent = "同步中…(正在从平台拉取在售商品)";
    try {
      const res = (await httpPost(`/api/v1/accounts/${accountId}/item-syncs`, {})) as Record<string, unknown>;
      const job = (res["job"] ?? {}) as Record<string, unknown>;
      const jobId = String(job["id"] ?? "");
      if (jobId === "") {
        feedback.className = "tone-normal";
        feedback.textContent = "已发起同步;完成后商品列表在此更新。";
        return;
      }
      // 轮询任务至终态(最长 ~90s)
      for (let i = 0; i < 60; i++) {
        await new Promise((r) => setTimeout(r, 1500));
        let j: Record<string, unknown>;
        try {
          j = (await httpGet(`/api/v1/jobs/${jobId}`)) as Record<string, unknown>;
        } catch {
          continue; // 单次查询失败不终止轮询
        }
        const state = String(j["state"] ?? "running");
        if (state === "succeeded") {
          feedback.className = "tone-normal";
          feedback.textContent = `同步完成:${String(j["result_ref"] ?? "")}`;
          onDone?.();
          return;
        }
        if (state === "failed") {
          feedback.className = "error-text";
          feedback.textContent = `同步失败:${String(j["safe_error"] ?? "未知错误")}`;
          return;
        }
        if (state === "cancelled" || state === "needs_review") {
          feedback.className = "error-text";
          feedback.textContent = `同步任务异常结束(${state})`;
          return;
        }
      }
      feedback.className = "muted";
      feedback.textContent = "同步仍在进行;稍后刷新页面查看商品。";
    } catch (e) {
      feedback.className = "error-text";
      feedback.textContent = `同步不可用:${e instanceof Error ? e.message : String(e)}`;
    } finally {
      button.disabled = false;
    }
  });
}
