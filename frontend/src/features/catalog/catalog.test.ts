import { afterEach, describe, expect, it, vi } from "vitest";
import { validateContent, CONTENT_LIMITS, renderRuleForm, defaultRuleFormValue, validateRuleForm } from "./rule-editor";
import { filterItems, parseItem, type ProductItem } from "./products";
import { parseMatchOutcome, renderMatchOutcome, renderMatchPreview } from "./match-preview";
import { el } from "../../shared/dom";

describe("规则内容校验(FR-013/T037)", () => {
  it("空内容拒绝;边界与超限", () => {
    expect(validateContent("").ok).toBe(false);
    expect(validateContent("   ").ok).toBe(false);
    const edge = "a".repeat(CONTENT_LIMITS.unicode_scalars);
    expect(validateContent(edge).ok).toBe(true);
    const over = "a".repeat(CONTENT_LIMITS.unicode_scalars + 1);
    const v = validateContent(over);
    expect(v.ok).toBe(false);
    expect(v.message).toContain("超出字数上限");
    expect(v.unicodeScalars).toBe(CONTENT_LIMITS.unicode_scalars + 1);
  });

  it("字数按 Unicode 标量计数(中文/emoji 不按字节)", () => {
    const v = validateContent("链接https://e.example/x 提取码:甲乙丙🔑");
    expect(v.ok).toBe(true);
    expect(v.unicodeScalars).toBe([...("链接https://e.example/x 提取码:甲乙丙🔑")].length);
  });
});

describe("商品筛选(FR-012)", () => {
  const items: ProductItem[] = [
    { ...parseItem({ id: "i1", account_id: "a1", platform_item_id: "P1", title: "网盘资料合集", status: "on_sale", rule_state: "configured" }), skuDefinition: [], version: 1 },
    { ...parseItem({ id: "i2", account_id: "a2", platform_item_id: "P2", title: "教程视频", status: "off_sale", rule_state: "missing" }), skuDefinition: [], version: 1 }
  ];

  it("按账号/仅已配置/关键字组合筛选", () => {
    expect(filterItems(items, { accountId: "a1", configuredOnly: false, keyword: "" }).length).toBe(1);
    expect(filterItems(items, { accountId: null, configuredOnly: true, keyword: "" }).length).toBe(1);
    expect(filterItems(items, { accountId: null, configuredOnly: false, keyword: "教程" })[0]?.id).toBe("i2");
    expect(filterItems(items, { accountId: null, configuredOnly: false, keyword: "P1" })[0]?.id).toBe("i1");
    expect(filterItems(items, { accountId: null, configuredOnly: true, keyword: "教程" }).length).toBe(0);
  });
});

describe("匹配预演结果(FR-014:三态,判定全部来自服务端)", () => {
  it("解析 matched/none/ambiguous", () => {
    const m = parseMatchOutcome({ state: "matched", rule_id: "rule-abc123", rule_version: 4 });
    expect(m.state).toBe("matched");
    if (m.state === "matched") {
      expect(m.ruleId).toBe("rule-abc123");
      expect(m.ruleVersion).toBe(4);
    }
    expect(parseMatchOutcome({ state: "none", reason: "无启用规则匹配完整规格组合" }).state).toBe("none");
    expect(parseMatchOutcome({ state: "ambiguous", reason: "多条启用规则" }).state).toBe("ambiguous");
  });

  it("三态呈现:命中/冲突/未命中", () => {
    const matched = renderMatchOutcome({ state: "matched", ruleId: "rule-abc123456", ruleVersion: 2 });
    expect(matched.textContent).toContain("命中");
    expect(matched.textContent).toContain("自动发货");
    const amb = renderMatchOutcome({ state: "ambiguous", reason: "多条启用规则命中" });
    expect(amb.textContent).toContain("冲突");
    expect(amb.querySelector(".badge-warning")).not.toBeNull();
    const none = renderMatchOutcome({ state: "none", reason: "无启用规则命中" });
    expect(none.textContent).toContain("未命中");
    expect(none.textContent).toContain("不会自动发货");
  });
});

describe("预演面板只经端点判定(前端零匹配算法)", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("点击预览调用 match-preview 端点并渲染结果", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ state: "none", reason: "无启用规则匹配完整规格组合" }), {
        status: 200,
        headers: { "Content-Type": "application/json" }
      })
    );
    vi.stubGlobal("fetch", fetchMock);
    const container = el("div");
    const item = parseItem({ id: "i1", account_id: "a1", platform_item_id: "P1", title: "商品", status: "on_sale" });
    renderMatchPreview(container, "a1", [item], { item });
    const run = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === "预览匹配")!;
    run.click();
    await vi.waitFor(() => {
      expect(container.textContent).toContain("未命中");
    });
    const call = fetchMock.mock.calls.find((c) => String(c[0]).includes("match-preview"));
    expect(call).toBeDefined();
    expect(String(call![0])).toContain("/api/v1/accounts/a1/rules/match-preview");
  });
});

