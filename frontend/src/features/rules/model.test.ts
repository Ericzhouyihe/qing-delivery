import { describe, expect, it } from "vitest";
import {
  TRIGGER_LABELS,
  actionSummary,
  buildRefIndex,
  buildReplyRulePayload,
  filterReplyRules,
  filterRules,
  parseDefaultReply,
  parseReplyRule,
  parseRuleDto,
  parseRuleList,
  rebuildPayloadForToggle,
  replyKindBadge,
  ruleToFormValue,
  ruleWarnings,
  scopeLabel,
  triggerBadge,
  validateDefaultReplyForm,
  validateReplyRuleForm
} from "./model";
// 变体/边界校验(实现在 catalog/rule-editor,经 model 复用导出)
import {
  buildRulePayload,
  splitSpecValues,
  validateRuleForm,
  type RuleFormValue
} from "../catalog/rule-editor";

const form = (patch: Partial<RuleFormValue> = {}): RuleFormValue => ({
  itemId: "item-1",
  skuKey: "",
  triggerType: "order_paid",
  priority: 100,
  contentSource: "fixed_text",
  content: "链接 https://example.com/d 提取码 ab12",
  cardPoolId: "",
  templateId: "",
  variants: [],
  review: { waitHours: 24, intervalHours: 24, maxCount: 1, text: "" },
  allItemsConfirmed: false,
  enabled: true,
  ...patch
});

// ===== RuleDto 解析与降级(contracts §3;缺失字段按后端列缺省降级)=====

describe("RuleDto 解析/降级", () => {
  it("完整字段解析(含变体与求评配置)", () => {
    const dto = parseRuleDto({
      id: "rul-1",
      account_id: "acct-1",
      item_id: "",
      sku_key: "",
      enabled: true,
      version: 3,
      trigger_type: "review_missing_timeout",
      priority: 50,
      all_items_confirmed: true,
      needs_reconfiguration: false,
      content_source: "card_pool",
      card_pool_id: "pool-1",
      template_id: null,
      variants: [
        {
          spec_name: "颜色",
          spec_values: ["红色", "蓝色"],
          source: "card_pool",
          card_pool_id: "pool-red",
          template_id: null,
          units_per_item: 2,
          delay_override_seconds: 60
        }
      ],
      review_config: { wait_hours: 24, interval_hours: 12, max_count: 3, text: "求评文案" }
    });
    expect(dto.triggerType).toBe("review_missing_timeout");
    expect(dto.priority).toBe(50);
    expect(dto.allItemsConfirmed).toBe(true);
    expect(dto.variants).toHaveLength(1);
    expect(dto.variants[0]?.specValues).toEqual(["红色", "蓝色"]);
    expect(dto.variants[0]?.unitsPerItem).toBe(2);
    expect(dto.reviewConfig?.maxCount).toBe(3);
  });

  it("字段缺失/形状不对按缺省降级(001 存量语义),非对象抛错", () => {
    const legacy = parseRuleDto({ id: "rul-2", account_id: "acct-1", item_id: "item-1", enabled: false });
    expect(legacy.triggerType).toBe("order_paid");
    expect(legacy.priority).toBe(100);
    expect(legacy.contentSource).toBe("fixed_text");
    expect(legacy.variants).toEqual([]);
    expect(legacy.reviewConfig).toBeNull();
    expect(legacy.allItemsConfirmed).toBe(false);
    expect(legacy.needsReconfiguration).toBe(false);
    const badVariant = parseRuleDto({ id: "r", variants: [{}, "junk"] });
    expect(badVariant.variants).toHaveLength(2);
    expect(badVariant.variants[0]?.source).toBe("card_pool");
    expect(badVariant.variants[1]?.unitsPerItem).toBe(1);
    expect(() => parseRuleDto("junk")).toThrow();
    expect(() => parseRuleDto(null)).toThrow();
  });

  it("规则列表解析:trigger_counts 汇总缺失降级空对象", () => {
    const list = parseRuleList({
      items: [{ id: "r1", trigger_type: "order_paid" }, { id: "r2", trigger_type: "review_missing_timeout" }],
      trigger_counts: { order_paid: 3, review_missing_timeout: 1 }
    });
    expect(list.rules).toHaveLength(2);
    expect(list.triggerCounts).toEqual({ order_paid: 3, review_missing_timeout: 1 });
    const noCounts = parseRuleList({ items: [] });
    expect(noCounts.triggerCounts).toEqual({});
    expect(() => parseRuleList({ items: "junk" })).toThrow();
  });
});

