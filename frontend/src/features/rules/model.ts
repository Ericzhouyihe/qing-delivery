/** 自动化规则视图模型(007 T040/T041):RuleDto/ReplyRule/DefaultReply 契约解析
 *  (只校验消费字段、缺失降级)、触发类型徽标映射、动作摘要派生、筛选纯函数、
 *  启停负载重建;表单级校验(变体/边界)自 catalog/rule-editor 复用导出。
 *  "算"在此(宪章 II),页面只装配与调用既有 API。 */
import { isRecord } from "../../shared/contracts";
import type { BadgeSpec } from "../../ui/badge";
import {
  buildRulePayload,
  splitSpecValues,
  validateRuleForm,
  type ContentSourceValue,
  type RuleFormValue,
  type TriggerTypeValue,
  type VariantForm
} from "../catalog/rule-editor";

// ===== RuleDto 解析(contracts §3;transport rule_dto 字段名对齐)=====

export interface RuleVariantDto {
  specName: string;
  specValues: string[];
  source: string;
  cardPoolId: string | null;
  templateId: string | null;
  unitsPerItem: number;
  delayOverrideSeconds: number | null;
}

export interface ReviewConfigDto {
  waitHours: number;
  intervalHours: number;
  maxCount: number;
  text: string;
}

export interface RuleDto {
  id: string;
  accountId: string;
  /** "" = 账号级(全部商品)。 */
  itemId: string;
  skuKey: string;
  enabled: boolean;
  version: number;
  /** 原始值透传(未知枚举安全渲染,不崩溃不误判)。 */
  triggerType: string;
  priority: number;
  allItemsConfirmed: boolean;
  needsReconfiguration: boolean;
  contentSource: string;
  cardPoolId: string | null;
  templateId: string | null;
  variants: RuleVariantDto[];
  reviewConfig: ReviewConfigDto | null;
}

function optString(v: unknown): string | null {
  if (typeof v !== "string" || v.length === 0) return null;
  return v;
}

function parseVariant(v: unknown): RuleVariantDto {
  const r = isRecord(v) ? v : {};
  return {
    specName: String(r["spec_name"] ?? ""),
    specValues: Array.isArray(r["spec_values"]) ? r["spec_values"].map((s) => String(s)) : [],
    source: String(r["source"] ?? "card_pool"),
    cardPoolId: optString(r["card_pool_id"]),
    templateId: optString(r["template_id"]),
    unitsPerItem: typeof r["units_per_item"] === "number" ? r["units_per_item"] : 1,
    delayOverrideSeconds: typeof r["delay_override_seconds"] === "number" ? r["delay_override_seconds"] : null
  };
}

/** parse 守卫:非对象抛错;字段缺失/形状不对按缺省降级(trigger_type→order_paid、
 *  priority→100、content_source→fixed_text,与后端列缺省同语义)。 */
export function parseRuleDto(v: unknown): RuleDto {
  if (!isRecord(v)) throw new Error("规则格式错误");
  const reviewRaw = isRecord(v["review_config"]) ? v["review_config"] : null;
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    itemId: String(v["item_id"] ?? ""),
    skuKey: String(v["sku_key"] ?? ""),
    enabled: v["enabled"] === true,
    version: typeof v["version"] === "number" ? v["version"] : 1,
    triggerType: String(v["trigger_type"] ?? "order_paid"),
    priority: typeof v["priority"] === "number" ? v["priority"] : 100,
    allItemsConfirmed: v["all_items_confirmed"] === true,
    needsReconfiguration: v["needs_reconfiguration"] === true,
    contentSource: String(v["content_source"] ?? "fixed_text"),
    cardPoolId: optString(v["card_pool_id"]),
    templateId: optString(v["template_id"]),
    variants: Array.isArray(v["variants"]) ? v["variants"].map(parseVariant) : [],
    reviewConfig:
      reviewRaw === null
        ? null
        : {
            waitHours: typeof reviewRaw["wait_hours"] === "number" ? reviewRaw["wait_hours"] : 0,
            intervalHours: typeof reviewRaw["interval_hours"] === "number" ? reviewRaw["interval_hours"] : 0,
            maxCount: typeof reviewRaw["max_count"] === "number" ? reviewRaw["max_count"] : 0,
            text: String(reviewRaw["text"] ?? "")
          }
  };
}

