/** 007 T040/T041 页面冒烟:商品列表装配、「关联发货规则」跨页深链 /rules?account=&item=。 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { itemsPage } from "./page";

/** PageFactory 返回值可能为异步/void;仅当为函数时调用清理。 */
const teardown = (ret: unknown): void => {
  if (typeof ret === "function") (ret as () => void)();
};

const json = (body: unknown): Response =>
  new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });

describe("itemsPage 装配冒烟(拆分后 /items 页可用)", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("装载商品表;「关联发货规则」生成 /rules 深链;未选账号时同步按钮禁用", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL): Promise<Response> => {
      const url = String(input);
      if (url.includes("/items")) {
        return Promise.resolve(
          json({ items: [{ id: "item-1", account_id: "acct-1", platform_item_id: "P1", title: "网盘资料", status: "on_sale", rule_state: "configured" }] })
        );
      }
      if (url.includes("/accounts")) {
        return Promise.resolve(json({ items: [{ id: "acct-1", display_name: "测试账号" }] }));
      }
      return Promise.resolve(json({}));
    });
    vi.stubGlobal("fetch", fetchMock);
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = itemsPage(root);
    await vi.waitFor(() => {
      expect(root.textContent).toContain("网盘资料");
      expect(root.textContent).toContain("已配置");
    });
    const link = root.querySelector<HTMLAnchorElement>("a.btn[href^='/rules']");
    expect(link).not.toBeNull();
    expect(link!.getAttribute("href")).toBe("/rules?account=acct-1&item=item-1");
    // 未选具体账号:同步按钮禁用并说明原因
    const syncBtn = Array.from(root.querySelectorAll("button")).find((b) => b.textContent?.includes("从平台同步")) as HTMLButtonElement;
    expect(syncBtn.disabled).toBe(true);
    expect(syncBtn.title).toContain("选择具体账号");
    teardown(dispose);
    root.remove();
  });
});
