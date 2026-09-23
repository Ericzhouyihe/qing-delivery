/** 订单详情抽屉(US5/T042):基本信息 + 三轴时间线 + 尝试记录 +
 *  内容揭示(默认折叠,授权端点)+ guard 启停的人工动作区(FR-017)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { openDrawer } from "../../ui/drawer";
import { btn } from "../../ui/dom";
import { badgeEl, confirmationBadge, deliveryBadge, reviewBadge } from "../../ui/badge";
import { renderTimeline, type TimelineNode } from "../../ui/timeline";
import { openConfirmFlow } from "../../ui/confirm-flow";
import { actionAvailability, manualActionBody, parseOrderRow, type ManualAction, type OrderRow } from "./model";
import { formatAbsolute } from "../../ui/time";

interface OrderDetail {
  summary: unknown;
  delivery: {
    id: string | null;
    state: string | null;
    rawContentState: string | null;
    reviewState: string | null;
    confirmationState: string | null;
    acceptedEvidence: string;
    automaticRetriesUsed: number;
    attempts: Array<{ id: string; kind: string; sequence: number; state: string }>;
  } | null;
}

function parseDetail(v: unknown): OrderDetail {
  if (!isRecord(v)) throw new Error("订单详情格式错误");
  const d = v["delivery"];
  const delivery = isRecord(d)
    ? {
        id: typeof d["id"] === "string" ? d["id"] : null,
        state: typeof d["state"] === "string" ? d["state"] : null,
        rawContentState: typeof d["raw_content_state"] === "string" ? d["raw_content_state"] : null,
        reviewState: typeof d["review_state"] === "string" ? d["review_state"] : null,
        confirmationState: typeof d["confirmation_state"] === "string" ? d["confirmation_state"] : null,
        acceptedEvidence: String(d["accepted_evidence"] ?? "none"),
        automaticRetriesUsed: typeof d["automatic_retries_used"] === "number" ? d["automatic_retries_used"] : 0,
        attempts: (Array.isArray(d["attempts"]) ? d["attempts"] : []).map((a) => {
          const r = a as Record<string, unknown>;
          return {
            id: String(r["id"] ?? ""),
            kind: String(r["kind"] ?? ""),
            sequence: typeof r["sequence"] === "number" ? r["sequence"] : 0,
            state: String(r["state"] ?? "")
          };
        })
      }
    : null;
  return { summary: v["summary"], delivery };
}

/** 三轴时间线节点(US5/FR-016 列表双轴、详情三轴全貌)。 */
export function buildTimelineNodes(row: OrderRow, detail: OrderDetail): TimelineNode[] {
  const nodes: TimelineNode[] = [];
  nodes.push({
    time: row.paidAt === null ? null : formatAbsolute(row.paidAt),
    axis: "订单",
    label: `已付款(${row.platformState})`,
    tone: "info"
  });
  const d = detail.delivery;
  if (d !== null) {
    if (d.state !== null) {
      const b = deliveryBadge(d.state);
      nodes.push({ time: null, axis: "内容交付", label: b.label, tone: b.tone });
    }
    if (d.reviewState !== null && d.reviewState !== "none") {
      const b = reviewBadge(d.reviewState);
      nodes.push({ time: null, axis: "审核", label: b.label, tone: b.tone });
    }
    if (d.confirmationState !== null && d.confirmationState !== "disabled") {
      const b = confirmationBadge(d.confirmationState);
      nodes.push({ time: null, axis: "平台确认", label: b.label, tone: b.tone });
    }
  }
  return nodes;
}

const ACTION_META: Record<ManualAction, { label: string; endpoint: string; requireRisk: boolean; riskText: string; impact: string }> = {
  resend: {
    label: "人工补发",
    endpoint: "resends",
    requireRisk: true,
    riskText: "我已确认此前内容未送达,知晓重复发送风险",
    impact: "将重新发送完整交付内容。若此前内容实际已送达,买家会收到重复内容——必须确认风险后执行。"
  },
  mark_received: {
    label: "确认平台已收到",
    endpoint: "mark-received",
    requireRisk: false,
    riskText: "",
    impact: "以人工结论覆盖平台确认结果,标记该订单确认完成。"
  },
  terminate: {
    label: "终止交付",
    endpoint: "terminate",
    requireRisk: false,
    riskText: "",
    impact: "终止该订单的全部交付路径;终止后不再自动或人工补发。"
  },
  takeover: {
    label: "人工接管",
    endpoint: "takeovers",
    // 接管必须显式确认历史范围(FR-022:服务端强制校验 acknowledge_historical_scope)
    requireRisk: true,
    riskText: "我确认接管监控起点之前的该笔订单,将由我在平台人工发货并自行负责",
    impact: "接管后该订单停止自动处理,由你在平台手动发货并在完成后回来标记。"
  }
};

