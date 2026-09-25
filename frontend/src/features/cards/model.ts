/** 卡密库存页视图模型(007 T016/T017):类型过滤/名称搜索/库存派生/表单构建等纯函数,
 *  "算"与"摆"分离;业务规则仍在后端(宪章 II)。 */
import { isRecord, type CardPoolKind, type CardPoolDto, type StockSummary } from "../../shared/contracts";

// ===== 类型词汇表 =====

export type KindFilter = CardPoolKind | "all";

export const KIND_LABELS: Record<CardPoolKind, string> = {
  data: "批量卡密",
  text: "文本",
  image: "图片",
  api: "API"
};

export const KIND_TONES: Record<CardPoolKind, "info" | "normal" | "neutral"> = {
  data: "info",
  text: "normal",
  image: "normal",
  api: "neutral"
};

/** 工具栏类型下拉(全部 + 四类型)。 */
export const KIND_FILTER_OPTIONS: Array<{ value: KindFilter; label: string }> = [
  { value: "all", label: "全部类型" },
  { value: "data", label: "批量卡密" },
  { value: "text", label: "文本" },
  { value: "image", label: "图片" },
  { value: "api", label: "API" }
];

// ===== 过滤与派生 =====

/** 类型 + 名称过滤(本地即时,不重新请求):名称子串匹配不区分大小写;空条件返回全部。 */
export function filterCardPools(
  items: readonly CardPoolDto[],
  kind: KindFilter,
  query: string
): CardPoolDto[] {
  const q = query.trim().toLowerCase();
  return items.filter(
    (p) => (kind === "all" || p.kind === kind) && (q.length === 0 || p.name.toLowerCase().includes(q))
  );
}

/** 余量派生:分状态计数与总数;stock 缺失(非 data 组或旧版本降级)→ null。 */
export interface StockView {
  available: number;
  reserved: number;
  used: number;
  total: number;
}

export function stockView(stock: StockSummary | undefined): StockView | null {
  if (stock === undefined) return null;
  return {
    available: stock.available,
    reserved: stock.reserved,
    used: stock.used,
    total: stock.available + stock.reserved + stock.used
  };
}

/** 内容/库存单元格文案:data → 分状态库存计数;其余类型 → 形态说明(无明文)。 */
export function contentCellText(pool: CardPoolDto): { main: string; detail: string | null } {
  switch (pool.kind) {
    case "data": {
      const s = stockView(pool.stock);
      if (s === null) return { main: "库存: —", detail: "计数不可用" };
      return {
        main: `库存: ${s.total} 条`,
        detail: `可用 ${s.available} · 预留 ${s.reserved} · 已用 ${s.used}`
      };
    }
    case "text":
      return { main: "固定内容", detail: null };
    case "image":
      return { main: "图片链接", detail: null };
    case "api":
      return { main: "API 取卡", detail: null };
  }
}

/** data 组 textarea 行计数:非空行数(空行忽略,与后端 append 语义一致)。 */
export function countDataLines(text: string): number {
  return text.split(/\r?\n/).filter((l) => l.trim().length > 0).length;
}

/** 非空行拆分(提交 entries/append-data 用,去除首尾空白)。 */
export function dataLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

/** 延时发货秒校验(0—3600,与后端 check_delay_seconds 同口径)。 */
export function validateDelaySeconds(raw: string): { ok: boolean; value: number; message: string } {
  const t = raw.trim();
  if (!/^-?\d+$/.test(t)) return { ok: false, value: 0, message: "延时必须为整数秒(0—3600)" };
  const v = Number(t);
  if (v < 0 || v > 3600) return { ok: false, value: 0, message: "延时范围 0—3600 秒" };
  return { ok: true, value: v, message: "" };
}

// ===== API 取卡配置构建(编辑器内嵌构建器的纯逻辑)=====

/** 与后端 ApiCardConfig 对齐(headers/params 为 ["键","值"] 二元组数组)。 */
export interface ApiConfigPayload {
  url: string;
  method: "GET" | "POST";
  timeout_ms: number;
  headers: Array<[string, string]>;
  params: Array<[string, string]>;
  body?: string;
  content_type?: string;
  response_path: string;
  retry_enabled: boolean;
}

/** 请求头/参数 JSON 文本 → 二元组数组;接受 {"k":"v"} 或 [["k","v"]];空文本 = 清除。 */
export function parsePairsJson(text: string): { pairs: Array<[string, string]>; error: string | null } {
  const t = text.trim();
  if (t.length === 0) return { pairs: [], error: null };
  let parsed: unknown;
  try {
    parsed = JSON.parse(t);
  } catch {
    return { pairs: [], error: "不是合法 JSON" };
  }
  const pairs: Array<[string, string]> = [];
  if (Array.isArray(parsed)) {
    for (const item of parsed) {
      if (
        Array.isArray(item) &&
        item.length === 2 &&
        typeof item[0] === "string" &&
        typeof item[1] === "string"
      ) {
        pairs.push([item[0], item[1]]);
      } else {
        return { pairs: [], error: "数组元素必须为 [\"键\",\"值\"] 二元组" };
      }
    }
    return { pairs, error: null };
  }
  if (isRecord(parsed)) {
    for (const [k, v] of Object.entries(parsed)) {
      if (typeof v !== "string") {
        return { pairs: [], error: `键 "${k}" 的值必须为字符串` };
      }
      pairs.push([k, v]);
    }
    return { pairs, error: null };
  }
  return { pairs: [], error: "应为 {\"键\":\"值\"} 对象或二元组数组" };
}

