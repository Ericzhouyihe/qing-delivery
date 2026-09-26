/** 商品共享内核(US4/T034 建立;007 T040 拆分后由 features/items 复用):
 *  契约解析 + 筛选(账号/仅已配置/关键字)+ 同步轮询反馈(FR-012)。
 *  列表渲染在 features/items/page.ts(含「关联发货规则」跨页跳转)。 */
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";

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
