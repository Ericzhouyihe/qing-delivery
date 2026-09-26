/** catalog(007 T040 起):原「商品与规则」合并页已拆分为 features/items(商品列表)
 *  与 features/rules(自动化规则);本目录保留商品/规则共享内核:
 *  products(商品契约/筛选/同步)、rule-editor(可复用规则表单 renderRuleForm)、
 *  match-preview(匹配预演,零改动)。 */
export { parseItem, filterItems, parseItem as parseProductItem, wireSyncButton, type ProductItem, type AccountRef } from "./products";
export {
  CONTENT_LIMITS,
  validateContent,
  renderRuleForm,
  validateRuleForm,
  buildRulePayload,
  splitSpecValues,
  defaultRuleFormValue,
  type RuleFormValue,
  type RuleFormOptions,
  type RuleFormHandle,
  type RuleFormRefOption
} from "./rule-editor";
export { parseMatchOutcome, renderMatchOutcome } from "./match-preview";
