/** 扫码接入弹窗(US3/T029):创建→展示二维码+倒计时→轮询→结果;
 *  过期在弹窗内"重新获取",长时间无响应显示仍在等待+取消(边界条款)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { openModal } from "../../ui/modal";
import { btn } from "../../ui/dom";
import { badgeEl, qrBadge } from "../../ui/badge";
import { formatDuration, parseIso } from "../../ui/time";

export type QrStage = "creating" | "showing" | "success" | "expired" | "failed";

/** 引擎状态 → 界面阶段(纯映射,便于行为测试)。 */
export function nextQrStage(state: string): QrStage {
  switch (state) {
    case "awaiting_scan":
    case "awaiting_authorization":
    case "verification_required":
      return "showing";
    case "authorized":
      return "success";
    case "expired":
      return "expired";
    default:
      return "failed"; // cancelled / failed / 未知
  }
}

interface QrSessionDto {
  id: string;
  state: string;
  expires_at: string;
}

function parseQr(v: unknown): QrSessionDto {
  if (!isRecord(v)) throw new Error("二维码会话格式错误");
  return {
    id: String(v["id"] ?? ""),
    state: String(v["state"] ?? ""),
    expires_at: String(v["expires_at"] ?? "")
  };
}

export interface QrModalCallbacks {
  /** 授权成功后刷新账号列表。 */
  onAuthorized: () => void;
}

const QR_STATE_HINTS: Record<string, string> = {
  awaiting_scan: "请用闲鱼 App 扫码",
  awaiting_authorization: "已扫码,请在手机上确认授权",
  verification_required: "平台要求安全验证,稍后可在账号卡发起验证"
};

export function openQrModal(opener: HTMLElement | null, cb: QrModalCallbacks): void {
  const handle = openModal(opener, { title: "扫码接入闲鱼账号" });
  const stageBox = el("div", { class: "qr-box" });
  const stateLine = el("div", { class: "muted" }, "");
  const countdown = el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "");
  const actionRow = el("div", { style: "display:flex;gap:12px;" }, "");
  handle.body.append(stageBox, stateLine, countdown, actionRow);

  let stopped = false;
  let pollTimer: number | undefined;
  let tickTimer: number | undefined;
  const cleanup = (): void => {
    stopped = true;
    if (pollTimer !== undefined) window.clearInterval(pollTimer);
    if (tickTimer !== undefined) window.clearInterval(tickTimer);
  };
  const origClose = handle.close.bind(handle);
  handle.close = (force?: boolean): void => {
    cleanup();
    origClose(force);
  };

  const renderStage = (stage: QrStage, qr: QrSessionDto | null): void => {
    stageBox.replaceChildren();
    actionRow.replaceChildren();
    if (stage === "creating") {
      stageBox.append(el("div", { class: "skeleton", style: "width:240px;height:240px;" }));
      stateLine.textContent = "正在创建二维码会话…";
      countdown.textContent = "";
      return;
    }
    if (stage === "showing" && qr !== null) {
      stageBox.append(
        el("img", { src: `/api/v1/accounts/qr-sessions/${qr.id}/image`, alt: "登录二维码" })
      );
      const b = qrBadge(qr.state);
      stateLine.replaceChildren(badgeEl(b, qr.state), el("span", { class: "muted" }, QR_STATE_HINTS[qr.state] ?? ""));
      const cancel = btn("取消", { variant: "ghost" });
      cancel.addEventListener("click", async () => {
        try {
          await httpPost(`/api/v1/accounts/qr-sessions/${qr.id}/cancel`, {});
        } catch {
          // 取消失败也停止本地轮询;服务端会话由 TTL 兜底
        }
        cleanup();
        origClose(true);
      });
      actionRow.append(cancel);
      return;
    }
    if (stage === "success") {
      stageBox.append(el("div", { class: "empty-state" }, el("div", { class: "empty-ico" }, "✅"), el("div", {}, "账号已接入")));
      stateLine.textContent = "请到账号卡启用运行与自动化开关";
      const done = btn("完成", { variant: "primary" });
      done.addEventListener("click", () => {
        cb.onAuthorized();
        handle.close(true);
      });
      actionRow.append(done);
      return;
    }
    // expired / failed
    const isExpired = stage === "expired";
    stageBox.append(
      el(
        "div",
        { class: "empty-state" },
        el("div", { class: "empty-ico" }, isExpired ? "⏰" : "⚠️"),
        el("div", {}, isExpired ? "二维码已过期" : "接入失败或已取消")
      )
    );
    stateLine.textContent = "";
    const retry = btn("重新获取", { variant: "primary" });
    retry.addEventListener("click", () => void start());
    actionRow.append(retry);
  };

  const startCountdown = (expiresAt: string): void => {
    if (tickTimer !== undefined) window.clearInterval(tickTimer);
    const update = (): void => {
      const t = parseIso(expiresAt);
      if (t === null) {
        countdown.textContent = "";
        return;
      }
      const remain = Math.max(0, Math.round((t - Date.now()) / 1000));
      countdown.textContent = remain > 0 ? `有效期剩余 ${formatDuration(remain)}` : "已过期,可重新获取";
    };
    update();
    tickTimer = window.setInterval(update, 1000);
  };

  const poll = (qrId: string): void => {
    let waiting = 0;
    pollTimer = window.setInterval(() => {
      if (stopped) return;
      void (async () => {
        try {
          const qr = parseQr(await httpGet(`/api/v1/accounts/qr-sessions/${qrId}`));
          const stage = nextQrStage(qr.state);
          if (stage === "showing") {
            waiting = 0;
            renderStage("showing", qr);
            return;
          }
          cleanup();
          renderStage(stage, qr);
        } catch {
          // 单次查询失败不终止轮询;持续无响应提示仍在等待
          waiting += 1;
          if (waiting >= 10) stateLine.textContent = "仍在等待平台响应…";
        }
      })();
    }, 3000);
  };

  const start = async (): Promise<void> => {
    renderStage("creating", null);
    try {
      const qr = parseQr(await httpPost("/api/v1/accounts/qr-sessions", { platform: "xianyu" }));
      if (stopped) return;
      renderStage("showing", qr);
      startCountdown(qr.expires_at);
      poll(qr.id);
    } catch (e) {
      cleanup();
      renderStage("failed", null);
      stateLine.textContent = `创建失败:${e instanceof Error ? e.message : String(e)}`;
    }
  };

  void start();
}
