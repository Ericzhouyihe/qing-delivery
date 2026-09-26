/** 发货模板视图模型(007 T026/T027):契约解析(TemplateDto)、名称过滤、
 *  占位符提取(extractKeys 供卡片变量行)、消息校验(与后端 src/domain/templates.rs
 *  同规则:1..=10 条、单条 trim 非空、≤1000 标量/≤4000 字节、占位符语法)。
 *  前端预校验仅为即时反馈,服务端校验为准(宪章 II);"算"与"摆"分离。 */
import { isRecord } from "../../shared/contracts";

// ===== 边界常量(与后端 MAX_MESSAGES/MAX_MESSAGE_SCALARS/MAX_MESSAGE_BYTES 同值)=====

export const MAX_MESSAGES = 10;
export const MAX_MESSAGE_SCALARS = 1000;
export const MAX_MESSAGE_BYTES = 4000;
export const MAX_NAME_CHARS = 100;

/** 系统变量全集(后端 SYSTEM_VARS 同源;cards./custom. 为前缀命名空间)。 */
export const SYSTEM_VARS: readonly string[] = [
  "buyer_nickname",
  "order_id",
  "buyer_id",
  "card_name"
];

/** 变量名合法性:key 限 [A-Za-z0-9_-]+(data-model 约定,至少一个字符)。 */
const KEY_PATTERN = /^[A-Za-z0-9_-]+$/;

// ===== 契约解析(contracts §2 TemplateDto;只校验消费字段,缺失降级)=====

export interface TemplateKeys {
  cards: string[];
  custom: string[];
}

export interface TemplateDto {
  id: string;
  name: string;
  enabled: boolean;
  /** 自上而下有序;保存时整体替换。 */
  messages: string[];
  /** 服务端提取的占位符变量名(展示用;缺失按空数组降级)。 */
  keys: TemplateKeys;
  used_by_rules: number;
  version: number;
}

function stringList(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.map((s) => String(s));
}

/** parse 守卫:非对象抛错;字段缺失/形状不对按缺省降级(不冒充真实计数)。 */
export function parseTemplate(v: unknown): TemplateDto {
  if (!isRecord(v)) throw new Error("发货模板格式错误");
  const keysRaw = isRecord(v["keys"]) ? v["keys"] : {};
  return {
    id: String(v["id"] ?? ""),
    name: String(v["name"] ?? ""),
    enabled: v["enabled"] === true,
    messages: stringList(v["messages"]),
    keys: {
      cards: stringList(keysRaw["cards"]),
      custom: stringList(keysRaw["custom"])
    },
    used_by_rules: typeof v["used_by_rules"] === "number" ? v["used_by_rules"] : 0,
    version: typeof v["version"] === "number" ? v["version"] : 1
  };
}

// ===== 过滤(本地即时,不重新请求;006 账号页模式)=====

/** 名称过滤:trim 后子串匹配不区分大小写;空条件返回全部。 */
export function filterTemplates(items: readonly TemplateDto[], query: string): TemplateDto[] {
  const q = query.trim().toLowerCase();
  if (q.length === 0) return items.slice();
  return items.filter((t) => t.name.toLowerCase().includes(q));
}

// ===== 占位符解析(与后端 parse_body/parse_var/check_key 同算法)=====

/** 单个占位符出现位置:name 为去空白后的内部文本,column 为 1-based 字符列。 */
interface PlaceholderSite {
  name: string;
  column: number;
}

interface ScanOk {
  ok: true;
  sites: PlaceholderSite[];
}
interface ScanErr {
  ok: false;
  column: number;
  reason: string;
}

/** 扫描单条消息的占位符:纯文本合法;"{{" 后取首个 "}}" 配对;
 *  未闭合/内部为空报错并定位到 "{{" 起始列(1-based,与后端口径一致)。 */
function scanPlaceholders(body: string): ScanOk | ScanErr {
  const chars = [...body];
  const sites: PlaceholderSite[] = [];
  let i = 0;
  while (i < chars.length) {
    if (chars[i] === "{" && chars[i + 1] === "{") {
      const start = i;
      let j = i + 2;
      while (j < chars.length && !(chars[j] === "}" && chars[j + 1] === "}")) {
        j += 1;
      }
      if (j >= chars.length) {
        return { ok: false, column: start + 1, reason: "占位符未闭合(缺少 }})" };
      }
      const inner = chars.slice(i + 2, j).join("").trim();
      if (inner.length === 0) {
        return { ok: false, column: start + 1, reason: "占位符为空" };
      }
      sites.push({ name: inner, column: start + 1 });
      i = j + 2;
    } else {
      i += 1;
    }
  }
  return { ok: true, sites };
}

/** 变量名 → 分类:系统变量/cards./custom. 前缀;未知变量/非法 key 返回可读原因。 */
function parseVarName(
  name: string
): { ok: true; kind: "system" | "cards" | "custom"; key?: string } | { ok: false; reason: string } {
  if (name.startsWith("cards.") || name.startsWith("custom.")) {
    const kind = name.startsWith("cards.") ? "cards" : "custom";
    const key = name.slice(kind.length + 1);
    if (key.length === 0) {
      return { ok: false, reason: "cards./custom. 后缺少变量名" };
    }
    if (!KEY_PATTERN.test(key)) {
      return {
        ok: false,
        reason: `变量名“${key}”含非法字符(仅限字母/数字/下划线/连字符)`
      };
    }
    return { ok: true, kind, key };
  }
  if ((SYSTEM_VARS as readonly string[]).includes(name)) {
    return { ok: true, kind: "system" };
  }
  return {
    ok: false,
    reason: `未知变量“${name}”(可用:buyer_nickname/order_id/buyer_id/card_name/cards.<key>/custom.<key>)`
  };
}

