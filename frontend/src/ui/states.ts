/** 异步块三态(T007,研究 D5/FR-003):骨架屏 → 内容/空态/错误块。
 *  实现为容器内的独立槽位:不要求容器独占(同容器可有工具栏等其他子节点);
 *  每次加载持有递增序号,过期响应不覆盖新一轮加载。 */
import { el } from "../shared/dom";
import { btn } from "./dom";

export interface AsyncBlockOptions {
  /** 骨架行数(决定占位高度,与典型内容高度一致)。 */
  skeletonLines?: number;
  /** 空态渲染:必须包含引导动作(FR-003);缺省提供通用文案。 */
  empty?: () => HTMLElement;
}

export interface AsyncBlockController {
  reload(): void;
  /** 从容器移除本异步块槽位。 */
  dispose(): void;
}

export function asyncBlock(
  container: HTMLElement,
  load: () => Promise<HTMLElement | null>,
  opts: AsyncBlockOptions = {}
): AsyncBlockController {
  const lines = opts.skeletonLines ?? 3;
  const slot = el("div", { class: "async-slot" });
  container.append(slot);
  let seq = 0;

  const renderSkeleton = (): void => {
    const box = el("div", { class: "skeleton-lines", "aria-busy": "true" });
    for (let i = 0; i < lines; i++) {
      const width = i === 0 ? "60%" : i === lines - 1 ? "40%" : "100%";
      const line = el("div", { class: "skeleton" });
      line.style.height = "14px";
      line.style.width = width;
      box.append(line);
    }
    slot.replaceChildren(box);
  };

  const renderEmpty = (): void => {
    if (opts.empty !== undefined) {
      slot.replaceChildren(opts.empty());
      return;
    }
    slot.replaceChildren(
      el("div", { class: "empty-state" }, el("div", { class: "empty-ico" }, "📭"), el("div", {}, "暂无数据"))
    );
  };

  const renderError = (err: unknown): void => {
    const msg = err instanceof Error ? err.message : String(err);
    const text = el("div", {}, `加载失败:${msg}`);
    const retry = btn("重试", { variant: "secondary" });
    const block = el("div", { class: "error-block", role: "alert" }, text, retry);
    retry.addEventListener("click", () => void run());
    slot.replaceChildren(block);
  };

  const run = (): void => {
    const my = ++seq;
    renderSkeleton();
    load()
      .then((node) => {
        if (my !== seq) return; // 已被更新的加载取代
        if (node === null) renderEmpty();
        else slot.replaceChildren(node);
      })
      .catch((err: unknown) => {
        if (my !== seq) return;
        renderError(err);
      });
  };

  run();
  return {
    reload: run,
    dispose(): void {
      seq += 1; // 使在途响应失效
      slot.remove();
    }
  };
}