export interface RuleListResult {
  rules: RuleDto[];
  /** 触发类型 → 规则数(全部状态;缺失降级空对象)。 */
  triggerCounts: Record<string, number>;
}

export function parseRuleList(v: unknown): RuleListResult {
  if (!isRecord(v) || !Array.isArray(v["items"])) throw new Error("规则列表格式错误");
  const counts: Record<string, number> = {};
  if (isRecord(v["trigger_counts"])) {
    for (const [k, n] of Object.entries(v["trigger_counts"])) {
      if (typeof n === "number") counts[k] = n;
    }
  }
  return { rules: v["items"].map(parseRuleDto), triggerCounts: counts };
}

// ===== 触发类型徽标映射(FR-038;未知值安全降级为「未知状态」)=====

export const TRIGGER_LABELS: Record<string, string> = {
  order_paid: "付款后自动发货",
  review_missing_timeout: "超时未评价求评价",
  // buyer_reviewed 未开放创建(能力门禁 FR-036);存在旧数据时如实渲染
  buyer_reviewed: "评价后发赠品"
};

export const TRIGGER_TONES: Record<string, BadgeSpec["tone"]> = {
  order_paid: "normal",
  review_missing_timeout: "info",
  buyer_reviewed: "neutral"
};

export function triggerBadge(value: string): BadgeSpec {
  return {
    tone: TRIGGER_TONES[value] ?? "neutral",
    label: TRIGGER_LABELS[value] ?? "未知状态"
  };
}

// ===== 关联范围与动作摘要派生(FR-038 规则卡)=====

export interface RuleRefIndex {
  poolName: (id: string) => string | null;
  templateName: (id: string) => string | null;
  itemTitle: (id: string) => string | null;
}

export function buildRefIndex(refs: {
  pools: ReadonlyArray<{ id: string; name: string }>;
  templates: ReadonlyArray<{ id: string; name: string }>;
  items: ReadonlyArray<{ id: string; title: string }>;
}): RuleRefIndex {
  const pools = new Map(refs.pools.map((p) => [p.id, p.name]));
  const templates = new Map(refs.templates.map((t) => [t.id, t.name]));
  const items = new Map(refs.items.map((i) => [i.id, i.title]));
  return {
    poolName: (id) => pools.get(id) ?? null,
    templateName: (id) => templates.get(id) ?? null,
    itemTitle: (id) => items.get(id) ?? null
  };
}

/** 关联范围:账号级 / 商品标题(±规格键)。 */
export function scopeLabel(rule: RuleDto, refs: RuleRefIndex): string {
  if (rule.itemId === "") return "账号级(全部商品)";
  const title = refs.itemTitle(rule.itemId) ?? rule.itemId;
  return rule.skuKey === "" ? title : `${title} · ${rule.skuKey}`;
}

