/** 007 T040/T041 页面冒烟:三页签装配、未选账号禁用主按钮、账号选择后装载
 *  交易规则卡(徽标/摘要/优先级)、关键词列表、默认回复卡。fetch 全 mock;
 *  只验证装配与既有端点调用形态,不测业务判定(在 model.test.ts)。 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { rulesPage } from "./page";

/** PageFactory 返回值可能为异步/void;仅当为函数时调用清理。 */
const teardown = (ret: unknown): void => {
  if (typeof ret === "function") (ret as () => void)();
};

const json = (body: unknown): Response =>
  new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });

describe("rulesPage 装配冒烟(拆分后 /rules 页可用)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.replaceState(null, "", "/rules");
  });

  it("三页签齐全;未选账号主按钮禁用;选账号后三页签数据可装载", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL): Promise<Response> => {
      const url = String(input);
      if (url.includes("/reply-rules")) {
        return Promise.resolve(
          json({ items: [{ id: "rr-1", account_id: "acct-1", keyword: "套餐", reply_kind: "text", reply_text: "发货啦", enabled: true, item_ids: [] }] })
        );
      }
      if (url.includes("/default-reply")) {
        return Promise.resolve(
          json({ account_id: "acct-1", enabled: true, reply_text: "在的", reply_image_url: null, reply_once: true, updated_at: "", recent_records: [] })
        );
      }
      if (url.includes("/rules")) {
        return Promise.resolve(
          json({
            items: [
              {
                id: "rul-1", account_id: "acct-1", item_id: "item-1", sku_key: "", enabled: true, version: 2,
                trigger_type: "order_paid", priority: 100, all_items_confirmed: false, needs_reconfiguration: false,
                content_source: "card_pool", card_pool_id: "pool-1", template_id: null,
                variants: [{ spec_name: "颜色", spec_values: ["红"], source: "card_pool", card_pool_id: "pool-1", units_per_item: 2 }]
              }
            ],
            trigger_counts: { order_paid: 1 }
          })
        );
      }
      if (url.includes("/card-pools")) {
        return Promise.resolve(json({ items: [{ id: "pool-1", name: "新手卡池", kind: "data", enabled: true, stock: { available: 3, reserved: 0, used: 0 } }] }));
      }
      if (url.includes("/delivery-templates")) {
        return Promise.resolve(
          json({ items: [{ id: "tpl-1", name: "标准模板", enabled: true, messages: ["a"], keys: { cards: [], custom: [] }, used_by_rules: 0, version: 1 }] })
        );
      }
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
    const dispose = rulesPage(root);
    await vi.waitFor(() => {
      expect(root.textContent).toContain("交易自动化");
      expect(root.textContent).toContain("关键词回复");
      expect(root.textContent).toContain("账号默认回复");
    });
    // 未选账号:主按钮禁用 + 引导空态(先等账号装载完成)
    await vi.waitFor(() => {
      expect(root.textContent).toContain("测试账号");
    });
    const addBtn = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "新建规则") as HTMLButtonElement;
    expect(addBtn.disabled).toBe(true);
    expect(root.textContent).toContain("请先选择账号");
    // 选择账号 → 装载规则卡(触发类型徽标 + 关联商品 + 优先级 + 变体摘要)
    const accountSel = root.querySelector("select") as HTMLSelectElement;
    accountSel.value = "acct-1";
    accountSel.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => {
      expect(root.textContent).toContain("付款后自动发货");
      expect(root.textContent).toContain("优先级 100");
      expect(root.textContent).toContain("变体 颜色=红 → 卡密组「新手卡池」 ×2/件");
    });
    // 关键词页签
    const kwTab = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "关键词回复")!;
    kwTab.click();
    await vi.waitFor(() => {
      expect(root.textContent).toContain("套餐");
      expect(root.textContent).toContain("账号级");
    });
    // 默认回复页签(每账号一张卡)
    const defTab = Array.from(root.querySelectorAll("button")).find((b) => b.textContent === "账号默认回复")!;
    defTab.click();
    await vi.waitFor(() => {
      expect(root.textContent).toContain("测试账号");
      expect(root.textContent).toContain("只回复一次");
      expect(root.textContent).toContain("在的");
    });
    expect(typeof dispose).toBe("function");
    teardown(dispose);
    root.remove();
  });
});
