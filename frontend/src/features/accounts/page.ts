/** 账号管理页(US3/T028/T030/T031):卡片网格、扫码接入、启停确认流与过渡态、
 *  验证入口按能力如实展示(FR-011 不伪称可用)。 */
import { el } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";
import { badgeEl, accountBadge } from "../../ui/badge";
import { btn } from "../../ui/dom";
import { asyncBlock } from "../../ui/states";
import { formatRelative, startTicker } from "../../ui/time";
import { confirmDialog } from "../../ui/modal";
import { openQrModal } from "./qr-modal";
import type { PageFactory } from "../../app/page";

export interface AccountCard {
  id: string;
  platformUserId: string;
  displayName: string;
  connectionState: string;
  runEnabled: boolean;
  autoDelivery: boolean;
  autoConfirm: boolean;
  controlVersion: number;
  monitoringSince: string | null;
}

/** 异常优先排序(US3/FR-009):授权失效/需验证 > 连接中 > 在线 > 其余。 */
function priority(state: string): number {
  switch (state) {
    case "authorization_expired":
    case "verification_required":
      return 0;
    case "connecting":
      return 1;
    case "online":
      return 2;
    default:
      return 3;
  }
}

export function parseAccountCard(v: unknown): AccountCard {
  if (!isRecord(v)) throw new Error("账号格式错误");
  const c = (v["control"] ?? {}) as Record<string, unknown>;
  return {
    id: String(v["id"] ?? ""),
    platformUserId: String(v["platform_user_id"] ?? ""),
    displayName: String(v["display_name"] ?? ""),
    connectionState: String(v["connection_state"] ?? "paused"),
    runEnabled: c["run_enabled"] === true,
    autoDelivery: c["auto_delivery_enabled"] === true,
    autoConfirm: c["auto_confirm_enabled"] === true,
    controlVersion: typeof v["control_version"] === "number" ? v["control_version"] : 1,
    monitoringSince: typeof v["monitoring_since"] === "string" ? v["monitoring_since"] : null
  };
}

function maskPlatformId(id: string): string {
  if (id.length <= 8) return id;
  return `${id.slice(0, 4)}****${id.slice(-4)}`;
}