export function openOrderDetail(row: OrderRow, onChanged: () => void): void {
  const handle = openDrawer(null, { title: `订单 ${row.platformOrderId}` });
  handle.body.append(el("p", { class: "muted" }, "加载详情…"));

  void (async () => {
    try {
      const raw = await httpGet(`/api/v1/accounts/${row.accountId}/orders/${row.id}`);
      const detail = parseDetail(raw);
      handle.body.replaceChildren();

      // 基本信息
      handle.body.append(
        el(
          "div",
          { class: "account-meta" },
          el("span", {}, `数量:${row.quantity === null ? "未知" : String(row.quantity)}`),
          el("span", {}, `交易类型:${row.tradeType}`),
          el("span", {}, `证据:${detail.delivery?.acceptedEvidence ?? "none"}`),
          el("span", {}, `自动重试已用:${detail.delivery?.automaticRetriesUsed ?? 0} 次`)
        ),
        el("h3", { style: "margin-top:12px;" }, "状态时间线"),
        renderTimeline(buildTimelineNodes(row, detail)),
        el("h3", { style: "margin-top:12px;" }, "交付尝试"),
        detail.delivery === null || detail.delivery.attempts.length === 0
          ? el("p", { class: "muted" }, "暂无尝试记录")
          : el(
              "table",
              { class: "data" },
              el("tr", {}, el("th", {}, "#"), el("th", {}, "动作"), el("th", {}, "状态")),
              ...detail.delivery.attempts.map((a) =>
                el(
                  "tr",
                  {},
                  el("td", {}, String(a.sequence)),
                  el("td", {}, a.kind),
                  el("td", {}, badgeEl(deliveryBadge(a.state), a.state))
                )
              )
            )
      );

      // 内容揭示(默认折叠,防肩窥;宪章 V)
      const revealBox = el("div", { class: "reveal-box" });
      const revealBtn = btn("👁 揭示交付内容", { variant: "secondary" });
      let revealed = false;
      let cached: string | null = null;
        revealBtn.addEventListener("click", async () => {
        if (revealed) {
          revealed = false;
          revealBox.replaceChildren();
          revealBtn.textContent = "👁 揭示交付内容";
          return;
        }
        revealBtn.disabled = true;
        try {
          if (cached === null) {
            // 授权揭示端点仅接受 POST(自动携带 CSRF;T063)
            const res = (await httpPost(`/api/v1/accounts/${row.accountId}/orders/${row.id}/content`)) as Record<string, unknown>;
            cached = String(res["content"] ?? "");
          }
          revealed = true;
          revealBtn.textContent = "🙈 收起内容";
          revealBox.replaceChildren(
            el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "仅限管理员核对使用,不要外传"),
            el("pre", {}, cached)
          );
        } catch (e) {
          revealBox.replaceChildren(
            el("p", { class: "error-text" }, `无法揭示:${e instanceof Error ? e.message : String(e)}`)
          );
        } finally {
          revealBtn.disabled = false;
        }
      });
      handle.body.append(el("h3", { style: "margin-top:12px;" }, "交付内容"), el("div", { style: "display:flex;gap:8px;align-items:flex-start;" }, revealBtn, revealBox));

      // 人工动作区:按展示状态启停 + 禁用原因;服务端 guard 权威(FR-017)
      const actionsBox = el("div", { style: "display:flex;flex-wrap:wrap;gap:8px;" });
      for (const action of ["resend", "mark_received", "terminate", "takeover"] as ManualAction[]) {
        const meta = ACTION_META[action];
        const avail = actionAvailability(action, detail.delivery?.state ?? null, detail.delivery?.confirmationState ?? null);
        const actionBtn = btn(meta.label, {
          variant: action === "resend" || action === "terminate" ? "danger" : "secondary",
          disabledReason: avail.enabled ? undefined : avail.reason
        });
        if (avail.enabled) {
          actionBtn.addEventListener("click", () => {
            openConfirmFlow({
              title: meta.label,
              impact: meta.impact,
              requireReason: true,
              requireRiskAck: meta.requireRisk,
              riskText: meta.riskText,
              confirmLabel: meta.label,
              danger: action === "resend" || action === "terminate",
              submit: async (reason, riskConfirmed) => {
                await httpPost(
                  `/api/v1/accounts/${row.accountId}/orders/${row.id}/${meta.endpoint}`,
                  manualActionBody(action, reason, riskConfirmed)
                );
              },
              onSuccess: () => {
                handle.close(true);
                onChanged();
              }
            });
          });
        }
        actionsBox.append(actionBtn);
      }
      handle.body.append(el("h3", { style: "margin-top:12px;" }, "人工动作"), actionsBox);
    } catch (e) {
      handle.body.replaceChildren(
        el("div", { class: "error-block" }, `详情加载失败:${e instanceof Error ? e.message : String(e)}`)
      );
    }
  })();
}

export { parseOrderRow };
