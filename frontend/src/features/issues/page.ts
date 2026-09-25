/** 待处理工作台(US6/T046/T047):按类别分组、等待时长、风险文案、允许操作;
 *  统一确认流(原因必填/风险勾选);完成即时移出并联动计数(FR-018)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { confirmDialog } from "../../ui/modal";
import { startTicker } from "../../ui/time";
import { setPendingIssues } from "../../app/store";
import { manualActionBody, type ManualAction } from "../orders/model";
import type { PageFactory } from "../../app/page";

export interface IssueItem {
  id: string;
  orderId: string | null;
  accountId: string;
  kind: string;
  reasonCode: string;
  allowedActions: string[];
  createdAt: string;
  /** 003:安全验证类携带 verification_url(json 解析后对象) */
  metadata?: Record<string, unknown> | null;
}

export interface RestoreReviewItem {
  id: string;
  kind: string;
  orderId: string | null;
  version: number;
}

export function parseIssueItem(v: unknown): IssueItem {
  if (!isRecord(v)) throw new Error("事项格式错误");
  // 契约字段:category(类别)/ reason(原因)/ allowed_actions(已是数组)
  const rawAllowed = v["allowed_actions"];
  const allowed = Array.isArray(rawAllowed) ? rawAllowed.map(String) : [];
  return {
    id: String(v["id"] ?? ""),
    orderId: v["order_id"] === null || v["order_id"] === undefined ? null : String(v["order_id"]),
    accountId: String(v["account_id"] ?? ""),
    kind: String(v["category"] ?? v["kind"] ?? ""),
    reasonCode: String(v["reason"] ?? v["reason_code"] ?? ""),
    allowedActions: allowed,
    createdAt: String(v["created_at"] ?? ""),
    metadata: parseMetadata(v["metadata"])
  };
}