export const accountsPage: PageFactory = (root) => {
  const grid = el("div", { class: "account-grid" });
  const timeCells: Array<[HTMLElement, string]> = [];
  let stopTicker: (() => void) | null = null;
  /** 控制屏障进行中的账号(US3-4):仅由 applyControl 显式标记,不从状态推断。 */
  const pausingIds = new Set<string>();

  const load = async (): Promise<HTMLElement | null> => {
    const raw = await httpGet("/api/v1/accounts");
    const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    const accounts = items
      .map(parseAccountCard)
      .sort((a, b) => {
        const p = priority(a.connectionState) - priority(b.connectionState);
        return p !== 0 ? p : a.displayName.localeCompare(b.displayName);
      });
    if (accounts.length === 0) return null;
    grid.replaceChildren();
    timeCells.length = 0;
    for (const a of accounts) {
      grid.append(renderCard(a));
    }
    stopTicker = startTicker(() => {
      for (const [cell, iso] of timeCells) cell.textContent = formatRelative(iso);
    }, 30_000);
    return grid;
  };

  const renderCard = (a: AccountCard): HTMLElement => {
    const badgeBox = el("span", { class: "badge-pair" });
    const syncBadge = (): void => {
      badgeBox.replaceChildren(badgeEl(accountBadge(a.connectionState), a.connectionState));
      if (pausingIds.has(a.id)) {
        // 控制屏障进行中的本地过渡态(US3-4),落定后由 applyControl 移除
        badgeBox.append(el("span", { class: "badge badge-info" }, "暂停中"));
      }
    };
    syncBadge();

    const heartbeat = el("span", { class: "muted" }, a.monitoringSince ? formatRelative(a.monitoringSince) : "未启用监控");
    if (a.monitoringSince !== null) timeCells.push([heartbeat, a.monitoringSince]);

    const runToggle = el("input", { type: "checkbox", role: "switch" });
    runToggle.checked = a.runEnabled;
    runToggle.addEventListener("change", () => {
      runToggle.checked = a.runEnabled; // 确认前不改变视觉状态
      const enabling = !a.runEnabled;
      confirmDialog({
        title: enabling ? "启用账号" : "停用账号",
        message: enabling
          ? "启用后该账号将恢复接单与自动发货(按其自动化开关)。"
          : "停用后该账号停止接单与发货;进行中的交付按暂停屏障安全收尾。",
        confirmLabel: enabling ? "启用" : "停用",
        danger: !enabling,
        onConfirm: () => applyControl(a, enabling, syncBadge)
      });
    });

    const verifyBtn = btn("查看验证", { variant: "ghost" });
    verifyBtn.addEventListener("click", async () => {
      verifyBtn.disabled = true;
      try {
        const res = (await httpPost(`/api/v1/accounts/${a.id}/verification`, {})) as Record<string, unknown>;
        const v = (res["verification"] ?? {}) as Record<string, unknown>;
        const browserOk = v["browser_available"] === true;
        confirmDialog({
          title: "官方安全验证",
          message: el(
            "div",
            {},
            el("p", {}, String(v["instruction"] ?? "请在官方页面完成安全验证。")),
            el(
              "p",
              { class: browserOk ? "tone-normal" : "tone-warning" },
              browserOk ? "浏览器自动化已接入" : "⚠ 当前构建尚未接入浏览器自动化,本入口仅提供指引,不代操作"
            )
          ),
          confirmLabel: "知道了"
        });
      } catch (e) {
        confirmDialog({
          title: "验证不可用",
          message: `发起验证失败:${e instanceof Error ? e.message : String(e)}`,
          confirmLabel: "关闭"
        });
      } finally {
        verifyBtn.disabled = false;
      }
    });

    return el(
      "div",
      { class: "account-card" },
      el(
        "div",
        { class: "account-head" },
        el("span", { class: "avatar" }, a.displayName.slice(0, 1) || "?"),
        el(
          "div",
          {},
          el("strong", {}, a.displayName || "未命名账号"),
          el("div", { class: "muted", style: "font-size:var(--text-xs);" }, maskPlatformId(a.platformUserId))
        ),
        badgeBox
      ),
      el(
        "div",
        { class: "account-meta" },
        el("span", { class: a.autoDelivery ? "badge badge-normal" : "badge badge-neutral" }, a.autoDelivery ? "自动交付开" : "自动交付关"),
        el("span", { class: a.autoConfirm ? "badge badge-normal" : "badge badge-neutral" }, a.autoConfirm ? "自动确认开" : "自动确认关"),
        el("span", {}, "心跳:"),
        heartbeat
      ),
      el(
        "div",
        { class: "switch-row" },
        el("label", {}, "运行开关"),
        runToggle
      ),
      el("div", { style: "display:flex;justify-content:flex-end;" }, verifyBtn)
    );
  };

  /** 启停执行(US3-4):确认后本地进入"暂停中"过渡态,轮询单账号至状态落定。 */
  const applyControl = (a: AccountCard, enabling: boolean, syncBadge: () => void): Promise<void> => {
    a.runEnabled = enabling;
    pausingIds.add(a.id);
    syncBadge();
    return (async () => {
      await httpPost(`/api/v1/accounts/${a.id}/control`, {
        expected_version: a.controlVersion,
        run_enabled: enabling,
        acknowledge_monitoring_scope: true
      });
      const deadline = Date.now() + 30_000;
      await new Promise<void>((resolve) => {
        const timer = window.setInterval(() => {
          void (async () => {
            try {
              const fresh = parseAccountCard(await httpGet(`/api/v1/accounts/${a.id}`));
              a.connectionState = fresh.connectionState;
              a.controlVersion = fresh.controlVersion;
              a.monitoringSince = fresh.monitoringSince;
              syncBadge();
              const settled = enabling
                ? fresh.connectionState !== "paused"
                : fresh.connectionState === "paused";
              if (settled || Date.now() > deadline) {
                window.clearInterval(timer);
                resolve();
              }
            } catch {
              if (Date.now() > deadline) {
                window.clearInterval(timer);
                resolve();
              }
            }
          })();
        }, 2000);
      });
    })()
      .catch(() => {
        a.runEnabled = !enabling; // 失败回滚视觉状态
        syncBadge();
      })
      .finally(() => {
        pausingIds.delete(a.id);
        syncBadge();
      });
  };

  const emptyState = (): HTMLElement =>
    el(
      "div",
      { class: "empty-state" },
      el("div", { class: "empty-ico" }, "👤"),
      el("div", {}, "还没有接入闲鱼账号"),
      el("div", { class: "muted" }, "扫码接入后即可配置商品与自动发货")
    );

  const qrBtn = btn("＋ 扫码接入", { variant: "primary" });
  const toolbar = el("div", { style: "display:flex;justify-content:flex-end;margin-bottom:16px;" }, qrBtn);
  root.append(toolbar);

  const block = asyncBlock(root, load, { skeletonLines: 6, empty: emptyState });
  qrBtn.addEventListener("click", () => {
    openQrModal(qrBtn, {
      onAuthorized: () => {
        stopTicker?.();
        block.reload();
      }
    });
  });

  return () => {
    stopTicker?.();
    block.dispose();
  };
};