// ===== 触发类型徽标映射(未知值安全降级)=====

describe("触发类型徽标映射", () => {
  it("order_paid/review_missing_timeout/buyer_reviewed/未知值", () => {
    expect(triggerBadge("order_paid")).toEqual({ tone: "normal", label: TRIGGER_LABELS["order_paid"]! });
    expect(triggerBadge("review_missing_timeout").label).toBe("超时未评价求评价");
    expect(triggerBadge("review_missing_timeout").tone).toBe("info");
    expect(triggerBadge("buyer_reviewed").label).toBe("评价后发赠品");
    expect(triggerBadge("bogus").label).toBe("未知状态");
    expect(triggerBadge("bogus").tone).toBe("neutral");
  });
});

// ===== 动作摘要派生(FR-038 规则卡)=====

const refs = buildRefIndex({
  pools: [{ id: "pool-1", name: "新手卡池" }, { id: "pool-red", name: "红色卡池" }],
  templates: [{ id: "tpl-1", name: "标准发货模板" }],
  items: [{ id: "item-1", title: "网盘资料合集" }, { id: "item-2", title: "教程视频" }]
});

describe("动作摘要与范围标签", () => {
  it("固定文字/卡密组/模板三种规则级来源", () => {
    expect(actionSummary(parseRuleDto({ id: "r", item_id: "item-1", content_source: "fixed_text" }), refs)).toEqual([
      "内容来源:固定文字"
    ]);
    expect(actionSummary(parseRuleDto({ id: "r", item_id: "item-1", content_source: "card_pool", card_pool_id: "pool-1" }), refs)).toContain(
      "内容来源:卡密组「新手卡池」(每件 1 份)"
    );
    // 引用 id 无法解析名称时回退 id 本身(不猜测)
    expect(actionSummary(parseRuleDto({ id: "r", item_id: "item-1", content_source: "card_pool", card_pool_id: "pool-x" }), refs)).toContain(
      "内容来源:卡密组「pool-x」(每件 1 份)"
    );
    expect(actionSummary(parseRuleDto({ id: "r", item_id: "item-1", content_source: "template", template_id: "tpl-1" }), refs)).toContain(
      "内容来源:发货模板「标准发货模板」"
    );
  });

  it("变体规则:逐变体来源/份数/延时;容器默认来源留空", () => {
    const lines = actionSummary(
      parseRuleDto({
        id: "r",
        item_id: "item-1",
        content_source: "card_pool",
        card_pool_id: null,
        variants: [
          { spec_name: "颜色", spec_values: ["红色"], source: "card_pool", card_pool_id: "pool-red", units_per_item: 2, delay_override_seconds: 30 },
          { spec_name: "", spec_values: [], source: "template", template_id: "tpl-1", units_per_item: 1, delay_override_seconds: null }
        ]
      }),
      refs
    );
    expect(lines[0]).toBe("默认来源:未绑定(按变体)");
    expect(lines[1]).toBe("变体 颜色=红色 → 卡密组「红色卡池」 ×2/件 · 延时 30s");
    expect(lines[2]).toBe("变体 兜底=全部规格 → 发货模板「标准发货模板」");
  });

  it("求评规则:等待/间隔/次数/文案摘要;未配置如实标注", () => {
    const lines = actionSummary(
      parseRuleDto({
        id: "r",
        trigger_type: "review_missing_timeout",
        item_id: "",
        review_config: { wait_hours: 24, interval_hours: 12, max_count: 3, text: "  感谢购买,\n期待您的评价~  " }
      }),
      refs
    );
    expect(lines[0]).toBe("等待 24h · 间隔 12h · 最多 3 次");
    expect(lines[1]).toContain("文案:感谢购买, 期待您的评价~");
    expect(actionSummary(parseRuleDto({ id: "r", trigger_type: "review_missing_timeout" }), refs)).toEqual(["求评计划未配置"]);
  });

  it("范围标签与警示(账号级未确认/引用缺失)", () => {
    const account = parseRuleDto({ id: "r", item_id: "", sku_key: "" });
    expect(scopeLabel(account, refs)).toBe("账号级(全部商品)");
    expect(scopeLabel(parseRuleDto({ id: "r", item_id: "item-1", sku_key: "combo:p1=v1" }), refs)).toBe("网盘资料合集 · combo:p1=v1");
    expect(ruleWarnings(account)).toEqual([{ kind: "needs_confirmation", label: "需确认·暂不发货" }]);
    expect(ruleWarnings(parseRuleDto({ id: "r", item_id: "", all_items_confirmed: true }))).toEqual([]);
    expect(ruleWarnings(parseRuleDto({ id: "r", item_id: "item-1", needs_reconfiguration: true }))).toEqual([
      { kind: "needs_reconfiguration", label: "需重新配置" }
    ]);
  });
});