function parseMetadata(raw: unknown): Record<string, unknown> | null {
  if (typeof raw !== "string" || raw.length === 0) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export function parseRestoreReview(v: unknown): RestoreReviewItem {
  if (!isRecord(v)) throw new Error("核对条目格式错误");
  return {
    id: String(v["id"] ?? ""),
    kind: String(v["kind"] ?? ""),
    orderId: v["order_id"] === null || v["order_id"] === undefined ? null : String(v["order_id"]),
    version: typeof v["version"] === "number" ? v["version"] : 1
  };
}

const KIND_LABELS: Record<string, { label: string; risk: string }> = {
  security_verification: {
    label: "安全验证",
    risk: "自动滑块未通过;请点击链接人工完成验证,系统将在完成后自动恢复账号。"
  },
  delivery_unknown: {
    label: "发送结果未知",
    risk: "结果未知时系统已停止自动重发;请核对平台实际送达情况后再决定补发或终止。"
  },
  delivery_rejected: { label: "平台拒绝发送", risk: "平台拒绝了发送请求;请确认内容合规后重试或人工接管。" },
  delivery_retry_exhausted: { label: "重试预算耗尽", risk: "自动重试已达上限;请人工核对后补发或终止。" },
  delivery_ineligible: { label: "资格不满足", risk: "订单不满足自动交付条件;请人工核对后处理。" },
  token_expired: { label: "账号令牌过期", risk: "请到账号页重新扫码授权。" }
};

const ACTION_LABELS: Record<string, { label: string; endpoint: string; danger: boolean; riskRequired: boolean; riskText: string; impact: string }> = {
  resend: {
    label: "补发",
    endpoint: "resends",
    danger: true,
    riskRequired: true,
    riskText: "我已核对平台记录,确认此前内容未送达",
    impact: "重新发送完整交付内容;若此前已实际送达,买家将收到重复内容。"
  },
  terminate: {
    label: "终止",
    endpoint: "terminate",
    danger: true,
    riskRequired: false,
    riskText: "",
    impact: "终止该订单全部交付路径,不再自动或人工补发。"
  },
  takeover: {
    label: "人工接管",
    endpoint: "takeovers",
    danger: false,
    // 接管必须显式确认历史范围(FR-022:服务端强制校验 acknowledge_historical_scope)
    riskRequired: true,
    riskText: "我确认接管监控起点之前的该笔订单,将由我在平台人工发货并自行负责",
    impact: "接管后停止自动处理,由你在平台手动发货。"
  },
  mark_received: {
    label: "确认已收到",
    endpoint: "mark-received",
    danger: false,
    riskRequired: false,
    riskText: "",
    impact: "以人工结论标记平台确认完成。"
  }
};

export const issuesPage: PageFactory = (root) => {
  let stopTicker: (() => void) | null = null;

  const block = asyncBlock(
    root,
    async () => {
      const [issuesRaw, restoreRaw] = await Promise.all([
        httpGet("/api/v1/issues?state=open&limit=100").catch(() => null),
        httpGet("/api/v1/restore").catch(() => null)
      ]);
      const issues = issuesRaw === null
        ? []
        : (((issuesRaw as Record<string, unknown>)["items"] ?? []) as unknown[]).map(parseIssueItem);
      const restoreSummary =
        restoreRaw !== null && isRecord(restoreRaw) && restoreRaw["unresolved_count"] !== undefined
          ? (restoreRaw as Record<string, unknown>)
          : null;

      let reviews: RestoreReviewItem[] = [];
      if (restoreSummary !== null) {
        const raw = await httpGet("/api/v1/restore/reviews?state=unresolved").catch(() => null);
        if (raw !== null) {
          reviews = (((raw as Record<string, unknown>)["items"] ?? []) as unknown[]).map(parseRestoreReview);
        }
      }

      const total = issues.length + reviews.length;
      setPendingIssues(total);
      if (total === 0) {
        return el(
          "div",
          { class: "empty-state", style: "padding:48px 16px;" },
          el("div", { class: "empty-ico" }, "✅"),
          el("div", { style: "font-size:var(--text-lg);" }, "全部处理完毕"),
          el("div", { class: "muted" }, "未知结果、失败与恢复核对事项会持久出现在这里,直到人工处理")
        );
      }

      const oldest = issues
        .map((x) => x.createdAt)
        .filter((t) => t.length > 0)
        .sort()
        .at(0);

      const wrap = el("div", {});
      const header = el(
        "div",
        { class: "filter-bar" },
        el("span", { class: "badge badge-warning" }, `待处理 ${total}`),
        el(
          "span",
          { class: "muted" },
          restoreSummary === null ? "" : `恢复隔离中 · 未核对 ${String(restoreSummary["unresolved_count"] ?? "?")} 笔 · `
        ),
        el("button", { class: "btn btn-ghost btn-sm" }, "↻ 刷新")
      );
      const refreshBtn = header.lastElementChild as HTMLButtonElement;
      refreshBtn.addEventListener("click", () => block.reload());
      if (oldest !== undefined) {
        const oldestLabel = el("span", { class: "muted" }, "");
        const update = (): void => {
          const sec = Math.max(0, Math.round((Date.now() - (Date.parse(oldest) || Date.now())) / 1000));
          oldestLabel.textContent = `最久已等待 ${Math.floor(sec / 60)} 分钟`;
        };
        update();
        stopTicker = startTicker(update, 30_000);
        header.append(oldestLabel);
      }
      wrap.append(header);

      const removeItem = (node: HTMLElement): void => {
        node.classList.add("leaving");
        window.setTimeout(() => block.reload(), 250);
      };

      // 分组:恢复核对优先,其余按 kind
      if (reviews.length > 0) {
        const group = el("div", { class: "issue-group" });
        group.append(
          el("div", { class: "issue-group-head" }, el("h3", { style: "margin:0;" }, "恢复核对"), el("span", { class: "badge badge-warning" }, String(reviews.length)))
        );
        for (const r of reviews) {
          const item = el("div", { class: "issue-item" });
          const receivedBtn = btn("确认已收到", { variant: "danger" });
          const notSentBtn = btn("确认未发送", { variant: "secondary" });
          const terminateBtn = btn("终止旧路径", { variant: "secondary" });
          const decide = (decision: "received" | "approved_not_sent" | "terminated", riskRequired: boolean): void => {
            openConfirmFlow({
              title: "恢复核对决定",
              impact: `备份恢复期间该订单可能已有交付动作;决定将解除隔离并记录审计。订单:${r.orderId ?? "—"}`,
              requireReason: true,
              requireRiskAck: riskRequired,
              riskText: "我已核对平台记录,确认买家已收到,接受重复内容风险",
              confirmLabel: "提交决定",
              danger: riskRequired,
              submit: async (reason, riskConfirmed) => {
                await httpPost(`/api/v1/restore/reviews/${r.id}/decisions`, {
                  expected_version: r.version,
                  decision,
                  reason,
                  evidence_note: "待处理工作台核对",
                  acknowledge_duplicate_risk: riskConfirmed
                });
              },
              onSuccess: () => removeItem(item)
            });
          };
          receivedBtn.addEventListener("click", () => decide("received", true));
          notSentBtn.addEventListener("click", () => decide("approved_not_sent", false));
          terminateBtn.addEventListener("click", () => decide("terminated", false));
          item.append(
            el("strong", {}, "恢复核对"),
            el("span", { class: "muted mono" }, `订单 ${r.orderId?.slice(0, 14) ?? "—"}…`),
            el("span", { class: "muted" }, "kind:" + r.kind),
            el("span", { style: "flex:1;" }),
            notSentBtn,
            receivedBtn,
            terminateBtn
          );
          group.append(item);
        }
        wrap.append(group);
      }

      const byKind = new Map<string, IssueItem[]>();
      for (const it of issues) {
        const list = byKind.get(it.kind) ?? [];
        list.push(it);
        byKind.set(it.kind, list);
      }
      for (const [kind, list] of byKind) {
        const meta = KIND_LABELS[kind] ?? { label: kind, risk: "请人工核对后处理。" };
        const group = el("div", { class: "issue-group" });
        group.append(
          el("div", { class: "issue-group-head" }, el("h3", { style: "margin:0;" }, meta.label), el("span", { class: "badge badge-warning" }, String(list.length)))
        );
        for (const it of list) {
          const item = el("div", { class: "issue-item" });
          const waitLabel = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, "");
          const updateWait = (): void => {
            const t = Date.parse(it.createdAt);
            if (Number.isNaN(t)) {
              waitLabel.textContent = "";
              return;
            }
            const sec = Math.max(0, Math.round((Date.now() - t) / 1000));
            waitLabel.textContent = sec < 90 ? `${sec} 秒` : `${Math.floor(sec / 60)} 分钟`;
          };
          updateWait();
          stopTicker = startTicker(updateWait, 30_000);
          item.append(
            el("span", { class: "badge badge-warning" }, meta.label),
            el("span", { class: "muted mono" }, it.orderId === null ? "无关联订单" : `订单 ${it.orderId.slice(0, 14)}…`),
            el("span", { class: "muted", title: it.reasonCode }, it.reasonCode.slice(0, 24)),
            waitLabel,
            el("span", { style: "flex:1;" })
          );
          for (const action of it.allowedActions) {
            // 003:安全验证类动作不走订单端点
            if (action === "open_verification") {
              const url = (it.metadata as Record<string, unknown> | undefined)?.["verification_url"];
              if (typeof url !== "string" || url.length === 0) continue; // 无链接不渲染按钮
              const openBtn = btn("打开验证页", { variant: "primary" });
              openBtn.addEventListener("click", () => window.open(url, "_blank", "noopener"));
              item.append(openBtn);
              continue;
            }
            if (action === "resolve") {
              const resolveBtn = btn("我已处理", { variant: "secondary" });
              resolveBtn.addEventListener("click", () => {
                // 恢复检测由服务 60s 周期自动执行;这里仅尽快刷新界面
                resolveBtn.disabled = true;
                resolveBtn.textContent = "等待恢复检测…";
                window.setTimeout(() => block.reload(), 60_000);
              });
              item.append(resolveBtn);
              continue;
            }
            const meta2 = ACTION_LABELS[action];
            if (meta2 === undefined) continue;
            const actionBtn = btn(meta2.label, { variant: meta2.danger ? "danger" : "secondary" });
            actionBtn.addEventListener("click", () => {
              if (it.orderId === null) return;
              openConfirmFlow({
                title: meta2.label,
                impact: `${meta2.impact} ${meta.risk}`,
                requireReason: true,
                requireRiskAck: meta2.riskRequired,
                riskText: meta2.riskText,
                confirmLabel: meta2.label,
                danger: meta2.danger,
                submit: async (reason, riskConfirmed) => {
                  await httpPost(
                    `/api/v1/accounts/${it.accountId}/orders/${it.orderId}/${meta2.endpoint}`,
                    manualActionBody(action as ManualAction, reason, riskConfirmed)
                  );
                },
                onSuccess: () => removeItem(item)
              });
            });
            item.append(actionBtn);
          }
          group.append(item);
        }
        wrap.append(group);
      }
      return wrap;
    },
    { skeletonLines: 6 }
  );

  return () => {
    stopTicker?.();
    block.dispose();
  };
};
