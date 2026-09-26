/** 发货模板视图模型测试(007 T027):契约解析(含字段缺失降级)、名称过滤、
 *  占位符 keys 提取(cards/custom/系统变量/非法 key)、消息校验边界
 *  (0 条/11 条/超 1000 标量/空行/占位符定位)——与后端 domain/templates.rs 同规则。 */
import { describe, expect, it } from "vitest";
import {
  extractKeys,
  filterTemplates,
  keysLine,
  messagePreview,
  parseTemplate,
  validateMessages,
  validateTemplateName,
  type TemplateDto
} from "./model";

function tpl(over: Partial<TemplateDto> = {}): TemplateDto {
  return {
    id: "dtp-1",
    name: "网课发货",
    enabled: true,
    messages: ["感谢 {{buyer_nickname}}", "卡密:{{cards.key1}}"],
    keys: { cards: ["key1"], custom: [] },
    used_by_rules: 0,
    version: 2,
    ...over
  };
}

describe("parseTemplate 契约解析(T026/T027)", () => {
  it("正常载荷完整解析(transport dto_json 形状)", () => {
    const v = parseTemplate({
      id: "dtp-x",
      name: "多消息模板",
      enabled: false,
      messages: ["第一条", "第二条 {{order_id}}"],
      keys: { cards: ["a", "b"], custom: ["note"] },
      used_by_rules: 3,
      version: 9
    });
    expect(v.name).toBe("多消息模板");
    expect(v.enabled).toBe(false);
    expect(v.messages).toEqual(["第一条", "第二条 {{order_id}}"]);
    expect(v.keys).toEqual({ cards: ["a", "b"], custom: ["note"] });
    expect(v.used_by_rules).toBe(3);
    expect(v.version).toBe(9);
  });

  it("字段缺失降级:keys/messages/used_by_rules 缺失不冒充真实值", () => {
    const v = parseTemplate({ id: "dtp-y", name: "最小载荷" });
    expect(v.messages).toEqual([]);
    expect(v.keys).toEqual({ cards: [], custom: [] });
    expect(v.used_by_rules).toBe(0);
    expect(v.version).toBe(1); // 缺失按 1,不臆造高版本
    expect(v.enabled).toBe(false); // 缺失按 false
  });

  it("keys 形状异常按空数组降级;非对象抛错", () => {
    const v = parseTemplate({ id: "dtp-z", keys: "bad", messages: [1, true] });
    expect(v.keys).toEqual({ cards: [], custom: [] });
    expect(v.messages).toEqual(["1", "true"]); // 逐项 String 化,不丢行
    expect(() => parseTemplate("nope")).toThrow();
    expect(() => parseTemplate(null)).toThrow();
    expect(() => parseTemplate([])).toThrow();
  });
});

describe("名称过滤(T027)", () => {
  const items = [
    tpl({ id: "1", name: "网课发货A" }),
    tpl({ id: "2", name: "网课发货B" }),
    tpl({ id: "3", name: "感谢信模板" }),
    tpl({ id: "4", name: "Express Delivery" })
  ];

  it("不区分大小写子串;空串/空白返回全部;无匹配返回空", () => {
    expect(filterTemplates(items, "")).toHaveLength(4);
    expect(filterTemplates(items, "   ")).toHaveLength(4);
    expect(filterTemplates(items, "网课").map((t) => t.id)).toEqual(["1", "2"]);
    expect(filterTemplates(items, "express").map((t) => t.id)).toEqual(["4"]);
    expect(filterTemplates(items, "不存在")).toEqual([]);
  });
});

describe("extractKeys 占位符提取(T027)", () => {
  it("cards/custom 提取;按出现顺序去重", () => {
    const messages = [
      "{{cards.b}} {{cards.a}}",
      "{{custom.z}} {{cards.b}} {{custom.a}}",
      "{{cards.a}}"
    ];
    expect(extractKeys(messages)).toEqual({
      cards: ["b", "a"],
      custom: ["z", "a"]
    });
  });

  it("系统变量不进 keys;非法/未知占位符跳过不失败(与后端 unwrap_or_default 语义一致)", () => {
    const messages = [
      "{{buyer_nickname}}/{{order_id}}/{{buyer_id}}/{{card_name}}",
      "{{cards.非 法}}", // 非法 key(quickstart US2-2 用例)
      "{{custom.备注}}", // 中文 key 非法
      "{{unknown_var}}", // 未知变量
      "{{cards.}}", // 空 key
      "纯文本无占位符"
    ];
    expect(extractKeys(messages)).toEqual({ cards: [], custom: [] });
  });

  it("合法命名边界:字母/数字/下划线/连字符;首尾空格容错", () => {
    expect(extractKeys(["{{ cards.Key_1-2 }}"])).toEqual({ cards: ["Key_1-2"], custom: [] });
  });

  it("keysLine:拼接 cards./custom. 前缀;无变量为 null", () => {
    expect(keysLine({ cards: ["key1"], custom: ["note"] })).toBe("cards.key1 · custom.note");
    expect(keysLine({ cards: [], custom: [] })).toBeNull();
  });
});

