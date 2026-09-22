import type { PageFactory } from "../../app/page";
import { el, errorMessage } from "../../shared/dom";
import { httpGet, httpPost } from "../../shared/http";
import { isRecord } from "../../shared/contracts";

interface AccountRow {
  id: string;
  platform_user_id: string;
  display_name: string;
  connection_state: string;
  control: {
    run_enabled: boolean;
    auto_delivery_enabled: boolean;
    auto_confirm_enabled: boolean;
    transition: string;
  };
  control_version: number;
  monitoring_since: string | null;
}

interface QrSession {
  id: string;
  state: string;
  expires_at: string;
  generation: number;
  image_path: string | null;
  account_id: string | null;
}

const STATE_LABELS: Record<string, string> = {
  connecting: "连接中",
  online: "在线",
  paused: "已暂停",
  offline: "离线",
  authorization_expired: "授权失效",
  verification_required: "需验证"
};

const QR_LABELS: Record<string, string> = {
  awaiting_scan: "等待扫码",
  awaiting_authorization: "已扫码,等待授权",
  authorized: "已授权",
  verification_required: "需要平台验证",
  expired: "已超时",
  cancelled: "已取消",
  failed: "失败"
};

function parseAccount(v: unknown): AccountRow {
  if (!isRecord(v)) throw new Error("账号格式错误");
  const c = (v["control"] ?? {}) as Record<string, unknown>;
  return {
    id: String(v["id"]),
    platform_user_id: String(v["platform_user_id"] ?? ""),
    display_name: String(v["display_name"] ?? ""),
    connection_state: String(v["connection_state"] ?? "paused"),
    control: {
      run_enabled: c["run_enabled"] === true,
      auto_delivery_enabled: c["auto_delivery_enabled"] === true,
      auto_confirm_enabled: c["auto_confirm_enabled"] === true,
      transition: String(c["transition"] ?? "stable")
    },
    control_version: typeof v["control_version"] === "number" ? v["control_version"] : 1,
    monitoring_since: typeof v["monitoring_since"] === "string" ? v["monitoring_since"] : null
  };
}

export const accountsPage: PageFactory = async (root) => {
  root.append(el("h1", {}, "账号"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  const qrArea = el("div", {});
  root.append(qrArea);

  const createQrBtn = el("button", {}, "扫码接入闲鱼账号");
  createQrBtn.addEventListener("click", () => void startQr(qrArea));
  root.append(createQrBtn);

  const list = el("div", {});
  root.append(list);
  try {
    const raw = await httpGet("/api/v1/accounts");
    const items = ((raw as Record<string, unknown>)["items"] ?? []) as unknown[];
    const accounts = items.map(parseAccount);
    status.textContent =
      accounts.length === 0 ? "暂无账号;点击下方按钮扫码接入。" : "";
    renderAccounts(list, accounts);
  } catch (e) {
    status.textContent = `账号加载失败:${errorMessage(e)}`;
  }
};

function renderAccounts(container: HTMLElement, accounts: AccountRow[]): void {
  container.replaceChildren();
  const table = el("table", { class: "list" });
  table.append(
    el(
      "tr",
      {},
      el("th", {}, "账号"),
      el("th", {}, "状态"),
      el("th", {}, "运行"),
      el("th", {}, "自动交付"),
      el("th", {}, "自动确认"),
      el("th", {}, "监控起点")
    )
  );
  for (const a of accounts) {
    table.append(
      el(
        "tr",
        {},
        el("td", {}, `${a.display_name}(${a.platform_user_id.slice(0, 8)})`),
        el("td", {}, STATE_LABELS[a.connection_state] ?? a.connection_state),
        el("td", {}, a.control.run_enabled ? "开" : "关"),
        el("td", {}, a.control.auto_delivery_enabled ? "开" : "关"),
        el("td", {}, a.control.auto_confirm_enabled ? "开" : "关"),
        el("td", {}, a.monitoring_since ?? "未启用")
      )
    );
  }
  container.append(table);
}

async function startQr(container: HTMLElement): Promise<void> {
  container.replaceChildren(el("p", { class: "muted" }, "创建二维码会话…"));
  try {
    const qr = (await httpPost<QrSession>("/api/v1/accounts/qr-sessions", {
      platform: "xianyu"
    })) as QrSession;
    await pollQr(container, qr.id);
  } catch (e) {
    container.replaceChildren(el("p", { class: "error" }, `创建失败:${errorMessage(e)}`));
  }
}

async function pollQr(container: HTMLElement, qrId: string): Promise<void> {
  const stateLine = el("p", {}, "");
  const cancelBtn = el("button", {}, "取消");
  cancelBtn.addEventListener("click", async () => {
    try {
      await httpPost(`/api/v1/accounts/qr-sessions/${qrId}/cancel`, {});
      void pollQr(container, qrId);
    } catch (e) {
      stateLine.textContent = `取消失败:${errorMessage(e)}`;
    }
  });
  container.replaceChildren(
    el("h2", {}, "扫码接入"),
    el("img", { src: `/api/v1/accounts/qr-sessions/${qrId}/image`, alt: "登录二维码" }),
    stateLine,
    cancelBtn
  );

  // 每 3 秒查询;超时可重新获取(重新发起 startQr)
  for (let i = 0; i < 60; i++) {
    try {
      const qr = (await httpGet<QrSession>(`/api/v1/accounts/qr-sessions/${qrId}`)) as QrSession;
      stateLine.textContent = QR_LABELS[qr.state] ?? qr.state;
      if (qr.state === "authorized") {
        stateLine.textContent += "——账号已接入,请到列表启用运行与自动化开关";
        return;
      }
      if (qr.state === "expired" || qr.state === "cancelled" || qr.state === "failed") {
        const retry = el("button", {}, "重新获取二维码");
        retry.addEventListener("click", () => void startQr(container));
        container.append(retry);
        return;
      }
    } catch (e) {
      stateLine.textContent = `查询失败:${errorMessage(e)}`;
      return;
    }
    await new Promise((r) => setTimeout(r, 3000));
  }
  stateLine.textContent = "查询超时,请重新获取二维码";
}
