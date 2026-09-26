/** 规则表单(007 T040 重构):自 001/002 固定文字抽屉表单提取的可复用主体
 *  `renderRuleForm`,供 features/rules 编辑抽屉嵌入(/catalog 合并页已拆分);
 *  US3 v2 扩展全部在表单内呈现:触发类型/优先级/内容来源三选一(固定文字/
 *  卡密库存/发货模板)/变体列表/求评配置四字段/账号级「适用于全部商品」确认开关。
 *  纯函数(校验/负载组装/规格值拆分)与 DOM 装配同文件导出,供页面与测试复用;
 *  match-preview.ts 零改动,匹配判定仍全部经服务端(宪章 II)。
 *  契约限制保持:规则无读取既有正文的端点,编辑=整体替换并明示(保存负载自表单)。 */
import { el } from "../../shared/dom";
import { httpPost } from "../../shared/http";
import { btn } from "../../ui/dom";
import { renderMatchPreview } from "./match-preview";
import type { ProductItem } from "./products";

/** 字数上限取自 capabilities content_limits(001 既有约束,不自定口径)。 */
export const CONTENT_LIMITS = { unicode_scalars: 1000, utf8_bytes: 4000 } as const;

export interface ContentValidation {
  ok: boolean;
  unicodeScalars: number;
  message: string;
}

/** 客户端最小校验(必填 + 字数);占位符合法性由服务端预演判定(不本地复现)。 */
export function validateContent(content: string): ContentValidation {
  const scalars = [...content].length;
  if (content.trim().length === 0) {
    return { ok: false, unicodeScalars: scalars, message: "内容不能为空" };
  }
  if (scalars > CONTENT_LIMITS.unicode_scalars) {
    return {
      ok: false,
      unicodeScalars: scalars,
      message: `超出字数上限(${scalars}/${CONTENT_LIMITS.unicode_scalars} 字符)`
    };
  }
  return { ok: true, unicodeScalars: scalars, message: "" };
}

// ===== 边界常量(与 src/domain/rules_ext.rs、application/catalog/rules.rs 同值)=====

export const PRIORITY_MIN = 1;
export const PRIORITY_MAX = 10_000;
/** 001 存量行即此值(零迁移默认;数字越小优先级越高)。 */
export const PRIORITY_DEFAULT = 100;
/** 每件份数仅存在于变体(FR-030/后端 RuleDraft 无规则级份数字段)。 */
export const UNITS_MIN = 1;
export const UNITS_MAX = 100;
export const DELAY_OVERRIDE_MAX = 3_600;
export const REVIEW_WAIT_MIN = 1;
export const REVIEW_INTERVAL_MIN = 1;
export const REVIEW_MAX_COUNT_MIN = 1;
export const REVIEW_MAX_COUNT_MAX = 10;

// ===== 表单值类型(US3 v2;buyer_reviewed 未开放不进入表单,FR-036)=====

export type TriggerTypeValue = "order_paid" | "review_missing_timeout";
export type ContentSourceValue = "fixed_text" | "card_pool" | "template";
export type VariantSourceValue = "card_pool" | "template";

/** 求评配置四字段(FR-035;仅「超时未评价」触发携带)。 */
export interface ReviewConfigForm {
  waitHours: number;
  intervalHours: number;
  maxCount: number;
  text: string;
}

/** 变体行(编辑态;specValues 为原始输入,逗号/顿号/分号分隔多值)。 */
export interface VariantForm {
  specName: string;
  specValues: string;
  source: VariantSourceValue;
  cardPoolId: string;
  templateId: string;
  unitsPerItem: number;
  /** "" = 不覆盖(沿用组级延时)。 */
  delayOverride: string;
}

export interface RuleFormValue {
  /** "" = 账号级(全部商品)。 */
  itemId: string;
  skuKey: string;
  triggerType: TriggerTypeValue;
  priority: number;
  contentSource: ContentSourceValue;
  content: string;
  cardPoolId: string;
  templateId: string;
  variants: VariantForm[];
  review: ReviewConfigForm;
  /** 账号级「适用于全部商品」确认(FR-032;关闭=保存后「需确认·暂不发货」)。 */
  allItemsConfirmed: boolean;
  enabled: boolean;
}

