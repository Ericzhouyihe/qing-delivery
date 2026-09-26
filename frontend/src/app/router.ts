import type { PageFactory } from "./page";
import { accountsPage } from "../features/accounts";
import { cardsPage } from "../features/cards";
import { templatesPage } from "../features/templates";
import { itemsPage } from "../features/items";
import { rulesPage } from "../features/rules";
import { chatPage } from "../features/chat";
import { ordersPage } from "../features/orders";
import { issuesPage } from "../features/issues";
import { notificationsPage } from "../features/notifications";
import { settingsPage } from "../features/settings";
import { overviewPage } from "../features/overview/page";
import { renderAuth } from "./auth";
import { bootstrapSession, session } from "./session";
import { mountShell, type ShellHandle } from "./shell";
import { getChatUnread, getPendingIssues } from "./store";

export interface Route {
  path: string;
  title: string;
  icon: string;
  page: PageFactory;
}

export const routes: Route[] = [
  { path: "/", title: "概览", icon: "◫", page: overviewPage },
  { path: "/accounts", title: "账号", icon: "◉", page: accountsPage },
  // 007 T053:在线聊天(账号之后;侧边栏未读徽标由聊天页轮询写入 store 驱动)
  { path: "/chat", title: "在线聊天", icon: "✉", page: chatPage },
  // 007 T016:卡密库存(账号之后,内容链入口;最终导航顺序见 tasks T084)
  { path: "/cards", title: "卡密库存", icon: "▤", page: cardsPage },
  // 007 T040:/catalog「商品与规则」合并页拆分为商品列表与自动化规则两页(FR-001);
  // 顺序对齐参考页:卡密库存、商品列表、订单、发货模板、自动化规则、待处理…
  { path: "/items", title: "商品列表", icon: "▦", page: itemsPage },
  { path: "/orders", title: "订单", icon: "⇄", page: ordersPage },
  { path: "/templates", title: "发货模板", icon: "▣", page: templatesPage },
  { path: "/rules", title: "自动化规则", icon: "⚡", page: rulesPage },
  { path: "/issues", title: "待处理", icon: "⚑", page: issuesPage },
  // 007 T064:通知设置(待处理之后、设置之前;最终导航顺序见 tasks T084)
  { path: "/notifications", title: "通知设置", icon: "◆", page: notificationsPage },
  // 007 T084:按 FR-001 对齐参考页命名「系统与AI」(页面内标题同步)
  { path: "/settings", title: "系统与AI", icon: "⚙", page: settingsPage }
];

function matchRoute(path: string): Route {
  return routes.find((r) => r.path === path) ?? routes[0]!;
}

/** 会话过期回落登录时记住目标页,重登后返回原页面(US8-2/FR-020)。 */
let returnTo: string | null = null;

export function installRouter(root: HTMLElement): void {
  let shell: ShellHandle | null = null;
  let dispose: (() => void) | null = null;

  const nav = routes.map((r) => ({
    path: r.path,
    title: r.title,
    icon: r.icon,
    badge: r.path === "/issues" ? () => getPendingIssues() : r.path === "/chat" ? () => getChatUnread() : undefined
  }));

  const render = (): void => {
    dispose?.();
    dispose = null;
    if (!session.authenticated) {
      const path = location.pathname;
      // 根路径是默认落点,不作为"原页面"记忆
      returnTo = path !== "/" && path !== "" ? path : null;
      shell = null;
      root.replaceChildren();
      renderAuth(root, render);
      return;
    }
    const route = matchRoute(location.pathname);
    document.title = `轻交付 · ${route.title}`;
    if (shell === null) {
      shell = mountShell(root, nav, async () => {
        await bootstrapSession();
        render();
      });
    }
    shell.setTitle(route.title);
    shell.refresh(route.path);
    shell.main.replaceChildren();
    const result = route.page(shell.main);
    void Promise.resolve(result).then((fn) => {
      if (typeof fn === "function") dispose = fn;
    });
  };

  // 会话过期(http.ts 派发):回落登录,目标页已在 returnTo
  document.addEventListener("qing:session-expired", () => {
    void bootstrapSession().then(render);
  });

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

/** 供 auth 在登录成功后跳转到记忆的原页面。 */
export function consumeReturnTo(): string | null {
  const target = returnTo;
  returnTo = null;
  return target;
}
