import { describe, expect, it } from "vitest";
import { rulesDeepLink } from "./model";

describe("商品列表深链(007 T040:关联发货规则跨页跳转)", () => {
  it("构造 /rules?account=&item= 并对参数做 URL 编码", () => {
    expect(rulesDeepLink("acct-1", "item-1")).toBe("/rules?account=acct-1&item=item-1");
    expect(rulesDeepLink("a b", "i/1")).toBe("/rules?account=a+b&item=i%2F1");
  });
});
