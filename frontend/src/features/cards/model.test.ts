/** 卡密库存视图模型测试(007 T017):契约解析(含字段缺失降级)、
 *  类型筛选+名称搜索过滤、余量派生(available/reserved/used)、表单纯函数。 */
import { describe, expect, it } from "vitest";
import { parseCardPool, type CardPoolDto } from "../../shared/contracts";
import {
  KIND_FILTER_OPTIONS,
  buildApiConfig,
  contentCellText,
  countDataLines,
  dataLines,
  filterCardPools,
  parseAppendResult,
  parseImportReport,
  parsePairsJson,
  parseTestApiResult,
  stockView,
  validateDelaySeconds
} from "./model";

function pool(over: Partial<CardPoolDto> = {}): CardPoolDto {
  return {
    id: "cpl-1",
    name: "网课组A",
    kind: "data",
    enabled: true,
    delay_seconds: 0,
    description: "",
    version: 3,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T00:00:00Z",
    ...over
  };
}

describe("parseCardPool 契约解析(T015/T017)", () => {
  it("正常载荷完整解析(transport summary_json 形状)", () => {
    const v = parseCardPool({
      id: "cpl-x",
      name: "网课组",
      kind: "data",
      enabled: true,
      delay_seconds: 30,
      description: "主推",
      version: 7,
      created_at: "t1",
      updated_at: "t2",
      stock: { available: 3, reserved: 1, used: 2 },
      api_config: {
        url: "https://api.example.com/card",
        method: "POST",
        timeout_ms: 5000,
        content_type: "application/json",
        response_path: "data.card",
        retry_enabled: true,
        headers_configured: true,
        params_configured: false,
        body_configured: true
      }
    });
    expect(v.kind).toBe("data");
    expect(v.version).toBe(7);
    expect(v.stock).toEqual({ available: 3, reserved: 1, used: 2 });
    expect(v.api_config?.url).toBe("https://api.example.com/card");
    expect(v.api_config?.headers_configured).toBe(true);
    expect(v.api_config?.retry_enabled).toBe(true);
  });

  it("可选字段缺失降级:stock/content_set/api_config 不设值(不冒充 0)", () => {
    const v = parseCardPool({ id: "cpl-y", name: "文本组", kind: "text" });
    expect(v.stock).toBeUndefined();
    expect(v.content_set).toBeUndefined();
    expect(v.api_config).toBeUndefined();
    expect(v.enabled).toBe(false); // 缺失按 false,不臆造启用
    expect(v.delay_seconds).toBe(0);
    expect(v.version).toBe(1);
  });

  it("kind 未知值降级为 data;stock 子字段缺失按 0;非对象抛错", () => {
    const v = parseCardPool({ id: "cpl-z", kind: "voice", stock: {} });
    expect(v.kind).toBe("data");
    expect(v.stock).toEqual({ available: 0, reserved: 0, used: 0 });
    expect(() => parseCardPool("nope")).toThrow();
    expect(() => parseCardPool(null)).toThrow();
  });
});

describe("类型筛选 + 名称搜索(T017)", () => {
  const items = [
    pool({ id: "1", name: "网课组A", kind: "data" }),
    pool({ id: "2", name: "网课组B", kind: "data" }),
    pool({ id: "3", name: "感谢信", kind: "text" }),
    pool({ id: "4", name: "教程图", kind: "image" }),
    pool({ id: "5", name: "External API", kind: "api" })
  ];

  it("类型过滤:all 返回全部;单类型只留对应组", () => {
    expect(filterCardPools(items, "all", "")).toHaveLength(5);
    expect(filterCardPools(items, "data", "").map((p) => p.id)).toEqual(["1", "2"]);
    expect(filterCardPools(items, "text", "").map((p) => p.id)).toEqual(["3"]);
    expect(filterCardPools(items, "api", "").map((p) => p.id)).toEqual(["5"]);
  });

  it("名称搜索:不区分大小写子串;空串返回全部;与类型过滤可叠加", () => {
    expect(filterCardPools(items, "all", "网课").map((p) => p.id)).toEqual(["1", "2"]);
    expect(filterCardPools(items, "all", "external").map((p) => p.id)).toEqual(["5"]);
    expect(filterCardPools(items, "all", "   ")).toHaveLength(5);
    expect(filterCardPools(items, "data", "组b").map((p) => p.id)).toEqual(["2"]);
    expect(filterCardPools(items, "all", "不存在")).toEqual([]);
  });

  it("筛选下拉含 全部+四类型 五项", () => {
    expect(KIND_FILTER_OPTIONS.map((o) => o.value)).toEqual(["all", "data", "text", "image", "api"]);
  });
});

describe("余量派生(T017)", () => {
  it("stockView:总数 = available+reserved+used;缺失 → null", () => {
    expect(stockView({ available: 3, reserved: 1, used: 2 })).toEqual({
      available: 3,
      reserved: 1,
      used: 2,
      total: 6
    });
    expect(stockView(undefined)).toBeNull();
  });

  it("内容/库存单元格:data 显示库存计数与分状态;其余类型为形态说明", () => {
    const data = contentCellText(pool({ stock: { available: 3, reserved: 1, used: 2 } }));
    expect(data.main).toBe("库存: 6 条");
    expect(data.detail).toBe("可用 3 · 预留 1 · 已用 2");
    // stock 缺失(data 组但旧版本)→ 计数不可用,不显示 0
    const degraded = contentCellText(pool({}));
    expect(degraded.main).toBe("库存: —");
    expect(degraded.detail).toBe("计数不可用");
    expect(contentCellText(pool({ kind: "text" })).main).toBe("固定内容");
    expect(contentCellText(pool({ kind: "image" })).main).toBe("图片链接");
    expect(contentCellText(pool({ kind: "api" })).main).toBe("API 取卡");
  });
});

