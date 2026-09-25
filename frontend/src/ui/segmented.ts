/** 分段图标切换控件(006 契约 §2.1,FR-009):仅图标无文字,激活项高亮,
 *  title 悬停提示,aria-pressed 表达激活态。控件不持久化、不发请求,
 *  偏好读写由调用方负责(可复用、可测)。 */
import { el } from "../shared/dom";
import { icon, type IconName } from "./icons";

export interface SegmentedOption<V extends string> {
  value: V;
  icon: IconName;
  label: string;
}

export function createSegmented<V extends string>(opts: {
  options: ReadonlyArray<SegmentedOption<V>>;
  value: V;
  onSelect: (value: V) => void;
}): HTMLElement {
  const group = el("div", { class: "segmented", role: "group", "aria-label": "视图切换" });
  let current = opts.value;
  const buttons = new Map<V, HTMLButtonElement>();
  const sync = (): void => {
    for (const [value, b] of buttons) {
      const active = value === current;
      b.setAttribute("aria-pressed", active ? "true" : "false");
      b.classList.toggle("active", active);
    }
  };
  for (const o of opts.options) {
    const b = el("button", {
      class: "segmented-item",
      type: "button",
      title: o.label,
      "aria-pressed": "false"
    }) as HTMLButtonElement;
    b.append(icon(o.icon));
    b.addEventListener("click", () => {
      if (o.value === current) return; // 幂等:重复点击当前项不触发
      current = o.value;
      sync();
      opts.onSelect(o.value);
    });
    buttons.set(o.value, b);
    group.append(b);
  }
  sync();
  return group;
}