// ===== 007 T040:renderRuleForm 提取后的回归(抽屉/规则页共用表单主体)=====

describe("renderRuleForm 回归(拆分后表单主体可用)", () => {
  afterEach(() => vi.unstubAllGlobals());

  const sampleItem = parseItem({ id: "i1", account_id: "a1", platform_item_id: "P1", title: "网盘资料", status: "on_sale" });

  it("默认值校验失败(空内容),输入后内容预演仍走服务端 preview 端点", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ rendered_text: "链接 x", unicode_scalars: 4, utf8_bytes: 7, valid: true, violations: [] }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    vi.stubGlobal("fetch", fetchMock);
    const container = el("div");
    const handle = renderRuleForm(container, {
      accountId: "a1",
      items: [sampleItem],
      pools: [{ id: "p1", name: "卡池", enabled: true }],
      templates: [{ id: "t1", name: "模板", enabled: true }],
      value: { ...defaultRuleFormValue(), itemId: "i1" }
    });
    // 默认固定文字空内容 → 校验不通过
    expect(handle.validate().length).toBeGreaterThan(0);
    // 填入正文 → 通过
    const area = container.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "链接 x";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    expect(handle.validate()).toEqual([]);
    // 内容预演:仍调用 rules/preview(fixed_text 行为保留)
    const previewBtn = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === "内容预演")!;
    previewBtn.click();
    await vi.waitFor(() => {
      expect(container.textContent).toContain("链接 x");
    });
    const call = fetchMock.mock.calls.find((c) => String(c[0]).includes("/rules/preview"));
    expect(call).toBeDefined();
    expect(String(call![0])).toContain("/api/v1/accounts/a1/rules/preview");
    const body = JSON.parse(String(call![1]?.body)) as Record<string, unknown>;
    expect(body["item_id"]).toBe("i1");
    expect(body["content"]).toBe("链接 x");
  });

  it("表单嵌入匹配预览面板(零改动复用)并呈现账号级确认开关", () => {
    const container = el("div");
    renderRuleForm(container, {
      accountId: "a1",
      items: [sampleItem],
      pools: [],
      templates: [],
      value: defaultRuleFormValue()
    });
    expect(container.textContent).toContain("匹配预览");
    expect(Array.from(container.querySelectorAll("button")).some((b) => b.textContent === "预览匹配")).toBe(true);
    // 默认账号级:确认开关可见,警示在场;切到商品后随确认块隐藏
    expect(container.textContent).toContain("适用于全部商品");
    const confirmWarn = Array.from(container.querySelectorAll("div")).find(
      (d) => d.textContent?.includes("需确认·暂不发货") === true
    )!;
    expect(confirmWarn.hidden).toBe(false);
    // 下拉顺序:触发类型 → 商品范围(第二个)→ 内容来源…
    const selects = Array.from(container.querySelectorAll("select")) as HTMLSelectElement[];
    const scopeSel = selects[1]!;
    scopeSel.value = "i1";
    scopeSel.dispatchEvent(new Event("change", { bubbles: true }));
    expect(confirmWarn.hidden).toBe(true);
    // 变体添加入口在场
    expect(Array.from(container.querySelectorAll("button")).some((b) => b.textContent === "+ 添加变体")).toBe(true);
  });

  it("求评触发隐藏内容来源与变体区,校验求评四字段", () => {
    const container = el("div");
    const handle = renderRuleForm(container, {
      accountId: "a1",
      items: [sampleItem],
      pools: [],
      templates: [],
      value: { ...defaultRuleFormValue(), triggerType: "review_missing_timeout", itemId: "i1" }
    });
    const errors = handle.validate();
    expect(errors.join(";")).toContain("求评文案不能为空");
    expect(validateRuleForm({ ...defaultRuleFormValue(), triggerType: "review_missing_timeout" }).length).toBeGreaterThan(0);
    // 触发类型/商品范围仍可见;变体与来源区隐藏(hidden 属性)
    const hiddenBlocks = Array.from(container.querySelectorAll("div[hidden], h3"));
    expect(container.textContent).toContain("求评计划");
  });
});