// ===== 筛选纯函数(本地即时过滤)=====

describe("规则筛选", () => {
  const rules = [
    parseRuleDto({ id: "r-1", item_id: "item-1", enabled: true, trigger_type: "order_paid" }),
    parseRuleDto({ id: "r-2", item_id: "item-2", enabled: false, trigger_type: "order_paid" }),
    parseRuleDto({ id: "r-3", item_id: "", enabled: true, trigger_type: "review_missing_timeout" })
  ];
  it("按触发类型/状态/关键词组合", () => {
    expect(filterRules(rules, { trigger: "", status: "all", query: "" }, refs)).toHaveLength(3);
    expect(filterRules(rules, { trigger: "order_paid", status: "all", query: "" }, refs)).toHaveLength(2);
    expect(filterRules(rules, { trigger: "", status: "enabled", query: "" }, refs)).toHaveLength(2);
    expect(filterRules(rules, { trigger: "", status: "disabled", query: "" }, refs)).toHaveLength(1);
    // 关键词命中商品标题
    expect(filterRules(rules, { trigger: "", status: "all", query: "教程" }, refs).map((r) => r.id)).toEqual(["r-2"]);
    // 关键词命中规则 id
    expect(filterRules(rules, { trigger: "", status: "all", query: "r-3" }, refs).map((r) => r.id)).toEqual(["r-3"]);
    // 关键词命中范围标签(账号级)
    expect(filterRules(rules, { trigger: "", status: "all", query: "账号级" }, refs).map((r) => r.id)).toEqual(["r-3"]);
    expect(filterRules(rules, { trigger: "", status: "all", query: "不存在" }, refs)).toEqual([]);
  });
});

// ===== 变体校验与负载组装(catalog/rule-editor 实现,此处复用验证)=====

