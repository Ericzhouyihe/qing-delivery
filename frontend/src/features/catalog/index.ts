import type { PageFactory } from "../../app/page";
import { el, errorMessage } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";

const BADGE_LABELS: Record<string, string> = {
  configured: "已配置",
  missing: "未配置",
  disabled: "已禁用",
  conflict: "冲突"
};

const STATUS_LABELS: Record<string, string> = {
  on_sale: "在售",
  off_sale: "已下架",
  unknown: "未知"
};

export const catalogPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "商品"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  const syncInfo = el("p", { class: "muted" }, "");
  root.append(syncInfo);
  try {
    const raw = await httpGet("/api/v1/accounts");
    const accounts = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (accounts.length === 0) {
      status.textContent = "请先在「账号」页接入闲鱼账号,再同步商品。";
      return;
    }
    const account = accounts[0] as Record<string, unknown>;
    const accountId = String(account["id"]);
    status.textContent = "";
    const syncBtn = el("button", {}, "同步在售商品");
    syncBtn.addEventListener("click", async () => {
      syncInfo.textContent = "同步中…(真实平台同步待协议接入)";
      try {
        await httpPost(`/api/v1/accounts/${accountId}/item-syncs`, {});
        syncInfo.textContent = "已发起。";
      } catch (e) {
        syncInfo.textContent = `同步不可用:${errorMessage(e)}`;
      }
    });
    root.append(syncBtn);

    const itemsRaw = await httpGet(`/api/v1/accounts/${accountId}/items`);
    const items = ((itemsRaw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (items.length === 0) {
      status.textContent = "暂无商品记录;成功同步后在此展示(空列表为正常结果)。";
      return;
    }
    const table = el("table", { class: "list" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "商品"),
        el("th", {}, "平台 ID"),
        el("th", {}, "状态"),
        el("th", {}, "规则")
      )
    );
    for (const raw of items) {
      if (!isRecord(raw)) continue;
      const ruleState = String(raw["rule_state"] ?? "missing");
      table.append(
        el(
          "tr",
          {},
          el("td", {}, String(raw["title"] ?? "")),
          el("td", {}, String(raw["platform_item_id"] ?? "")),
          el("td", {}, STATUS_LABELS[String(raw["status"] ?? "unknown")] ?? "未知"),
          el("td", {}, BADGE_LABELS[ruleState] ?? ruleState)
        )
      );
    }
    root.append(table);
  } catch (e) {
    status.textContent = `商品加载失败:${errorMessage(e)}`;
  }
};

export const rulesPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "发货规则"));
  root.append(el("p", { class: "muted" }, "规则在商品同步后按完整规格组合配置;当前为只读视图,编辑器随商品数据一起启用。"));
  try {
    const raw = await httpGet("/api/v1/accounts");
    const accounts = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (accounts.length === 0) {
      root.append(el("p", { class: "muted" }, "请先接入账号。"));
      return;
    }
    const accountId = String((accounts[0] as Record<string, unknown>)["id"]);
    const rulesRaw = await httpGet(`/api/v1/accounts/${accountId}/rules`);
    const rules = ((rulesRaw as Record<string, unknown>)["items"] ?? []) as unknown[];
    if (rules.length === 0) {
      root.append(el("p", { class: "muted" }, "暂无规则;同步商品后为规格组合配置固定文字。"));
      return;
    }
    const table = el("table", { class: "list" });
    table.append(
      el(
        "tr",
        {},
        el("th", {}, "规则"),
        el("th", {}, "规格键"),
        el("th", {}, "状态"),
        el("th", {}, "版本")
      )
    );
    for (const r of rules) {
      if (!isRecord(r)) continue;
      table.append(
        el(
          "tr",
          {},
          el("td", {}, String(r["id"]).slice(0, 14) + "…"),
          el("td", {}, String(r["sku_key"] ?? "")),
          el("td", {}, r["enabled"] === true ? "启用" : "禁用"),
          el("td", {}, `v${String(r["version"] ?? 1)}(内容 v${String(r["content_version"] ?? 1)})`)
        )
      );
    }
    root.append(table);
  } catch (e) {
    root.append(el("p", { class: "error" }, `规则加载失败:${errorMessage(e)}`));
  }
};