export function defaultRuleFormValue(): RuleFormValue {
  return {
    itemId: "",
    skuKey: "",
    triggerType: "order_paid",
    priority: PRIORITY_DEFAULT,
    contentSource: "fixed_text",
    content: "",
    cardPoolId: "",
    templateId: "",
    variants: [],
    review: { waitHours: 24, intervalHours: 24, maxCount: 1, text: "" },
    allItemsConfirmed: false,
    enabled: true
  };
}

/** 规格值输入拆分:逗号/中文逗号/顿号/分号分隔,trim 后去空去重(后端分号集合语义)。 */
export function splitSpecValues(raw: string): string[] {
  const out: string[] = [];
  for (const part of raw.split(/[,，、;；]/)) {
    const t = part.trim();
    if (t.length > 0 && !out.includes(t)) out.push(t);
  }
  return out;
}

/** 客户端预校验(与后端 assemble 同口径;仅拦截明确错误,服务端校验为准)。
 *  返回错误消息列表(空=通过);账号级未确认不属于错误(允许落库,标「需确认」)。 */
export function validateRuleForm(v: RuleFormValue): string[] {
  const errors: string[] = [];
  if (!Number.isInteger(v.priority) || v.priority < PRIORITY_MIN || v.priority > PRIORITY_MAX) {
    errors.push(`优先级必须为 ${PRIORITY_MIN}–${PRIORITY_MAX} 的整数(数字越小越高)`);
  }
  if (v.triggerType === "review_missing_timeout") {
    if (!Number.isInteger(v.review.waitHours) || v.review.waitHours < REVIEW_WAIT_MIN) {
      errors.push(`发货后等待小时数须 ≥${REVIEW_WAIT_MIN}`);
    }
    if (!Number.isInteger(v.review.intervalHours) || v.review.intervalHours < REVIEW_INTERVAL_MIN) {
      errors.push(`再次求评间隔小时数须 ≥${REVIEW_INTERVAL_MIN}`);
    }
    if (
      !Number.isInteger(v.review.maxCount) ||
      v.review.maxCount < REVIEW_MAX_COUNT_MIN ||
      v.review.maxCount > REVIEW_MAX_COUNT_MAX
    ) {
      errors.push(`最多次数须为 ${REVIEW_MAX_COUNT_MIN}–${REVIEW_MAX_COUNT_MAX}`);
    }
    if (v.review.text.trim().length === 0) {
      errors.push("求评文案不能为空");
    }
  } else {
    // 来源绑定互斥与必填(带变体=容器,规则级绑定可留空)
    if (v.contentSource === "fixed_text") {
      const c = validateContent(v.content);
      if (!c.ok) errors.push(c.message);
    } else if (v.contentSource === "card_pool") {
      if (v.cardPoolId === "" && v.variants.length === 0) {
        errors.push("卡密库存来源须选择卡密组(或添加变体)");
      }
    } else if (v.contentSource === "template") {
      if (v.templateId === "" && v.variants.length === 0) {
        errors.push("发货模板来源须选择模板(或添加变体)");
      }
    }
  }
  v.variants.forEach((vr, i) => {
    const idx = i + 1;
    const hasName = vr.specName.trim().length > 0;
    const hasValues = splitSpecValues(vr.specValues).length > 0;
    if (hasName !== hasValues) {
      errors.push(`第 ${idx} 个变体:规格名与规格值须成对出现(或全空=兜底变体)`);
    }
    if (!Number.isInteger(vr.unitsPerItem) || vr.unitsPerItem < UNITS_MIN || vr.unitsPerItem > UNITS_MAX) {
      errors.push(`第 ${idx} 个变体:每件份数须为 ${UNITS_MIN}–${UNITS_MAX}`);
    }
    if (vr.delayOverride.trim().length > 0) {
      const d = Number(vr.delayOverride);
      if (!Number.isInteger(d) || d < 0 || d > DELAY_OVERRIDE_MAX) {
        errors.push(`第 ${idx} 个变体:延时覆盖须为 0–${DELAY_OVERRIDE_MAX} 秒`);
      }
    }
    if (vr.source === "card_pool" && vr.cardPoolId === "") {
      errors.push(`第 ${idx} 个变体(卡密组来源)须选择卡密组`);
    }
    if (vr.source === "template" && vr.templateId === "") {
      errors.push(`第 ${idx} 个变体(模板来源)须选择模板`);
    }
  });
  return errors;
}