// ===== 消息校验(与服务端 validate_messages 同规则同顺序)=====

export type MessageCheck =
  | { ok: true }
  | { ok: false; kind: "count" | "empty" | "too_long" | "placeholder"; index: number; message: string };

/** 校验单条消息(编辑器逐行实时反馈;index 为 1-based 消息序号)。 */
export function checkMessageBody(body: string, index = 1): { ok: boolean; message: string } {
  const r = validateMessages([body], index);
  return r.ok ? { ok: true, message: "" } : { ok: false, message: r.message };
}

/**
 * 校验消息列表:条数 1..=10;单条 trim 非空;≤1000 标量/≤4000 字节;占位符
 * 语法(命名/闭合/已知变量)。错误信息与后端 TemplateError 文案同口径(带定位)。
 */
export function validateMessages(messages: readonly string[], firstIndex = 1): MessageCheck {
  if (messages.length === 0 || messages.length > MAX_MESSAGES) {
    return {
      ok: false,
      kind: "count",
      index: 0,
      message: `消息列表必须为 1—${MAX_MESSAGES} 条,当前 ${messages.length} 条`
    };
  }
  for (let i = 0; i < messages.length; i++) {
    const index = firstIndex + i;
    const body = messages[i] ?? "";
    if (body.trim().length === 0) {
      return {
        ok: false,
        kind: "empty",
        index,
        message: `第 ${index} 条消息为空(去除首尾空白后不得为空)`
      };
    }
    const scalars = [...body].length;
    const bytes = new TextEncoder().encode(body).length;
    if (scalars > MAX_MESSAGE_SCALARS || bytes > MAX_MESSAGE_BYTES) {
      return {
        ok: false,
        kind: "too_long",
        index,
        message: `第 ${index} 条消息超限:${scalars} 标量/上限 ${MAX_MESSAGE_SCALARS},${bytes} 字节/上限 ${MAX_MESSAGE_BYTES};不截断`
      };
    }
    const scan = scanPlaceholders(body);
    if (!scan.ok) {
      return {
        ok: false,
        kind: "placeholder",
        index,
        message: `第 ${index} 条消息第 ${scan.column} 字符处占位符非法:${scan.reason}`
      };
    }
    for (const site of scan.sites) {
      const parsed = parseVarName(site.name);
      if (!parsed.ok) {
        return {
          ok: false,
          kind: "placeholder",
          index,
          message: `第 ${index} 条消息第 ${site.column} 字符处占位符非法:${parsed.reason}`
        };
      }
    }
  }
  return { ok: true };
}

// ===== 变量提取(卡片变量提示行;后端 extract_keys 同口径)=====

/** 提取消息列表全部 cards./custom. 变量名(按出现顺序去重;
 *  无法解析的占位符跳过不失败,与后端 unwrap_or_default 语义一致)。 */
export function extractKeys(messages: readonly string[]): TemplateKeys {
  const keys: TemplateKeys = { cards: [], custom: [] };
  for (const body of messages) {
    const scan = scanPlaceholders(body);
    if (!scan.ok) continue;
    for (const site of scan.sites) {
      const parsed = parseVarName(site.name);
      if (!parsed.ok) continue;
      if (parsed.kind === "cards" && !keys.cards.includes(parsed.key!)) {
        keys.cards.push(parsed.key!);
      } else if (parsed.kind === "custom" && !keys.custom.includes(parsed.key!)) {
        keys.custom.push(parsed.key!);
      }
    }
  }
  return keys;
}

/** 卡片变量行文案:列出全部 cards./custom. key;无变量返回 null。 */
export function keysLine(keys: TemplateKeys): string | null {
  const parts: string[] = [];
  for (const k of keys.cards) parts.push(`cards.${k}`);
  for (const k of keys.custom) parts.push(`custom.${k}`);
  return parts.length > 0 ? parts.join(" · ") : null;
}

// ===== 名称校验(后端 validate_meta 同口径:trim 非空、≤100 标量)=====

export function validateTemplateName(name: string): { ok: boolean; message: string } {
  const t = name.trim();
  if (t.length === 0) return { ok: false, message: "模板名不能为空" };
  if ([...t].length > MAX_NAME_CHARS) {
    return { ok: false, message: `模板名过长(上限 ${MAX_NAME_CHARS} 字)` };
  }
  return { ok: true, message: "" };
}

/** 卡片消息预览:压缩换行为空格并截断(展示用,不参与校验)。 */
export function messagePreview(body: string, max = 60): string {
  const flat = body.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

// ===== 变量指南(编辑器右侧卡;契约 §2/data-model 占位符语法)=====

export interface VariableDoc {
  token: string;
  desc: string;
}

export const VARIABLE_DOCS: readonly VariableDoc[] = [
  { token: "{{buyer_nickname}}", desc: "买家昵称" },
  { token: "{{order_id}}", desc: "订单号" },
  { token: "{{buyer_id}}", desc: "买家 ID" },
  { token: "{{card_name}}", desc: "商品名称" },
  {
    token: "{{cards.<变量名>}}",
    desc: "卡密内容:发货时按绑定卡密组预留扣减,变量名需在规则中绑定卡密组"
  },
  { token: "{{custom.<变量名>}}", desc: "自定义文本:在规则绑定中赋值" }
];

/** 指南示例块:全部为合法占位符(custom key 仅限 [A-Za-z0-9_-])。 */
export const VARIABLE_EXAMPLE = [
  "{{buyer_nickname}} 你好,感谢购买 {{card_name}}",
  "您的卡密:{{cards.key1}}",
  "订单 {{order_id}} 备查,备注:{{custom.note}}"
].join("\n");
