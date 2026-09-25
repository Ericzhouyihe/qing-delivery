import { afterEach, describe, expect, it, vi } from "vitest";
import { createSegmented } from "./segmented";

describe("分段图标切换控件(006 契约 §2.1,FR-009)", () => {
  const options = [
    { value: "row", icon: "list", label: "列表视图" },
    { value: "card", icon: "grid", label: "卡片视图" }
  ] as const;

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("容器 role=group 且 aria-label=视图切换", () => {
    const node = createSegmented({ options, value: "row", onSelect: () => {} });
    expect(node.getAttribute("role")).toBe("group");
    expect(node.getAttribute("aria-label")).toBe("视图切换");
    expect(node.classList.contains("segmented")).toBe(true);
  });

  it("选项为仅图标按钮:title=label,内容无文字节点", () => {
    const node = createSegmented({ options, value: "row", onSelect: () => {} });
    const buttons = node.querySelectorAll("button");
    expect(buttons.length).toBe(2);
    for (const b of buttons) {
      expect(b.getAttribute("type")).toBe("button");
      expect(options.map((o) => o.label)).toContain(b.title);
      expect(b.textContent?.trim()).toBe(""); // 仅图标,不显示文字
      expect(b.querySelector("svg")).not.toBeNull();
    }
    expect(buttons[0]?.title).toBe("列表视图");
    expect(buttons[1]?.title).toBe("卡片视图");
  });

  it("初始激活项 aria-pressed=true 且带高亮类,其余 false", () => {
    const node = createSegmented({ options, value: "card", onSelect: () => {} });
    const buttons = node.querySelectorAll("button");
    expect(buttons[0]?.getAttribute("aria-pressed")).toBe("false");
    expect(buttons[1]?.getAttribute("aria-pressed")).toBe("true");
    expect(buttons[1]?.classList.contains("active")).toBe(true);
    expect(buttons[0]?.classList.contains("active")).toBe(false);
  });

  it("点击未激活项触发 onSelect 并迁移激活态", () => {
    const onSelect = vi.fn();
    const node = createSegmented({ options, value: "row", onSelect });
    const buttons = node.querySelectorAll("button");
    (buttons[1] as HTMLButtonElement).click();
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("card");
    expect(buttons[1]?.getAttribute("aria-pressed")).toBe("true");
    expect(buttons[0]?.getAttribute("aria-pressed")).toBe("false");
  });

  it("点击已激活项不触发(幂等)", () => {
    const onSelect = vi.fn();
    const node = createSegmented({ options, value: "row", onSelect });
    const buttons = node.querySelectorAll("button");
    (buttons[0] as HTMLButtonElement).click();
    (buttons[0] as HTMLButtonElement).click();
    expect(onSelect).not.toHaveBeenCalled();
  });
});
