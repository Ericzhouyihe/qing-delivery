/** 三轴状态时间线(US5/T041):节点 = 时间 + 轴 + 状态 + 语义色。 */
import { el } from "../shared/dom";
import type { Tone } from "./badge";

export interface TimelineNode {
  time: string | null;
  axis: "订单" | "内容交付" | "审核" | "平台确认";
  label: string;
  tone: Tone;
}

export function renderTimeline(nodes: TimelineNode[]): HTMLElement {
  const list = el("ul", { class: "timeline" });
  if (nodes.length === 0) {
    list.append(el("li", {}, el("span", { class: "muted" }, "暂无状态节点")));
    return list;
  }
  for (const n of nodes) {
    list.append(
      el(
        "li",
        { class: `tl-${n.tone}` },
        el(
          "div",
          {},
          el("span", { class: `badge badge-${n.tone}` }, n.label),
          el("span", { class: "muted", style: "margin-left:8px;font-size:var(--text-xs);" }, n.axis)
        ),
        n.time === null ? el("div", { class: "tl-time" }, "时间未知") : el("div", { class: "tl-time" }, n.time)
      )
    );
  }
  return list;
}
