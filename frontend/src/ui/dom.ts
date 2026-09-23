/** UI 组件工厂(T003):基于 shared/dom 的 el() 构建无状态基础构件。
 *  约定:组件只负责呈现与事件接线,业务判定一律来自服务端(宪章 II)。 */
import { el } from "../shared/dom";

export type ButtonVariant = "primary" | "secondary" | "danger" | "ghost";

/** 按钮层级(US1):主/次/危险/幽灵;禁用时必须携带原因(title)。 */
export function btn(
  label: string,
  opts: { variant?: ButtonVariant; disabledReason?: string; type?: "button" | "submit" } = {}
): HTMLButtonElement {
  const node = el("button", { class: `btn btn-${opts.variant ?? "secondary"}`, type: opts.type ?? "button" }, label);
  if (opts.disabledReason !== undefined) {
    node.disabled = true;
    node.title = opts.disabledReason;
    node.setAttribute("aria-disabled", "true");
  }
  return node;
}

/** 卡片:内容区信息组织的基本单位(US1)。 */
export function card(title: string | null, ...children: (Node | string | null | undefined)[]): HTMLElement {
  const body = el("div", { class: "card-body" }, ...children);
  const node = el("section", { class: "card" });
  if (title !== null) node.append(el("h2", { class: "card-title" }, title));
  node.append(body);
  return node;
}

/** 统计卡(US2):数字 + 标签 + 可选下钻链接与强调色。 */
export function statCard(
  label: string,
  value: string | Node,
  opts: { tone?: string; href?: string; sub?: string } = {}
): HTMLElement {
  const valueCls = `stat-value${opts.tone ? ` tone-${opts.tone}` : ""}`;
  const valueNode = el("div", { class: valueCls }, value);
  const node = el("div", { class: "stat-card" }, el("div", { class: "stat-label" }, label), valueNode);
  if (opts.sub !== undefined) node.append(el("div", { class: "stat-sub muted" }, opts.sub));
  if (opts.href !== undefined) {
    const link = el("a", { class: "stat-link", href: opts.href }, "查看 →");
    node.append(link);
    node.classList.add("stat-clickable");
  }
  return node;
}

/** 长文本单元格:截断 + 悬浮全文(边界条款)。 */
export function truncate(text: string, max = 24): HTMLElement {
  const short = text.length > max ? `${text.slice(0, max)}…` : text;
  return el("span", { class: "truncate", title: text }, short);
}

/** 空态占位(骨架之外的引导性空态由 states.ts 统一封装,此为轻量辅助)。 */
export function muted(text: string): HTMLElement {
  return el("p", { class: "muted" }, text);
}