export interface ApiFormInput {
  url: string;
  method: "GET" | "POST";
  timeoutSeconds: string;
  headersJson: string;
  paramsJson: string;
  contentType: string;
  body: string;
  responsePath: string;
  retryEnabled: boolean;
}

/** API 表单 → 请求载荷(创建/更新/test-api 共用);非法时逐项中文说明。 */
export function buildApiConfig(
  f: ApiFormInput
): { ok: true; config: ApiConfigPayload } | { ok: false; error: string } {
  const violations: string[] = [];
  const url = f.url.trim();
  if (url.length === 0) violations.push("API 地址为空");
  else if (!/^https?:\/\/\S+/i.test(url)) violations.push("API 地址必须是带主机的 http(s) URL");
  const secs = f.timeoutSeconds.trim();
  let timeout_ms = -1;
  if (!/^\d+$/.test(secs)) violations.push("超时秒必须为整数");
  else {
    const v = Number(secs);
    if (v < 1 || v > 60) violations.push("超时范围 1—60 秒");
    else timeout_ms = v * 1000;
  }
  const headers = parsePairsJson(f.headersJson);
  if (headers.error !== null) violations.push(`请求头 JSON:${headers.error}`);
  const params = parsePairsJson(f.paramsJson);
  if (params.error !== null) violations.push(`参数 JSON:${params.error}`);
  const body = f.body.trim();
  if (body.length > 0) {
    try {
      JSON.parse(body);
    } catch {
      violations.push("请求体不是合法 JSON");
    }
  }
  const response_path = f.responsePath.trim();
  if (response_path.length === 0) violations.push("响应取值路径不能为空");
  if (violations.length > 0) return { ok: false, error: violations.join(";") };
  const config: ApiConfigPayload = {
    url,
    method: f.method,
    timeout_ms,
    headers: headers.pairs,
    params: params.pairs,
    response_path,
    retry_enabled: f.retryEnabled
  };
  if (body.length > 0) config.body = body;
  const ct = f.contentType.trim();
  if (ct.length > 0) config.content_type = ct;
  return { ok: true, config };
}

// ===== 写接口响应解析 =====

/** append-data 响应(contracts §1)。 */
export interface AppendResult {
  appended: number;
  skipped_empty: number;
  skipped_duplicate: number;
}

export function parseAppendResult(v: unknown): AppendResult {
  if (!isRecord(v)) throw new Error("追加结果格式错误");
  return {
    appended: typeof v["appended"] === "number" ? v["appended"] : 0,
    skipped_empty: typeof v["skipped_empty"] === "number" ? v["skipped_empty"] : 0,
    skipped_duplicate: typeof v["skipped_duplicate"] === "number" ? v["skipped_duplicate"] : 0
  };
}

/** batch-import 响应(contracts §1):总行/成功/失败 + 逐行错误。 */
export interface ImportReport {
  total: number;
  succeeded: number;
  failed: Array<{ row: number; error: string }>;
}

export function parseImportReport(v: unknown): ImportReport {
  if (!isRecord(v)) throw new Error("导入结果格式错误");
  const failedRaw = Array.isArray(v["failed"]) ? v["failed"] : [];
  return {
    total: typeof v["total"] === "number" ? v["total"] : 0,
    succeeded: typeof v["succeeded"] === "number" ? v["succeeded"] : 0,
    failed: failedRaw.map((f): ImportReport["failed"][number] => {
      const r = isRecord(f) ? f : {};
      return {
        row: typeof r["row"] === "number" ? r["row"] : 0,
        error: String(r["error"] ?? "")
      };
    })
  };
}

/** test-api 响应(contracts §1):样例值 + 延迟,或错误。 */
export interface TestApiOutcome {
  ok: boolean;
  sample: string | null;
  latency_ms: number | null;
  error: string | null;
}

export function parseTestApiResult(v: unknown): TestApiOutcome {
  if (!isRecord(v)) throw new Error("测试结果格式错误");
  return {
    ok: v["ok"] === true,
    sample: typeof v["sample"] === "string" ? v["sample"] : null,
    latency_ms: typeof v["latency_ms"] === "number" ? v["latency_ms"] : null,
    error: typeof v["error"] === "string" ? v["error"] : null
  };
}