describe("变体校验(与后端 assemble 同口径)", () => {
  const variant = (patch: Partial<RuleFormValue["variants"][number]> = {}): RuleFormValue["variants"][number] => ({
    specName: "颜色",
    specValues: "红色,蓝色",
    source: "card_pool",
    cardPoolId: "pool-1",
    templateId: "",
    unitsPerItem: 2,
    delayOverride: "",
    ...patch
  });

  it("合法表单零错误", () => {
    expect(validateRuleForm(form())).toEqual([]);
    // 带变体 = 容器:规则级卡密绑定可留空(各变体独立来源)
    expect(
      validateRuleForm(form({ contentSource: "card_pool", cardPoolId: "", content: "", variants: [variant()] }))
    ).toEqual([]);
  });

  it("优先级边界 1..=10000", () => {
    expect(validateRuleForm(form({ priority: 0 })).length).toBe(1);
    expect(validateRuleForm(form({ priority: 10001 })).length).toBe(1);
    expect(validateRuleForm(form({ priority: 1 }))).toEqual([]);
    expect(validateRuleForm(form({ priority: 10000 }))).toEqual([]);
  });

  it("来源互斥与必填:卡密/模板须绑定或带变体;固定文字须非空", () => {
    expect(validateRuleForm(form({ contentSource: "card_pool", cardPoolId: "", content: "" }))[0]).toContain("卡密库存来源须选择卡密组");
    expect(validateRuleForm(form({ contentSource: "template", templateId: "", content: "" }))[0]).toContain("发货模板来源须选择模板");
    expect(validateRuleForm(form({ content: "  " }))[0]).toBe("内容不能为空");
  });

  it("变体:规格名/值成对、份数 1..=100、延时 0..=3600、来源绑定", () => {
    expect(validateRuleForm(form({ variants: [variant({ specValues: "" })] }))[0]).toContain("成对出现");
    expect(validateRuleForm(form({ variants: [variant({ specName: "" })] }))[0]).toContain("成对出现");
    // 全空 = 兜底变体,合法
    expect(validateRuleForm(form({ variants: [variant({ specName: "", specValues: "" })] }))).toEqual([]);
    expect(validateRuleForm(form({ variants: [variant({ unitsPerItem: 0 })] }))[0]).toContain("每件份数须为 1–100");
    expect(validateRuleForm(form({ variants: [variant({ unitsPerItem: 101 })] }))[0]).toContain("每件份数须为 1–100");
    expect(validateRuleForm(form({ variants: [variant({ delayOverride: "-1" })] }))[0]).toContain("延时覆盖须为 0–3600 秒");
    expect(validateRuleForm(form({ variants: [variant({ delayOverride: "3601" })] }))[0]).toContain("延时覆盖须为 0–3600 秒");
    expect(validateRuleForm(form({ variants: [variant({ cardPoolId: "" })] }))[0]).toContain("须选择卡密组");
    expect(validateRuleForm(form({ variants: [variant({ source: "template", templateId: "", cardPoolId: "pool-1" })] }))[0]).toContain("须选择模板");
  });

  it("求评配置边界:wait/interval ≥1、次数 1..=10、文案非空", () => {
    const review = form({ triggerType: "review_missing_timeout" });
    expect(validateRuleForm(review)[0]).toContain("求评文案不能为空");
    expect(
      validateRuleForm(form({ triggerType: "review_missing_timeout", review: { waitHours: 0, intervalHours: 12, maxCount: 3, text: "x" } }))
    ).toContain("发货后等待小时数须 ≥1");
    expect(
      validateRuleForm(form({ triggerType: "review_missing_timeout", review: { waitHours: 24, intervalHours: 0, maxCount: 3, text: "x" } }))
    ).toContain("再次求评间隔小时数须 ≥1");
    expect(
      validateRuleForm(form({ triggerType: "review_missing_timeout", review: { waitHours: 24, intervalHours: 12, maxCount: 11, text: "x" } }))
    ).toContain("最多次数须为 1–10");
  });

  it("规格值拆分:逗号/中文逗号/顿号/分号,trim 去空去重", () => {
    expect(splitSpecValues("红色, 蓝色、绿色;黄色,红色")).toEqual(["红色", "蓝色", "绿色", "黄色"]);
    expect(splitSpecValues("  ,,、;  ")).toEqual([]);
    expect(splitSpecValues("单值")).toEqual(["单值"]);
  });
});

describe("保存负载组装(contracts §3)", () => {
  it("付款固定文字:旧别名+全字段;expected_version 仅更新携带", () => {
    const payload = buildRulePayload(form(), null);
    expect(payload["content_kind"]).toBe("fixed_text");
    expect(payload["content_source"]).toBe("fixed_text");
    expect(typeof payload["content"]).toBe("string");
    expect(payload["trigger_type"]).toBe("order_paid");
    expect(payload["all_items_confirmed"]).toBe(false);
    expect("expected_version" in payload).toBe(false);
    const updated = buildRulePayload(form(), 7);
    expect(updated["expected_version"]).toBe(7);
  });

  it("账号级确认仅在 item_id 为空时携带", () => {
    expect(buildRulePayload(form({ itemId: "", allItemsConfirmed: true }), null)["all_items_confirmed"]).toBe(true);
    expect(buildRulePayload(form({ allItemsConfirmed: true }), null)["all_items_confirmed"]).toBe(false);
  });

  it("变体:spec_values 拆分入负载、延时空= null、来源互斥字段清空", () => {
    const payload = buildRulePayload(
      form({
        contentSource: "card_pool",
        content: "",
        cardPoolId: "",
        variants: [
          { specName: "颜色", specValues: "红色,蓝色", source: "card_pool", cardPoolId: "pool-1", templateId: "", unitsPerItem: 3, delayOverride: "" },
          { specName: "套餐", specValues: "基础版", source: "template", cardPoolId: "", templateId: "tpl-1", unitsPerItem: 1, delayOverride: "45" }
        ]
      }),
      null
    );
    const variants = payload["variants"] as Array<Record<string, unknown>>;
    expect(variants[0]?.["spec_values"]).toEqual(["红色", "蓝色"]);
    expect(variants[0]?.["delay_override_seconds"]).toBeNull();
    expect(variants[1]?.["delay_override_seconds"]).toBe(45);
    expect(variants[1]?.["card_pool_id"]).toBe("");
    expect(variants[0]?.["template_id"]).toBe("");
  });

  it("求评触发:content=求评文案、review_config 四字段、变体清空", () => {
    const payload = buildRulePayload(
      form({
        triggerType: "review_missing_timeout",
        itemId: "",
        content: "",
        variants: [{ specName: "x", specValues: "y", source: "card_pool", cardPoolId: "p", templateId: "", unitsPerItem: 1, delayOverride: "" }],
        review: { waitHours: 24, intervalHours: 12, maxCount: 3, text: "求评文案" }
      }),
      null
    );
    expect(payload["content"]).toBe("求评文案");
    expect(payload["variants"]).toEqual([]);
    expect(payload["review_config"]).toEqual({ wait_hours: 24, interval_hours: 12, max_count: 3, text: "求评文案" });
  });
});