/** 组装保存请求体(contracts §3 RuleWriteRequest;旧别名 content_kind 一并携带)。
 *  求评触发:内容=求评文案(满足固定内容非空校验;实际发送走求评计划文案)。
 *  all_items_confirmed 仅账号级携带(商品级语义无关,统一 false)。 */
export function buildRulePayload(v: RuleFormValue, expectedVersion: number | null): Record<string, unknown> {
  const isReview = v.triggerType === "review_missing_timeout";
  const source: ContentSourceValue = isReview ? "fixed_text" : v.contentSource;
  const payload: Record<string, unknown> = {
    item_id: v.itemId,
    sku_key: v.skuKey,
    content_kind: source,
    content: isReview ? v.review.text : source === "fixed_text" ? v.content : "",
    enabled: v.enabled,
    trigger_type: v.triggerType,
    priority: v.priority,
    content_source: source,
    card_pool_id: !isReview && v.contentSource === "card_pool" ? v.cardPoolId : "",
    template_id: !isReview && v.contentSource === "template" ? v.templateId : "",
    variants: (isReview ? [] : v.variants).map((vr) => ({
      spec_name: vr.specName.trim(),
      spec_values: splitSpecValues(vr.specValues),
      source: vr.source,
      card_pool_id: vr.source === "card_pool" ? vr.cardPoolId : "",
      template_id: vr.source === "template" ? vr.templateId : "",
      units_per_item: vr.unitsPerItem,
      delay_override_seconds: vr.delayOverride.trim().length === 0 ? null : Number(vr.delayOverride)
    })),
    review_config: isReview
      ? {
          wait_hours: v.review.waitHours,
          interval_hours: v.review.intervalHours,
          max_count: v.review.maxCount,
          text: v.review.text
        }
      : null,
    all_items_confirmed: v.itemId === "" ? v.allItemsConfirmed : false
  };
  if (expectedVersion !== null) {
    payload["expected_version"] = expectedVersion;
  }
  return payload;
}

// ===== DOM 装配 =====

/** 内容来源引用(下拉数据源;disabled 项仅在被当前选择引用时出现,如实标注)。 */
export interface RuleFormRefOption {
  id: string;
  name: string;
  enabled: boolean;
}

export interface RuleFormOptions {
  accountId: string;
  /** 该账号商品(范围选择与匹配预览共用)。 */
  items: readonly ProductItem[];
  /** 卡密组引用(仅 data 组有意义;页面预过滤)。 */
  pools: readonly RuleFormRefOption[];
  /** 发货模板引用。 */
  templates: readonly RuleFormRefOption[];
  value: RuleFormValue;
  /** 范围(商品/规格)不可变:编辑既有规则时锁定(PUT 后端拒绝范围变更)。 */
  lockScope?: boolean;
  onDirty?: () => void;
}

export interface RuleFormHandle {
  /** 校验并刷新行内错误汇总;返回错误列表(空=可保存)。 */
  validate(): string[];
  getValue(): RuleFormValue;
  buildPayload(expectedVersion: number | null): Record<string, unknown>;
}

