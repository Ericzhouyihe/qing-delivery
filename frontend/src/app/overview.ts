import type { PageFactory } from "./page";
import { el, errorMessage } from "../shared/dom";
import { httpGet } from "../shared/http";
import { isRecord } from "../shared/contracts";

/** 概览 = dashboard 每 3 秒轮询;页面隐藏即停止(FR-024/SC-006)。 */
export const overviewPage: PageFactory = (root) => {
  root.append(el("h1", {}, "概览"));
  const status = el("p", { class: "muted" }, "加载中…");
  root.append(status);
  const body = el("div", {});
  root.append(body);

  let timer: number | undefined;
  const render = (data: Record<string, unknown>): void => {
    body.replaceChildren();
    const restore = data["restore"];
    if (isRecord(restore)) {
      body.append(
        el(
          "p",
          { class: "error" },
          `⚠ 恢复隔离中:待核对 ${String(restore["unresolved_count"] ?? "?")} 笔;请到「设置」页逐单核对后才能恢复处理`
        )
      );
    }
    if (data["stopping"] === true) {
      body.append(el("p", { class: "error" }, "服务正在停止:不再接受新任务"));
    }
    const accounts = (data["accounts"] ?? []) as unknown[];
    if (accounts.length === 0) {
      body.append(el("p", { class: "muted" }, "暂无账号;到「账号」页扫码接入。"));
      return;
    }
    const table = el("table", { class: "list" });
    table.append(
      el("tr", {}, el("th", {}, "账号"), el("th", {}, "状态"), el("th", {}, "自动交付"))
    );
    for (const raw of accounts) {
      if (!isRecord(raw)) continue;
      table.append(
        el(
          "tr",
          {},
          el("td", {}, String(raw["display_name"] ?? "")),
          el("td", {}, String(raw["connection_state"] ?? "paused")),
          el("td", {}, raw["auto_delivery_enabled"] === true ? "开" : "关")
        )
      );
    }
    body.append(table);
    body.append(el("p", { class: "muted" }, `待处理事项:${String(data["open_issue_count"] ?? 0)}`));
  };

  const poll = async (): Promise<void> => {
    if (document.hidden) return;
    try {
      const data = (await httpGet("/api/v1/dashboard")) as Record<string, unknown>;
      status.textContent = "";
      render(data);
    } catch (e) {
      status.textContent = `摘要加载失败:${errorMessage(e)}`;
    }
  };
  void poll();
  timer = window.setInterval(() => void poll(), 3000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void poll();
  });
  root.addEventListener("DOMNodeRemoved", () => window.clearInterval(timer));
  // 兜底:路由切换清理
  window.addEventListener("beforeunload", () => window.clearInterval(timer));
};