// ===== 快捷启停负载重建(固定文字规则不可重建 → null)=====

describe("rebuildPayloadForToggle", () => {
  it("固定文字付款规则返回 null(正文不回读,无法整体重建)", () => {
    expect(rebuildPayloadForToggle(parseRuleDto({ id: "r", item_id: "i", content_source: "fixed_text" }), true)).toBeNull();
  });

  it("卡密/模板/求评规则重建全量负载并翻转启用、携带乐观锁", () => {
    const dto = parseRuleDto({
      id: "r",
      item_id: "i",
      version: 5,
      content_source: "card_pool",
      card_pool_id: "pool-1",
      trigger_type: "order_paid",
      priority: 20,
      variants: [{ spec_name: "颜色", spec_values: ["红"], source: "card_pool", card_pool_id: "pool-1", units_per_item: 2 }],
      all_items_confirmed: false
    });
    const payload = rebuildPayloadForToggle(dto, false);
    expect(payload).not.toBeNull();
    expect(payload!["enabled"]).toBe(false);
    expect(payload!["expected_version"]).toBe(5);
    expect(payload!["priority"]).toBe(20);
    expect((payload!["variants"] as Array<Record<string, unknown>>)[0]?.["units_per_item"]).toBe(2);
    const review = parseRuleDto({
      id: "r",
      trigger_type: "review_missing_timeout",
      review_config: { wait_hours: 1, interval_hours: 1, max_count: 1, text: "t" }
    });
    expect(rebuildPayloadForToggle(review, true)!["review_config"]).toEqual({ wait_hours: 1, interval_hours: 1, max_count: 1, text: "t" });
    // 求评规则 content=求评文案(固定内容非空校验),非空字符串
    expect(rebuildPayloadForToggle(review, true)!["content"]).toBe("t");
  });
});

describe("ruleToFormValue(RuleDto → 表单初值)", () => {
  it("变体/求评/优先级/确认位回填;正文不回读为空", () => {
    const value = ruleToFormValue(
      parseRuleDto({
        id: "r",
        item_id: "",
        trigger_type: "review_missing_timeout",
        priority: 9,
        all_items_confirmed: true,
        review_config: { wait_hours: 6, interval_hours: 3, max_count: 2, text: "求评" }
      })
    );
    expect(value.triggerType).toBe("review_missing_timeout");
    expect(value.priority).toBe(9);
    expect(value.allItemsConfirmed).toBe(true);
    expect(value.review.text).toBe("求评");
    expect(value.content).toBe("");
    const variants = ruleToFormValue(
      parseRuleDto({ id: "r", item_id: "i", content_source: "card_pool", card_pool_id: "p", variants: [{ spec_name: "颜色", spec_values: ["红", "蓝"], source: "card_pool", card_pool_id: "p", units_per_item: 4, delay_override_seconds: 90 }] })
    );
    expect(variants.variants[0]?.specValues).toBe("红,蓝");
    expect(variants.variants[0]?.delayOverride).toBe("90");
  });
});

// ===== 关键词回复契约与校验 =====