/** 动作摘要行(内容来源 + 变体/求评计划;引用名缺失回退 id)。 */
export function actionSummary(rule: RuleDto, refs: RuleRefIndex): string[] {
  const poolLabel = (id: string | null): string => (id === null ? "未绑定" : `卡密组「${refs.poolName(id) ?? id}」`);
  const templateLabel = (id: string | null): string =>
    id === null ? "未绑定" : `发货模板「${refs.templateName(id) ?? id}」`;
  if (rule.triggerType === "review_missing_timeout") {
    if (rule.reviewConfig === null) return ["求评计划未配置"];
    const rc = rule.reviewConfig;
    const text = rc.text.replace(/\s+/g, " ").trim();
    return [
      `等待 ${rc.waitHours}h · 间隔 ${rc.intervalHours}h · 最多 ${rc.maxCount} 次`,
      `文案:${text.length > 24 ? `${text.slice(0, 24)}…` : text}`
    ];
  }
  const lines: string[] = [];
  if (rule.variants.length === 0) {
    if (rule.contentSource === "fixed_text") lines.push("内容来源:固定文字");
    else if (rule.contentSource === "card_pool") lines.push(`内容来源:${poolLabel(rule.cardPoolId)}(每件 1 份)`);
    else if (rule.contentSource === "template") lines.push(`内容来源:${templateLabel(rule.templateId)}`);
    else lines.push(`内容来源:未知(${rule.contentSource})`);
  } else {
    // 带变体 = 容器:规则级绑定可留空(各变体独立来源)
    if (rule.cardPoolId !== null && rule.cardPoolId !== "") {
      lines.push(`默认来源:${poolLabel(rule.cardPoolId)}`);
    } else if (rule.templateId !== null && rule.templateId !== "") {
      lines.push(`默认来源:${templateLabel(rule.templateId)}`);
    } else if (rule.contentSource !== "fixed_text") {
      lines.push("默认来源:未绑定(按变体)");
    }
    for (const v of rule.variants) {
      const name = v.specName === "" ? "兜底" : v.specName;
      const values = v.specValues.length === 0 ? "全部规格" : v.specValues.join("/");
      const binding = v.source === "template" ? templateLabel(v.templateId) : poolLabel(v.cardPoolId);
      const units = v.source === "card_pool" ? ` ×${v.unitsPerItem}/件` : "";
      const delay = v.delayOverrideSeconds !== null && v.delayOverrideSeconds > 0 ? ` · 延时 ${v.delayOverrideSeconds}s` : "";
      lines.push(`变体 ${name}=${values} → ${binding}${units}${delay}`);
    }
  }
  return lines;
}

/** 警示徽标(FR-032/引用缺失):需确认·暂不发货 / 需重新配置。 */
export interface RuleWarning {
  kind: "needs_confirmation" | "needs_reconfiguration";
  label: string;
}

export function ruleWarnings(rule: RuleDto): RuleWarning[] {
  const out: RuleWarning[] = [];
  if (rule.needsReconfiguration) {
    out.push({ kind: "needs_reconfiguration", label: "需重新配置" });
  }
  if (rule.itemId === "" && !rule.allItemsConfirmed) {
    out.push({ kind: "needs_confirmation", label: "需确认·暂不发货" });
  }
  return out;
}

// ===== 筛选纯函数(本地即时过滤,不重新请求;006 模式)=====

export interface RuleFilterState {
  /** "" = 全部触发类型。 */
  trigger: string;
  status: "all" | "enabled" | "disabled";
  query: string;
}