const field = (label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement =>
  el("div", { class: "field" }, el("label", {}, label), control, ...hints);

/** 引用下拉:启用项在前;当前绑定不在启用列表时补「(已停用)」选项,不静默丢失。 */
function refSelect(
  refs: readonly RuleFormRefOption[],
  selected: string,
  emptyLabel: string,
  attrs: Record<string, string> = {}
): HTMLSelectElement {
  const sel = el("select", attrs) as HTMLSelectElement;
  sel.append(el("option", { value: "" }, emptyLabel));
  for (const r of refs) {
    if (r.enabled || r.id === selected) {
      sel.append(el("option", { value: r.id }, r.enabled ? r.name : `${r.name}(已停用)`));
    }
  }
  sel.value = selected;
  return sel;
}

/** 可复用规则表单主体(T040):纯 DOM 装配 + 行内校验汇总 + 内容预演 + 匹配预览。
 *  宿主(抽屉)负责保存按钮与请求;本表单不发起保存。 */
export function renderRuleForm(container: HTMLElement, opts: RuleFormOptions): RuleFormHandle {
  const v = opts.value;
  let dirtyHook = opts.onDirty ?? (() => undefined);
  const markDirty = (): void => {
    dirtyHook();
  };

  // ---- 触发类型 + 优先级 ----
  const triggerSel = el("select", {}) as HTMLSelectElement;
  triggerSel.append(
    el("option", { value: "order_paid" }, "付款后自动发货"),
    el("option", { value: "review_missing_timeout" }, "超时未评价求评价")
  );
  triggerSel.value = v.triggerType;
  // buyer_reviewed(评价后赠品)能力未验证,入口隐藏(FR-036/SC-008)
  const priorityInput = el("input", {
    type: "number",
    min: String(PRIORITY_MIN),
    max: String(PRIORITY_MAX),
    step: "1"
  }) as HTMLInputElement;
  priorityInput.value = String(v.priority);

  // ---- 范围:商品(含账号级)+ 规格键 ----
  const itemSel = el("select", {}) as HTMLSelectElement;
  itemSel.append(el("option", { value: "" }, "账号级(全部商品)"));
  for (const it of opts.items) {
    itemSel.append(el("option", { value: it.id }, it.title.slice(0, 40)));
  }
  // 防御:编辑既有规则时商品可能暂未同步——保留原值选项,不得静默回落账号级改写范围
  if (v.itemId !== "" && !opts.items.some((it) => it.id === v.itemId)) {
    itemSel.append(el("option", { value: v.itemId }, `未同步商品(${v.itemId.slice(0, 12)}…)`));
  }
  itemSel.value = v.itemId;
  const skuInput = el("input", { type: "text", placeholder: "规格键(默认规格留空)" }) as HTMLInputElement;
  skuInput.value = v.skuKey;
  if (opts.lockScope === true) {
    itemSel.disabled = true;
    itemSel.title = "规则范围(商品/规格)不可修改,请删除后新建";
    skuInput.disabled = true;
    skuInput.title = "规则范围(商品/规格)不可修改";
  }

  // ---- 账号级确认开关(FR-032)----
  const allItemsCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  allItemsCheck.checked = v.allItemsConfirmed;
  const allItemsWarn = el(
    "div",
    { class: "banner banner-warning", role: "alert" },
    "未确认「适用于全部商品」:保存后标记「需确认·暂不发货」,触发订单不会自动发货"
  );
  const allItemsBlock = el(
    "div",
    { class: "field" },
    el("label", { class: "check-row" }, allItemsCheck, "适用于全部商品(账号级规则确认)"),
    el("div", { class: "hint" }, "账号级规则覆盖全部商品,须显式确认后才会真实执行"),
    allItemsWarn
  );

  // ---- 内容来源(仅付款触发;求评触发文案在求评配置内)----
  const sourceSel = el("select", {}) as HTMLSelectElement;
  sourceSel.append(
    el("option", { value: "fixed_text" }, "固定文字"),
    el("option", { value: "card_pool" }, "卡密库存"),
    el("option", { value: "template" }, "发货模板")
  );
  sourceSel.value = v.contentSource;

  const contentArea = el("textarea", {
    placeholder: "买家将收到的固定内容,如:链接 + 提取码"
  }) as HTMLTextAreaElement;
  contentArea.value = v.content;
  const charCount = el("span", { class: "muted", style: "font-size:var(--text-xs);" }, `0 / ${CONTENT_LIMITS.unicode_scalars}`);
  const contentError = el("div", { class: "field-error" }, "");
  const previewBtn = btn("内容预演", { variant: "secondary" });
  const previewBox = el("pre", {
    class: "preview",
    style: "display:none;white-space:pre-wrap;border:1px dashed var(--border-strong);padding:8px;border-radius:6px;"
  });

  const poolSel = refSelect(opts.pools, v.cardPoolId, "选择启用中的批量卡密组…");
  const poolHint = el(
    "div",
    { class: "hint" },
    "预留数量 = 每件 1 份 × 购买件数(规则级来源后端固定每件 1 份;不同份数请在下方变体中配置)"
  );
  const templateSel = refSelect(opts.templates, v.templateId, "选择启用中的发货模板…");
  const templateHint = el("div", { class: "hint" }, "发货时按模板消息逐条发送;卡密变量内容只经加密快照,界面不回显");

  // ---- 变体列表(FR-030;可选)----
  const variantHost = el("div", { class: "rule-variant-list" });
  const addVariantBtn = btn("+ 添加变体", { variant: "secondary" });

  // ---- 求评配置四字段(FR-035)----
  const waitInput = el("input", { type: "number", min: "1", step: "1" }) as HTMLInputElement;
  waitInput.value = String(v.review.waitHours);
  const intervalInput = el("input", { type: "number", min: "1", step: "1" }) as HTMLInputElement;
  intervalInput.value = String(v.review.intervalHours);
  const maxCountInput = el("input", { type: "number", min: "1", max: "10", step: "1" }) as HTMLInputElement;
  maxCountInput.value = String(v.review.maxCount);
  const reviewTextArea = el("textarea", { rows: "3", placeholder: "求评文案,如:感谢购买,期待您的评价~" }) as HTMLTextAreaElement;
  reviewTextArea.value = v.review.text;
  reviewTextArea.style.minHeight = "64px";

  // ---- 分区容器(按触发类型/来源/范围切换显隐)----
  const sourceBlock = el("div", {}, "");
  const fixedBox = el("div", {}, "");
  const poolBox = el("div", {}, "");
  const templateBox = el("div", {}, "");
  const variantsBlock = el("div", {}, "");
  const reviewBlock = el("div", {}, "");

  const refreshContentError = (): void => {
    const c = validateContent(contentArea.value);
    charCount.textContent = `${c.unicodeScalars} / ${CONTENT_LIMITS.unicode_scalars}`;
    contentError.textContent = c.message;
    contentArea.setAttribute("aria-invalid", c.ok ? "false" : "true");
  };

  /** 单个变体行(增删才重建行;输入写回 value) */
  const buildVariantRow = (idx: number): HTMLElement => {
    const vr = v.variants[idx]!;
    const nameInput = el("input", { type: "text", placeholder: "规格名,如:颜色" }) as HTMLInputElement;
    nameInput.value = vr.specName;
    const valuesInput = el("input", { type: "text", placeholder: "规格值,多个用逗号/顿号分隔,如:红色,蓝色" }) as HTMLInputElement;
    valuesInput.value = vr.specValues;
    const vSourceSel = el("select", {}) as HTMLSelectElement;
    vSourceSel.append(el("option", { value: "card_pool" }, "卡密组"), el("option", { value: "template" }, "发货模板"));
    vSourceSel.value = vr.source;
    const vPoolSel = refSelect(opts.pools, vr.cardPoolId, "选择卡密组…");
    const vTemplateSel = refSelect(opts.templates, vr.templateId, "选择模板…");
    const unitsInput = el("input", { type: "number", min: String(UNITS_MIN), max: String(UNITS_MAX), step: "1" }) as HTMLInputElement;
    unitsInput.value = String(vr.unitsPerItem);
    unitsInput.style.width = "90px";
    const delayInput = el("input", { type: "number", min: "0", max: String(DELAY_OVERRIDE_MAX), step: "1", placeholder: "不覆盖" }) as HTMLInputElement;
    delayInput.value = vr.delayOverride;
    delayInput.style.width = "100px";
    const delBtn = el("button", { class: "icon-btn danger-icon", type: "button", title: `删除第 ${idx + 1} 个变体` });
    delBtn.textContent = "✕";

    const bindingBox = el("div", { style: "display:flex;gap:8px;flex-wrap:wrap;align-items:center;" });
    const syncBinding = (): void => {
      bindingBox.replaceChildren(vr.source === "card_pool" ? vPoolSel : vTemplateSel);
    };
    syncBinding();
    const row = el(
      "div",
      { class: "rule-variant-row" },
      el(
        "div",
        { class: "rule-variant-grid" },
        el("label", {}, `规格名 ${idx + 1}`),
        nameInput,
        el("label", {}, "规格值(多个分隔)"),
        valuesInput,
        el("label", {}, "变体来源"),
        vSourceSel,
        el("label", {}, "绑定"),
        bindingBox,
        el("label", {}, "每件份数"),
        el("span", { style: "display:inline-flex;gap:4px;align-items:center;" }, unitsInput, el("span", { class: "muted", style: "font-size:var(--text-xs);" }, `(${UNITS_MIN}–${UNITS_MAX})`)),
        el("label", {}, "延时覆盖(秒)"),
        delayInput
      ),
      delBtn
    );
    nameInput.addEventListener("input", () => {
      vr.specName = nameInput.value;
      markDirty();
      refreshValidate();
    });
    valuesInput.addEventListener("input", () => {
      vr.specValues = valuesInput.value;
      markDirty();
      refreshValidate();
    });
    vSourceSel.addEventListener("change", () => {
      vr.source = vSourceSel.value === "template" ? "template" : "card_pool";
      if (vr.source === "card_pool") vr.templateId = "";
      else vr.cardPoolId = "";
      markDirty();
      rebuildVariants();
    });
    vPoolSel.addEventListener("change", () => {
      vr.cardPoolId = vPoolSel.value;
      markDirty();
      refreshValidate();
    });
    vTemplateSel.addEventListener("change", () => {
      vr.templateId = vTemplateSel.value;
      markDirty();
      refreshValidate();
    });
    unitsInput.addEventListener("input", () => {
      vr.unitsPerItem = unitsInput.value === "" ? Number.NaN : Number(unitsInput.value);
      markDirty();
      refreshValidate();
    });
    delayInput.addEventListener("input", () => {
      vr.delayOverride = delayInput.value;
      markDirty();
      refreshValidate();
    });
    delBtn.addEventListener("click", () => {
      v.variants.splice(idx, 1);
      markDirty();
      rebuildVariants();
      refreshValidate();
    });
    return row;
  };

  const rebuildVariants = (): void => {
    variantHost.replaceChildren(...v.variants.map((_, i) => buildVariantRow(i)));
    if (v.variants.length === 0) {
      variantHost.append(el("div", { class: "muted", style: "font-size:var(--text-xs);" }, "多规格商品可按规格名/规格值匹配不同发货内容(规格名与规格值全空 = 兜底变体)"));
    }
  };

  addVariantBtn.addEventListener("click", () => {
    v.variants.push({
      specName: "",
      specValues: "",
      source: "card_pool",
      cardPoolId: "",
      templateId: "",
      unitsPerItem: 1,
      delayOverride: ""
    });
    markDirty();
    rebuildVariants();
    refreshValidate();
  });

  // ---- 求评字段写回 ----
  waitInput.addEventListener("input", () => {
    v.review.waitHours = waitInput.value === "" ? Number.NaN : Number(waitInput.value);
    markDirty();
    refreshValidate();
  });
  intervalInput.addEventListener("input", () => {
    v.review.intervalHours = intervalInput.value === "" ? Number.NaN : Number(intervalInput.value);
    markDirty();
    refreshValidate();
  });
  maxCountInput.addEventListener("input", () => {
    v.review.maxCount = maxCountInput.value === "" ? Number.NaN : Number(maxCountInput.value);
    markDirty();
    refreshValidate();
  });
  reviewTextArea.addEventListener("input", () => {
    v.review.text = reviewTextArea.value;
    markDirty();
    refreshValidate();
  });

  // ---- 触发/来源/范围切换显隐 ----
  const syncVisibility = (): void => {
    const isReview = v.triggerType === "review_missing_timeout";
    sourceBlock.hidden = isReview;
    variantsBlock.hidden = isReview;
    reviewBlock.hidden = !isReview;
    if (!isReview) {
      fixedBox.hidden = v.contentSource !== "fixed_text";
      poolBox.hidden = v.contentSource !== "card_pool";
      templateBox.hidden = v.contentSource !== "template";
      previewRow.hidden = v.contentSource === "card_pool";
    }
    allItemsBlock.hidden = v.itemId !== "";
    allItemsWarn.hidden = allItemsCheck.checked;
  };

  triggerSel.addEventListener("change", () => {
    v.triggerType = triggerSel.value === "review_missing_timeout" ? "review_missing_timeout" : "order_paid";
    markDirty();
    syncVisibility();
    refreshValidate();
  });
  sourceSel.addEventListener("change", () => {
    const next = sourceSel.value;
    v.contentSource = next === "card_pool" || next === "template" ? next : "fixed_text";
    markDirty();
    syncVisibility();
    refreshValidate();
  });
  itemSel.addEventListener("change", () => {
    v.itemId = itemSel.value;
    markDirty();
    syncVisibility();
    refreshValidate();
  });
  skuInput.addEventListener("input", () => {
    v.skuKey = skuInput.value;
    markDirty();
  });
  allItemsCheck.addEventListener("change", () => {
    v.allItemsConfirmed = allItemsCheck.checked;
    markDirty();
    syncVisibility();
  });
  priorityInput.addEventListener("input", () => {
    v.priority = priorityInput.value === "" ? Number.NaN : Number(priorityInput.value);
    markDirty();
    refreshValidate();
  });
  contentArea.addEventListener("input", () => {
    v.content = contentArea.value;
    markDirty();
    refreshContentError();
    refreshValidate();
  });
  poolSel.addEventListener("change", () => {
    v.cardPoolId = poolSel.value;
    markDirty();
    refreshValidate();
  });
  templateSel.addEventListener("change", () => {
    v.templateId = templateSel.value;
    markDirty();
    refreshValidate();
  });

  // ---- 内容预演(fixed_text 原行为逐字保留;模板走掩码渲染;卡密组无预演入口)----
  previewBtn.addEventListener("click", async () => {
    previewBtn.disabled = true;
    try {
      const body: Record<string, unknown> = {
        item_id: v.itemId,
        sku_key: v.skuKey,
        content: v.contentSource === "fixed_text" ? contentArea.value : ""
      };
      if (v.contentSource === "template") {
        body["template_id"] = v.templateId;
      }
      const res = (await httpPost(`/api/v1/accounts/${opts.accountId}/rules/preview`, body)) as Record<string, unknown>;
      previewBox.style.display = "block";
      const violations = Array.isArray(res["violations"]) ? res["violations"] : [];
      if (res["valid"] === true) {
        const rendered = Array.isArray(res["rendered_messages"])
          ? (res["rendered_messages"] as unknown[]).map((m) => String(m))
          : [String(res["rendered_text"] ?? "")];
        previewBox.replaceChildren(
          el("div", { class: "tone-normal", style: "margin-bottom:4px;" }, `✓ 内容合法;发送内容预览(${rendered.length} 条消息,卡密内容掩码显示):`),
          el("span", {}, rendered.join("\n————\n"))
        );
      } else {
        previewBox.replaceChildren(
          el("div", { class: "error-text", style: "margin-bottom:4px;" }, "✗ 内容不合法:"),
          ...violations.map((x) =>
            el("div", { class: "error-text" }, String((x as Record<string, unknown>)["message"] ?? x))
          )
        );
      }
    } catch (e) {
      previewBox.style.display = "block";
      previewBox.replaceChildren(
        el("span", { class: "error-text" }, `预演失败:${e instanceof Error ? e.message : String(e)}`)
      );
    } finally {
      previewBtn.disabled = false;
    }
  });
  /** 预演行独立于三个来源盒:固定文字与模板可预演,卡密组不提供(内容不回显)。 */
  const previewRow = el(
    "div",
    { class: "field" },
    el("label", {}, "内容预演"),
    el("div", { style: "display:flex;justify-content:flex-start;" }, previewBtn),
    el("div", { class: "hint" }, "由服务端预演端点判定;固定文字按原文,模板逐条掩码渲染(卡密内容显示 [卡密内容 ×N])"),
    previewBox
  );

  // ---- 校验汇总(行内错误列表;宿主据此门禁保存)----
  const errorSummary = el("ul", { class: "field-error", style: "list-style:none;margin:0;padding:0;display:none;" });
  const refreshValidate = (): string[] => {
    const errors = validateRuleForm(snapshot());
    if (v.triggerType !== "review_missing_timeout" && v.contentSource === "fixed_text") {
      refreshContentError();
    }
    if (errors.length > 0) {
      errorSummary.style.display = "block";
      errorSummary.replaceChildren(...errors.map((m) => el("li", {}, `• ${m}`)));
    } else {
      errorSummary.style.display = "none";
      errorSummary.replaceChildren();
    }
    return errors;
  };

  const snapshot = (): RuleFormValue => ({
    ...v,
    variants: v.variants.map((vr) => ({ ...vr })),
    review: { ...v.review }
  });

  // ---- 装配 ----
  const enabledCheck = el("input", { type: "checkbox" }) as HTMLInputElement;
  enabledCheck.checked = v.enabled;
  enabledCheck.addEventListener("change", () => {
    v.enabled = enabledCheck.checked;
    markDirty();
  });
  fixedBox.append(
    field(
      "发货内容(固定文本)",
      contentArea,
      el("div", { style: "display:flex;justify-content:flex-end;" }, charCount),
      el("div", { class: "hint" }, "当前为纯固定文本:按原样发送给买家,不支持占位符替换")
    ),
    contentError
  );
  poolBox.append(
    field("卡密组", poolSel, poolHint)
  );
  templateBox.append(
    field("发货模板", templateSel, templateHint)
  );
  sourceBlock.append(
    field("内容来源", sourceSel, el("div", { class: "hint" }, "固定文字 / 卡密库存 / 发货模板三选一;带变体时规则级绑定可留空")),
    fixedBox,
    poolBox,
    templateBox,
    previewRow
  );
  variantsBlock.append(
    el("h3", { style: "margin:16px 0 4px;" }, "变体列表(可选)"),
    variantHost,
    el("div", { style: "display:flex;justify-content:flex-start;" }, addVariantBtn)
  );
  reviewBlock.append(
    el("h3", { style: "margin:16px 0 4px;" }, "求评计划"),
    field("发货后等待(小时)", waitInput, el("div", { class: "hint" }, "已交付订单超过该时长仍未评价时发送求评文案")),
    field("再次求评间隔(小时)", intervalInput),
    field("最多次数", maxCountInput, el("div", { class: "hint" }, "1–10 次;达到上限后停止")),
    field("求评文案", reviewTextArea, el("div", { class: "hint" }, "同时作为规则固定内容保存(编辑器不回读既有正文,保存将整体替换)"))
  );
  rebuildVariants();

  const matchBox = el("div", {});
  const matchItem = opts.items.find((it) => it.id === v.itemId);
  renderMatchPreview(matchBox, opts.accountId, [...opts.items], matchItem === undefined ? {} : { item: matchItem, skuKey: v.skuKey });

  container.append(
    field("触发类型", triggerSel, el("div", { class: "hint" }, "「评价后发送赠品」需平台评价事件能力验证后开放,当前未开放")),
    field(
      "优先级",
      priorityInput,
      el("div", { class: "hint" }, `${PRIORITY_MIN}–${PRIORITY_MAX},数字越小优先级越高;同范围同触发同优先级会被拒绝`)
    ),
    field(
      "商品范围",
      itemSel,
      skuInput,
      el("div", { class: "hint" }, "账号级=覆盖全部商品(需下方确认);规格键多规格商品填完整规格组合,默认规格留空")
    ),
    allItemsBlock,
    sourceBlock,
    variantsBlock,
    reviewBlock,
    el("label", { class: "check-row", style: "margin-top:12px;" }, enabledCheck, "启用该规则"),
    errorSummary,
    el("h3", { style: "margin-top:16px;" }, "匹配预览"),
    el("p", { class: "muted", style: "font-size:var(--text-xs);" }, "由服务端交付引擎判定;界面不复现匹配逻辑"),
    matchBox
  );
  syncVisibility();
  refreshValidate();

  return {
    validate: () => refreshValidate(),
    getValue: snapshot,
    buildPayload: (expectedVersion: number | null) => buildRulePayload(snapshot(), expectedVersion)
  };
}
