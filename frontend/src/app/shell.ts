/** 布局外壳(T013,研究 D7/FR-001):登录后挂载一次;
 *  路由切换仅重渲染内容区;侧边栏窄窗折叠由 CSS 承担。 */
import { el } from "../shared/dom";
import { httpPost } from "../shared/http";
import { btn } from "../ui/dom";
import { session } from "./session";
import { applyTheme, type ThemeMode } from "../ui/theme";

export interface ShellNav {
  path: string;
  title: string;
  icon: string;
  /** 导航徽章取值(如待处理数);无则不显示。 */
  badge?: () => number | undefined;
}

export interface ShellHandle {
  /** 内容区节点:路由把页面渲染进这里。 */
  main: HTMLElement;
  /** 顶栏页面标题。 */
  setTitle(t: string): void;
  /** 重新高亮当前导航项/更新徽章。 */
  refresh(activePath: string): void;
}

export function mountShell(root: HTMLElement, nav: ShellNav[], onLogout: () => void): ShellHandle {
  const side = el("aside", { class: "app-side" });
  const brand = el(
    "div",
    { class: "brand" },
    el("span", { class: "brand-mark" }, "轻"),
    el("span", { class: "brand-label" }, "轻交付")
  );
  const navBox = el("nav", { class: "side-nav" });
  const links = new Map<string, HTMLAnchorElement>();
  for (const item of nav) {
    const a = el(
      "a",
      { href: item.path },
      el("span", { class: "nav-ico" }, item.icon),
      el("span", { class: "nav-label" }, item.title)
    );
    links.set(item.path, a);
    navBox.append(a);
  }

  const themeBtn = el("button", { class: "btn btn-ghost btn-sm", title: "切换明暗主题" }, "◐ 主题");
  themeBtn.addEventListener("click", () => {
    const current = document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
    const next: ThemeMode = current === "dark" ? "light" : "dark";
    applyTheme(next);
    themeBtn.textContent = next === "dark" ? "☀ 主题" : "◐ 主题";
  });
  const logout = btn("退出", { variant: "secondary" });
  logout.addEventListener("click", async () => {
    logout.disabled = true;
    try {
      await httpPost("/api/v1/auth/logout");
    } catch {
      // 退出失败也回到登录界面(服务端会话由 TTL 兜底)
    }
    onLogout();
  });
  const foot = el(
    "div",
    { class: "side-foot" },
    el("div", { class: "who muted" }, session.displayName ?? "管理员"),
    el("div", { style: "display:flex;gap:6px;margin-top:8px;" }, themeBtn, logout)
  );
  side.append(brand, navBox, foot);

  const title = el("span", { class: "page-title" }, "");
  const top = el("header", { class: "app-top" }, title, el("span", { class: "spacer" }));

  const main = el("main", { class: "app-main" });
  const shell = el("div", { class: "app-shell" }, side, top, main);
  root.replaceChildren(shell);

  let lastActive = "/";
  const refresh = (activePath: string): void => {
    lastActive = activePath;
    for (const [path, a] of links) {
      a.classList.toggle("active", path === activePath);
    }
    for (const item of nav) {
      const a = links.get(item.path);
      const value = item.badge?.();
      let badge = a?.querySelector(".nav-badge");
      if (value !== undefined && value > 0) {
        if (badge === null || badge === undefined) {
          badge = el("span", { class: "nav-badge" });
          a?.append(badge);
        }
        badge.textContent = String(value);
      } else {
        badge?.remove();
      }
    }
  };

  // 007 T053:徽标数值通常随页面轮询变化(如在线聊天 unread-summary),
  // 而 refresh 仅在路由切换时被调用;页面通过该事件请求按当前路由重算徽标。
  const onBadgesChanged = (): void => {
    if (!shell.isConnected) return; // 登出后外壳已卸载:忽略陈旧事件
    refresh(lastActive);
  };
  document.addEventListener("qing:badges-changed", onBadgesChanged);

  return {
    main,
    setTitle(t: string): void {
      title.textContent = t;
    },
    refresh
  };
}
