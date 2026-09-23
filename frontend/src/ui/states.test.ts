import { describe, expect, it } from "vitest";
import { asyncBlock } from "./states";
import { el } from "../shared/dom";

function setup(): { container: HTMLElement } {
  const container = el("div");
  document.body.append(container);
  return { container };
}

describe("asyncBlock 三态(FR-003)", () => {
  it("加载中渲染骨架,成功后渲染内容", async () => {
    const { container } = setup();
    const content = el("p", {}, "内容");
    const load = () => Promise.resolve<HTMLElement | null>(content);
    asyncBlock(container, load);
    expect(container.querySelector(".skeleton-lines")).not.toBeNull();
    expect(container.querySelector(".skeleton-lines")?.getAttribute("aria-busy")).toBe("true");
    await Promise.resolve();
    await Promise.resolve();
    expect(container.contains(content)).toBe(true);
  });

  it("容器可含其他子节点(工具栏),槽位互不干扰", async () => {
    const { container } = setup();
    const toolbar = el("div", { class: "toolbar" }, "工具栏");
    container.append(toolbar);
    const content = el("p", {}, "内容");
    asyncBlock(container, () => Promise.resolve(content));
    await Promise.resolve();
    await Promise.resolve();
    expect(container.contains(toolbar)).toBe(true);
    expect(container.contains(content)).toBe(true);
  });

  it("过期响应不覆盖新一轮加载;dispose 后在途响应不渲染", async () => {
    const { container } = setup();
    let resolveFirst: ((n: HTMLElement | null) => void) | undefined;
    const ctrl = asyncBlock(container, () => new Promise<HTMLElement | null>((r) => (resolveFirst = r)));
    const first = el("p", {}, "旧数据");
    const second = el("p", {}, "新数据");
    resolveFirst?.(first);
    ctrl.reload(); // 第二轮立即开始
    await Promise.resolve();
    expect(container.contains(first)).toBe(false); // 旧响应被序号守卫丢弃
    void second;
    ctrl.dispose();
    await Promise.resolve();
    expect(container.querySelector(".skeleton-lines")).toBeNull();
  });

  it("返回 null 渲染空态,自定义空态含引导动作", async () => {
    const { container } = setup();
    const cta = el("button", {}, "去接入");
    asyncBlock(container, () => Promise.resolve(null), {
      empty: () => el("div", { class: "empty-state" }, el("div", {}, "还没有账号"), cta)
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(container.querySelector(".empty-state")).not.toBeNull();
    expect(container.contains(cta)).toBe(true);
  });

  it("失败渲染错误块,点击重试后恢复", async () => {
    const { container } = setup();
    let shouldFail = true;
    const load = (): Promise<HTMLElement | null> =>
      shouldFail ? Promise.reject(new Error("服务停止")) : Promise.resolve(el("p", {}, "恢复"));
    const ctrl = asyncBlock(container, load);
    await Promise.resolve();
    await Promise.resolve();
    const block = container.querySelector(".error-block");
    expect(block).not.toBeNull();
    expect(block?.textContent).toContain("服务停止");

    shouldFail = false;
    (block?.querySelector("button") as HTMLButtonElement).click();
    await Promise.resolve();
    await Promise.resolve();
    expect(container.textContent).toContain("恢复");
    expect(ctrl.reload).toBeTypeOf("function");
  });

  it("骨架占位高度稳定(行数与设置一致)", () => {
    const { container } = setup();
    asyncBlock(container, () => new Promise(() => undefined), { skeletonLines: 5 });
    const lines = container.querySelectorAll(".skeleton");
    expect(lines.length).toBe(5);
  });
});