describe("validateMessages 消息校验边界(T027,与服务端同规则)", () => {
  it("0 条拒绝(创建空列表)/11 条拒绝/10 条合法", () => {
    expect(validateMessages([]).ok).toBe(false);
    const countErr = validateMessages([]);
    expect(countErr.ok).toBe(false);
    if (!countErr.ok) expect(countErr.message).toContain("当前 0 条");
    const eleven = Array.from({ length: 11 }, (_, i) => `消息${i}`);
    expect(validateMessages(eleven).ok).toBe(false);
    const ten = Array.from({ length: 10 }, (_, i) => `消息${i}`);
    expect(validateMessages(ten).ok).toBe(true);
  });

  it("超 1000 标量拒绝;恰 1000 合法;1001 报超限并带序号", () => {
    expect(validateMessages(["a".repeat(1000)]).ok).toBe(true);
    const bad = validateMessages(["a".repeat(1001)]);
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.kind).toBe("too_long");
      expect(bad.index).toBe(1);
      expect(bad.message).toContain("1001 标量");
    }
  });

  it("空行拒绝:空串与纯空白都算空(trim 口径),报 1-based 序号", () => {
    const blank = validateMessages(["正常", "   "]);
    expect(blank.ok).toBe(false);
    if (!blank.ok) {
      expect(blank.kind).toBe("empty");
      expect(blank.index).toBe(2);
    }
    expect(validateMessages([""]).ok).toBe(false);
  });

  it("占位符错误带消息序号与字符列定位(quickstart US2-2)", () => {
    const err = validateMessages(["你好{{buyer}}!"]);
    expect(err.ok).toBe(false);
    if (!err.ok) {
      expect(err.kind).toBe("placeholder");
      expect(err.index).toBe(1);
      expect(err.message).toContain("第 3 字符处"); // “你好”之后第 3 字符起
      expect(err.message).toContain("未知变量");
    }
    // {{cards.非 法}}:key 含空格 → 定位到 "{{" 起始列
    const bad2 = validateMessages(["{{cards.非 法}}"]);
    expect(bad2.ok).toBe(false);
    if (!bad2.ok) expect(bad2.message).toContain("非法字符");
  });

  it("未闭合/空占位符/前缀后缺 key 拒绝;{{ order_id }} 容错合法", () => {
    expect(validateMessages(["前缀{{order_id"]).ok).toBe(false);
    expect(validateMessages(["{{}}"]).ok).toBe(false);
    expect(validateMessages(["{{cards.}}"]).ok).toBe(false);
    expect(validateMessages(["{{cards.a.b}}"]).ok).toBe(false); // 点号非法
    expect(validateMessages(["{{ order_id }}", "{{cards.Key-1}}"]).ok).toBe(true);
  });

  it("多消息全类型合法(系统变量 + cards + custom + 纯文本)", () => {
    const msgs = [
      "{{buyer_nickname}} 购买 {{card_name}}",
      "{{order_id}}/{{buyer_id}}",
      "您的卡密:{{cards.key1}}",
      "备注:{{custom.note}}",
      "纯文本消息"
    ];
    expect(validateMessages(msgs).ok).toBe(true);
  });
});

describe("名称校验与预览(T027)", () => {
  it("validateTemplateName:trim 非空、≤100 标量", () => {
    expect(validateTemplateName("  模板A ").ok).toBe(true);
    expect(validateTemplateName("   ").ok).toBe(false);
    expect(validateTemplateName("").ok).toBe(false);
    expect(validateTemplateName("a".repeat(100)).ok).toBe(true);
    expect(validateTemplateName("a".repeat(101)).ok).toBe(false);
  });

  it("messagePreview:压缩空白并截断加省略号", () => {
    expect(messagePreview("感谢\n购买  卡密")).toBe("感谢 购买 卡密");
    const long = "a".repeat(80);
    expect(messagePreview(long, 60)).toBe(`${"a".repeat(60)}…`);
  });
});