export function filterRules(rules: readonly RuleDto[], f: RuleFilterState, refs: RuleRefIndex): RuleDto[] {
  const q = f.query.trim().toLowerCase();
  return rules.filter((r) => {
    if (f.trigger !== "" && r.triggerType !== f.trigger) return false;
    if (f.status === "enabled" && !r.enabled) return false;
    if (f.status === "disabled" && r.enabled) return false;
    if (q !== "") {
      const hay = [scopeLabel(r, refs), r.skuKey, r.id, TRIGGER_LABELS[r.triggerType] ?? r.triggerType]
        .join("\n")
        .toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

// ===== 启停负载重建(规则卡快捷启停;固定文字规则不回读正文,由编辑器整体替换)=====

/** 自 RuleDto 重建保存负载(内容空=后端保持现版;仅非固定来源可用)。
 *  返回 null 表示该规则无法在不回读正文的情况下整体重建(固定文字来源)。
 *  求评规则:content=求评文案(与 buildRulePayload 同语义,满足固定内容非空校验)。 */
export function rebuildPayloadForToggle(rule: RuleDto, enabled: boolean): Record<string, unknown> | null {
  if (rule.triggerType !== "review_missing_timeout" && rule.contentSource === "fixed_text") {
    return null;
  }
  const isReview = rule.triggerType === "review_missing_timeout";
  return {
    item_id: rule.itemId,
    sku_key: rule.skuKey,
    content_kind: isReview ? "fixed_text" : rule.contentSource,
    content: isReview ? (rule.reviewConfig?.text ?? "") : "",
    enabled,
    trigger_type: rule.triggerType,
    priority: rule.priority,
    content_source: isReview ? "fixed_text" : rule.contentSource,
    card_pool_id: !isReview && rule.contentSource === "card_pool" ? (rule.cardPoolId ?? "") : "",
    template_id: !isReview && rule.contentSource === "template" ? (rule.templateId ?? "") : "",
    variants: rule.variants.map((v) => ({
      spec_name: v.specName,
      spec_values: v.specValues,
      source: v.source === "template" ? "template" : "card_pool",
      card_pool_id: v.source === "template" ? "" : (v.cardPoolId ?? ""),
      template_id: v.source === "template" ? (v.templateId ?? "") : "",
      units_per_item: v.unitsPerItem,
      delay_override_seconds: v.delayOverrideSeconds
    })),
    review_config: isReview
      ? {
          wait_hours: rule.reviewConfig?.waitHours ?? 0,
          interval_hours: rule.reviewConfig?.intervalHours ?? 0,
          max_count: rule.reviewConfig?.maxCount ?? 0,
          text: rule.reviewConfig?.text ?? ""
        }
      : null,
    all_items_confirmed: rule.allItemsConfirmed,
    expected_version: rule.version
  };
}

/** RuleDto → 表单初值(编辑;固定内容不回读,正文区为空由宿主明示整体替换)。 */
export function ruleToFormValue(rule: RuleDto): RuleFormValue {
  const trigger: TriggerTypeValue = rule.triggerType === "review_missing_timeout" ? "review_missing_timeout" : "order_paid";
  const source: ContentSourceValue =
    rule.contentSource === "card_pool" || rule.contentSource === "template" ? rule.contentSource : "fixed_text";
  const variants: VariantForm[] = rule.variants.map((v) => ({
    specName: v.specName,
    specValues: v.specValues.join(","),
    source: v.source === "template" ? "template" : "card_pool",
    cardPoolId: v.cardPoolId ?? "",
    templateId: v.templateId ?? "",
    unitsPerItem: v.unitsPerItem,
    delayOverride: v.delayOverrideSeconds === null ? "" : String(v.delayOverrideSeconds)
  }));
  return {
    itemId: rule.itemId,
    skuKey: rule.skuKey,
    triggerType: trigger,
    priority: rule.priority,
    contentSource: source,
    content: "",
    cardPoolId: rule.cardPoolId ?? "",
    templateId: rule.templateId ?? "",
    variants,
    review: {
      waitHours: rule.reviewConfig?.waitHours ?? 24,
      intervalHours: rule.reviewConfig?.intervalHours ?? 24,
      maxCount: rule.reviewConfig?.maxCount ?? 1,
      text: rule.reviewConfig?.text ?? ""
    },
    allItemsConfirmed: rule.allItemsConfirmed,
    enabled: rule.enabled
  };
}

// ===== 关键词回复(contracts §3 reply-rules)=====

export interface ReplyRuleDto {
  id: string;
  accountId: string;
  keyword: string;
  /** text | image(未知值安全渲染)。 */
  replyKind: string;
  replyText: string | null;
  replyImageUrl: string | null;
  enabled: boolean;
  /** 空 = 账号级。 */
  itemIds: string[];
}

export function parseReplyRule(v: unknown): ReplyRuleDto {
  if (!isRecord(v)) throw new Error("关键词规则格式错误");
  return {
    id: String(v["id"] ?? ""),
    accountId: String(v["account_id"] ?? ""),
    keyword: String(v["keyword"] ?? ""),
    replyKind: String(v["reply_kind"] ?? "text"),
    replyText: optString(v["reply_text"]),
    replyImageUrl: optString(v["reply_image_url"]),
    enabled: v["enabled"] === true,
    itemIds: Array.isArray(v["item_ids"]) ? v["item_ids"].map((s) => String(s)) : []
  };
}

export function replyKindBadge(value: string): BadgeSpec {
  if (value === "text") return { tone: "normal", label: "文字" };
  if (value === "image") return { tone: "info", label: "图片" };
  return { tone: "neutral", label: "未知类型" };
}

/** 关键词回复表单值 + 客户端预校验(与后端 replies.rs 同口径:
 *  关键词非空 ≤100 字;text→文案必填 ≤2000;image→图片链接必填)。 */
export interface ReplyRuleFormValue {
  keyword: string;
  replyKind: "text" | "image";
  replyText: string;
  replyImageUrl: string;
  enabled: boolean;
  itemIds: string[];
}

export const REPLY_KEYWORD_MAX = 100;
export const REPLY_TEXT_MAX = 2_000;

export function validateReplyRuleForm(v: ReplyRuleFormValue): string[] {
  const errors: string[] = [];
  const kw = v.keyword.trim();
  if (kw.length === 0) errors.push("关键词不能为空");
  if ([...kw].length > REPLY_KEYWORD_MAX) errors.push(`关键词过长(上限 ${REPLY_KEYWORD_MAX} 字)`);
  if (v.replyKind === "text") {
    if (v.replyText.trim().length === 0) errors.push("文字回复必须填写回复文案");
    if ([...v.replyText].length > REPLY_TEXT_MAX) errors.push(`回复文案过长(上限 ${REPLY_TEXT_MAX} 字)`);
  } else if (v.replyKind === "image") {
    if (v.replyImageUrl.trim().length === 0) errors.push("图片回复必须填写图片链接");
  }
  return errors;
}

export function buildReplyRulePayload(v: ReplyRuleFormValue): Record<string, unknown> {
  return {
    keyword: v.keyword.trim(),
    reply_kind: v.replyKind,
    reply_text: v.replyText.trim().length === 0 ? null : v.replyText,
    reply_image_url: v.replyImageUrl.trim().length === 0 ? null : v.replyImageUrl.trim(),
    enabled: v.enabled,
    item_ids: v.itemIds
  };
}

export function filterReplyRules(rules: readonly ReplyRuleDto[], query: string, refs: RuleRefIndex): ReplyRuleDto[] {
  const q = query.trim().toLowerCase();
  if (q === "") return rules.slice();
  return rules.filter((r) => {
    const scope = r.itemIds.length === 0 ? "账号级" : r.itemIds.map((id) => refs.itemTitle(id) ?? id).join("/");
    return `${r.keyword}\n${scope}\n${r.id}`.toLowerCase().includes(q);
  });
}

// ===== 账号默认回复(contracts §3 default-reply)=====

export interface DefaultReplyRecordDto {
  id: string;
  buyerId: string;
  state: string;
  sentAt: string;
}

export interface DefaultReplyDto {
  accountId: string;
  enabled: boolean;
  replyText: string | null;
  replyImageUrl: string | null;
  replyOnce: boolean;
  updatedAt: string;
  recentRecords: DefaultReplyRecordDto[];
}

export function parseDefaultReply(v: unknown): DefaultReplyDto {
  if (!isRecord(v)) throw new Error("默认回复格式错误");
  return {
    accountId: String(v["account_id"] ?? ""),
    enabled: v["enabled"] === true,
    replyText: optString(v["reply_text"]),
    replyImageUrl: optString(v["reply_image_url"]),
    replyOnce: v["reply_once"] === true,
    updatedAt: String(v["updated_at"] ?? ""),
    recentRecords: Array.isArray(v["recent_records"])
      ? v["recent_records"].map((r): DefaultReplyRecordDto => {
          const rec = isRecord(r) ? r : {};
          return {
            id: String(rec["id"] ?? ""),
            buyerId: String(rec["buyer_id"] ?? ""),
            state: String(rec["state"] ?? ""),
            sentAt: String(rec["sent_at"] ?? "")
          };
        })
      : []
  };
}

/** 默认回复表单校验(后端同口径:启用须有文字或图片;文字 ≤2000)。 */
export function validateDefaultReplyForm(v: {
  enabled: boolean;
  replyText: string;
  replyImageUrl: string;
}): string[] {
  const errors: string[] = [];
  const hasText = v.replyText.trim().length > 0;
  const hasImage = v.replyImageUrl.trim().length > 0;
  if (v.enabled && !hasText && !hasImage) errors.push("启用默认回复必须填写文字或图片内容");
  if ([...v.replyText].length > REPLY_TEXT_MAX) errors.push(`默认回复过长(上限 ${REPLY_TEXT_MAX} 字)`);
  return errors;
}

// ===== 表单校验复用导出(测试口径;实现在 catalog/rule-editor)=====

export {
  buildRulePayload,
  splitSpecValues,
  validateRuleForm,
  type RuleFormValue,
  type TriggerTypeValue,
  type ContentSourceValue
};