describe("表单纯函数(T017)", () => {
  it("countDataLines/dataLines:空行忽略、去除首尾空白(与后端 append 语义一致)", () => {
    const text = "CARD-1\n  \nCARD-2\r\n\r\n  CARD-3  \n";
    expect(countDataLines(text)).toBe(3);
    expect(dataLines(text)).toEqual(["CARD-1", "CARD-2", "CARD-3"]);
    expect(dataLines("")).toEqual([]);
  });

  it("validateDelaySeconds:0—3600 整数;越界与非整数拒绝", () => {
    expect(validateDelaySeconds("0").ok).toBe(true);
    expect(validateDelaySeconds("3600").ok).toBe(true);
    expect(validateDelaySeconds("3601").ok).toBe(false);
    expect(validateDelaySeconds("-1").ok).toBe(false);
    expect(validateDelaySeconds("abc").ok).toBe(false);
    expect(validateDelaySeconds(" 30 ").value).toBe(30);
  });

  it("parsePairsJson:对象/二元组数组/空文本;非法 JSON 与形状拒绝", () => {
    expect(parsePairsJson("")).toEqual({ pairs: [], error: null });
    expect(parsePairsJson('{"a":"1"}').pairs).toEqual([["a", "1"]]);
    expect(parsePairsJson('[["a","1"],["b","2"]]')).toEqual({
      pairs: [
        ["a", "1"],
        ["b", "2"]
      ],
      error: null
    });
    expect(parsePairsJson("{bad").error).not.toBeNull();
    expect(parsePairsJson('{"a":1}').error).not.toBeNull();
    expect(parsePairsJson('[["a"]]').error).not.toBeNull();
    expect(parsePairsJson('"x"').error).not.toBeNull();
  });

  it("buildApiConfig:合法表单产出后端 ApiCardConfig 形状(秒→毫秒)", () => {
    const built = buildApiConfig({
      url: "https://api.example.com/card",
      method: "POST",
      timeoutSeconds: "5",
      headersJson: '{"Authorization":"Bearer t"}',
      paramsJson: "",
      contentType: "application/json",
      body: '{"k":"v"}',
      responsePath: "data.card",
      retryEnabled: true
    });
    expect(built.ok).toBe(true);
    if (built.ok) {
      expect(built.config.timeout_ms).toBe(5000);
      expect(built.config.headers).toEqual([["Authorization", "Bearer t"]]);
      expect(built.config.params).toEqual([]);
      expect(built.config.body).toBe('{"k":"v"}');
      expect(built.config.content_type).toBe("application/json");
      expect(built.config.retry_enabled).toBe(true);
    }
  });

  it("buildApiConfig:逐项拒绝(空地址/非 http(s)/超时越界/空路径/坏 JSON)", () => {
    const base = {
      url: "https://x.example.com/c",
      method: "GET" as const,
      timeoutSeconds: "10",
      headersJson: "",
      paramsJson: "",
      contentType: "",
      body: "",
      responsePath: "data.card",
      retryEnabled: false
    };
    expect(buildApiConfig({ ...base, url: "" }).ok).toBe(false);
    expect(buildApiConfig({ ...base, url: "ftp://x/y" }).ok).toBe(false);
    expect(buildApiConfig({ ...base, timeoutSeconds: "0" }).ok).toBe(false);
    expect(buildApiConfig({ ...base, timeoutSeconds: "61" }).ok).toBe(false);
    expect(buildApiConfig({ ...base, responsePath: " " }).ok).toBe(false);
    expect(buildApiConfig({ ...base, headersJson: "{bad" }).ok).toBe(false);
    expect(buildApiConfig({ ...base, body: "not-json" }).ok).toBe(false);
  });
});

describe("写接口响应解析(T017,字段缺失降级)", () => {
  it("parseAppendResult:appended/skipped 系列", () => {
    expect(parseAppendResult({ appended: 2, skipped_empty: 1, skipped_duplicate: 3 })).toEqual({
      appended: 2,
      skipped_empty: 1,
      skipped_duplicate: 3
    });
    expect(parseAppendResult({})).toEqual({ appended: 0, skipped_empty: 0, skipped_duplicate: 0 });
    expect(() => parseAppendResult([])).toThrow();
  });

  it("parseImportReport:总行/成功/逐行失败;缺失按 0", () => {
    const r = parseImportReport({
      total: 3,
      succeeded: 2,
      failed: [
        { row: 4, error: "类型无法识别:voice" },
        { row: 5, error: "内容为空" }
      ]
    });
    expect(r.total).toBe(3);
    expect(r.succeeded).toBe(2);
    expect(r.failed).toEqual([
      { row: 4, error: "类型无法识别:voice" },
      { row: 5, error: "内容为空" }
    ]);
    const degraded = parseImportReport({ succeeded: 1, failed: "bad" });
    expect(degraded.total).toBe(0);
    expect(degraded.failed).toEqual([]);
  });

  it("parseTestApiResult:成功带样例与延迟;失败带错误", () => {
    expect(parseTestApiResult({ ok: true, sample: "CARD-9", latency_ms: 120 })).toEqual({
      ok: true,
      sample: "CARD-9",
      latency_ms: 120,
      error: null
    });
    expect(parseTestApiResult({ ok: false, sample: null, latency_ms: 800, error: "404" })).toEqual({
      ok: false,
      sample: null,
      latency_ms: 800,
      error: "404"
    });
    expect(parseTestApiResult({})).toEqual({ ok: false, sample: null, latency_ms: null, error: null });
  });
});
