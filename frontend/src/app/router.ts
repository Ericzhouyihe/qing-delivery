import type { PageFactory } from "./page";
import { accountsPage } from "../features/accounts";
import { catalogPage, rulesPage } from "../features/catalog";
import { ordersPage } from "../features/orders";
import { issuesPage } from "../features/issues";
import { settingsPage } from "../features/settings";
import { overviewPage } from "./overview";
import { renderAuth } from "./auth";
import { bootstrapSession, session } from "./session";
import { el } from "../shared/dom";
import { httpPost } from "../shared/http";

export interface Route {
  path: string;
  title: string;
  page: PageFactory;
}

export const routes: Route[] = [
  { path: "/", title: "概览", page: overviewPage },
  { path: "/accounts", title: "账号", page: accountsPage },
  { path: "/catalog", title: "商品", page: catalogPage },
  { path: "/rules", title: "发货规则", page: rulesPage },
  { path: "/orders", title: "订单", page: ordersPage },
  { path: "/issues", title: "待处理", page: issuesPage },
  { path: "/settings", title: "设置", page: settingsPage },
];

function matchRoute(path: string): Route {
  return routes.find((r) => r.path === path) ?? routes[0]!;
}

export function installRouter(root: HTMLElement): void {
  const renderNav = (): void => {
    const nav = el("nav", { class: "top" });
    for (const r of routes) {
      const a = el("a", { href: r.path }, r.title);
      if (location.pathname === r.path) {
        a.style.fontWeight = "bold";
      }
      nav.append(a);
    }
    const who = el("span", { class: "muted" }, session.displayName ?? "管理员");
    const logout = el("button", {}, "退出");
    logout.addEventListener("click", async () => {
      try {
        await httpPost("/api/v1/auth/logout");
      } catch {
        // 退出失败也回到登录界面(服务端会话由 TTL 兜底)
      }
      await bootstrapSession();
      render();
    });
    nav.append(who, logout);
    root.append(nav);
  };

  const render = (): void => {
    root.replaceChildren();
    if (!session.authenticated) {
      renderAuth(root, render);
      return;
    }
    const route = matchRoute(location.pathname);
    document.title = `轻交付 · ${route.title}`;
    renderNav();
    route.page(root);
  };

  document.addEventListener("click", (ev) => {
    if (!(ev.target instanceof HTMLAnchorElement)) return;
    const href = ev.target.getAttribute("href");
    if (href === null || !href.startsWith("/")) return;
    ev.preventDefault();
    history.pushState(null, "", href);
    render();
  });
  window.addEventListener("popstate", render);
  render();
}