describe("关键词回复", () => {
  it("解析与降级;类型徽标未知值安全", () => {
    const dto = parseReplyRule({
      id: "rr-1",
      account_id: "acct-1",
      keyword: "套餐",
      reply_kind: "text",
      reply_text: "已收到,发你",
      reply_image_url: null,
      enabled: true,
      item_ids: ["item-1", "item-2"]
    });
    expect(dto.itemIds).toEqual(["item-1", "item-2"]);
    expect(parseReplyRule({ id: "rr" }).itemIds).toEqual([]);
    expect(parseReplyRule({ id: "rr" }).replyKind).toBe("text");
    expect(() => parseReplyRule([])).toThrow();
    expect(replyKindBadge("text").label).toBe("文字");
    expect(replyKindBadge("image").label).toBe("图片");
    expect(replyKindBadge("gif").label).toBe("未知类型");
  });

  it("表单校验(关键词必填≤100;text/image 内容成对)", () => {
    expect(validateReplyRuleForm({ keyword: "  ", replyKind: "text", replyText: "x", replyImageUrl: "", enabled: true, itemIds: [] })[0]).toContain("关键词不能为空");
    expect(validateReplyRuleForm({ keyword: "词".repeat(101), replyKind: "text", replyText: "x", replyImageUrl: "", enabled: true, itemIds: [] })[0]).toContain("关键词过长");
    expect(validateReplyRuleForm({ keyword: "套餐", replyKind: "text", replyText: " ", replyImageUrl: "", enabled: true, itemIds: [] })[0]).toContain("必须填写回复文案");
    expect(validateReplyRuleForm({ keyword: "套餐", replyKind: "image", replyText: "", replyImageUrl: " ", enabled: true, itemIds: [] })[0]).toContain("必须填写图片链接");
    expect(validateReplyRuleForm({ keyword: "套餐", replyKind: "image", replyText: "", replyImageUrl: "https://x/1.png", enabled: true, itemIds: [] })).toEqual([]);
    expect(validateReplyRuleForm({ keyword: "套餐", replyKind: "text", replyText: "字".repeat(2001), replyImageUrl: "", enabled: true, itemIds: [] })[0]).toContain("回复文案过长");
  });

  it("负载组装与筛选(关键词/关联商品命中)", () => {
    const payload = buildReplyRulePayload({ keyword: " 套餐 ", replyKind: "image", replyText: "", replyImageUrl: " https://x/1.png ", enabled: false, itemIds: ["item-1"] });
    expect(payload).toEqual({ keyword: "套餐", reply_kind: "image", reply_text: null, reply_image_url: "https://x/1.png", enabled: false, item_ids: ["item-1"] });
    const rows = [
      parseReplyRule({ id: "a", keyword: "套餐", item_ids: ["item-2"] }),
      parseReplyRule({ id: "b", keyword: "发货", item_ids: [] })
    ];
    expect(filterReplyRules(rows, "套餐", refs).map((r) => r.id)).toEqual(["a"]);
    // 关联商品标题命中(item-2 = 教程视频)
    expect(filterReplyRules(rows, "教程", refs).map((r) => r.id)).toEqual(["a"]);
    expect(filterReplyRules(rows, "账号级", refs).map((r) => r.id)).toEqual(["b"]);
    expect(filterReplyRules(rows, "", refs)).toHaveLength(2);
  });
});

// ===== 账号默认回复契约与校验 =====

describe("账号默认回复", () => {
  it("解析含最近记录;降级路径", () => {
    const dto = parseDefaultReply({
      account_id: "acct-1",
      enabled: true,
      reply_text: "在的",
      reply_image_url: null,
      reply_once: false,
      updated_at: "2026-09-26T00:00:00Z",
      recent_records: [{ id: "l1", buyer_id: "b1", state: "sent", sent_at: "t" }, "junk"]
    });
    expect(dto.replyOnce).toBe(false);
    expect(dto.recentRecords).toHaveLength(2);
    expect(dto.recentRecords[0]?.buyerId).toBe("b1");
    expect(dto.recentRecords[1]?.state).toBe("");
    expect(parseDefaultReply({ account_id: "a" }).recentRecords).toEqual([]);
    expect(() => parseDefaultReply(3)).toThrow();
  });

  it("表单校验:启用须有内容;长度上限", () => {
    expect(validateDefaultReplyForm({ enabled: true, replyText: "", replyImageUrl: "" })[0]).toContain("必须填写文字或图片内容");
    expect(validateDefaultReplyForm({ enabled: false, replyText: "", replyImageUrl: "" })).toEqual([]);
    expect(validateDefaultReplyForm({ enabled: true, replyText: "x".repeat(2001), replyImageUrl: "" })[0]).toContain("默认回复过长");
  });
});
