//! 007 US3 规则扩展领域逻辑(T029/T031,research D1 + 001 FR-008 修订):
//! 多条启用并存 + 优先级选取(数字越小越高)、多规格变体匹配
//! (变体 (spec_name→分号分隔值集合) ⊆ 订单 sku_pairs 才命中;无变体=适用全部规格)、
//! 同优先级冲突判定(同范围同触发同优先级拒绝保存)、账号级 all_items_confirmed 门禁
//! (FR-032:未确认 = 需确认·暂不发货)。
//! 纯业务决策:不接触 SQL/HTTP/存储(宪章 II)。

use crate::domain::sku::SkuPart;

/// 默认优先级(数字越小越高;001 存量行即此值,零迁移)
pub const DEFAULT_PRIORITY: i64 = 100;
pub const MIN_PRIORITY: i64 = 1;
pub const MAX_PRIORITY: i64 = 10_000;
/// 变体每件份数上下限(data-model rule_variants.units_per_item CHECK 1–100)
pub const MIN_UNITS_PER_ITEM: i64 = 1;
pub const MAX_UNITS_PER_ITEM: i64 = 100;
/// 延时覆盖上限(与卡密组组级延时同口径 0–3600,FR-014)
pub const MAX_DELAY_OVERRIDE_SECONDS: i64 = 3_600;

pub const TRIGGER_ORDER_PAID: &str = "order_paid";
pub const TRIGGER_REVIEW_MISSING_TIMEOUT: &str = "review_missing_timeout";
pub const TRIGGER_BUYER_REVIEWED: &str = "buyer_reviewed";

/// 触发类型(buyer_reviewed 仅能力门禁验证后可创建,FR-036;
/// 域层只做解析,创建侧门禁在应用层)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TriggerType {
    #[default]
    OrderPaid,
    ReviewMissingTimeout,
    BuyerReviewed,
}

impl TriggerType {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerType::OrderPaid => TRIGGER_ORDER_PAID,
            TriggerType::ReviewMissingTimeout => TRIGGER_REVIEW_MISSING_TIMEOUT,
            TriggerType::BuyerReviewed => TRIGGER_BUYER_REVIEWED,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            TRIGGER_ORDER_PAID => Some(TriggerType::OrderPaid),
            TRIGGER_REVIEW_MISSING_TIMEOUT => Some(TriggerType::ReviewMissingTimeout),
            TRIGGER_BUYER_REVIEWED => Some(TriggerType::BuyerReviewed),
            _ => None,
        }
    }
}

/// 规则级内容来源(FR-039:固定内容与卡密组、发货模板并存为并列来源,
/// 存量 fixed_text 数据零迁移)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentSource {
    #[default]
    FixedText,
    CardPool,
    Template,
}

impl ContentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentSource::FixedText => "fixed_text",
            ContentSource::CardPool => "card_pool",
            ContentSource::Template => "template",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fixed_text" => Some(ContentSource::FixedText),
            "card_pool" => Some(ContentSource::CardPool),
            "template" => Some(ContentSource::Template),
            _ => None,
        }
    }
}

/// 变体内容来源(与规则级来源并列;FR-039 固定内容与卡密/模板并存)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantSource {
    CardPool,
    Template,
}

impl VariantSource {
    pub fn as_str(self) -> &'static str {
        match self {
            VariantSource::CardPool => "card_pool",
            VariantSource::Template => "template",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "card_pool" => Some(VariantSource::CardPool),
            "template" => Some(VariantSource::Template),
            _ => None,
        }
    }
}

/// 规则变体(领域视图;spec_value 分号分隔值集合由仓储层拆装)。
/// spec_name 与 spec_values 全空 = 兜底变体(适用该规则全部规格)。
#[derive(Clone, Debug, PartialEq)]
pub struct RuleVariant {
    pub spec_name: String,
    pub spec_values: Vec<String>,
    pub source: VariantSource,
    pub card_pool_id: Option<String>,
    pub template_id: Option<String>,
    /// `{cards:[{key,pool_id,units}],custom:{k:v}}` JSON(source=template 时使用)
    pub template_bindings: Option<String>,
    pub units_per_item: i64,
    pub delay_override_seconds: Option<i64>,
}

/// 求评计划参数(FR-035:wait_hours/interval_hours ≥1、max_count 1..=10、文案非空)。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewConfig {
    pub wait_hours: i64,
    pub interval_hours: i64,
    pub max_count: i64,
    pub text: String,
}

/// 匹配候选(应用层从仓储装载的投影;仓储负责 trigger_type/enabled 过滤,
/// 领域负责层级/优先级/变体/门禁的最终裁决)。
#[derive(Clone, Debug, PartialEq)]
pub struct RuleCandidate {
    pub id: String,
    pub item_id: String,
    pub sku_key: String,
    pub priority: i64,
    pub all_items_confirmed: bool,
    pub needs_reconfiguration: bool,
    pub variants: Vec<RuleVariant>,
}

/// 订单侧匹配范围事实。
pub struct MatchScope<'a> {
    pub item_id: &'a str,
    pub sku_key: &'a str,
    pub sku_parts: &'a [SkuPart],
}

/// 匹配层级:商品精确规格 → 商品任意规格(变体规则所在层)→ 账号级回退。
/// 商品级优先于账号级回退(D1/research T034)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScopeTier {
    ItemExact,
    ItemAnySku,
    Account,
}

/// 匹配裁决。
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// 执行该规则;命中变体时携带变体来源(覆盖规则级默认,T034 注入 ContentPlan)
    Execute {
        rule_id: String,
        variant: Option<RuleVariant>,
    },
    /// 账号级规则未确认「适用于全部商品」:需确认·暂不发货(FR-032)
    NeedsConfirmation { rule_id: String },
    /// 规则引用缺失(卡组/模板/商品被删):需重新配置,旁路执行
    NeedsReconfiguration { rule_id: String },
    /// 同层级同优先级多条启用(唯一索引正常时不可达;防御性拒绝任选)
    Ambiguous { rule_ids: Vec<String> },
    /// 无适用规则
    NoRule,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RulesExtError {
    #[error("优先级必须在 {MIN_PRIORITY}..={MAX_PRIORITY}(数字越小越高)")]
    InvalidPriority,
    #[error("每件份数必须在 {MIN_UNITS_PER_ITEM}..={MAX_UNITS_PER_ITEM}")]
    InvalidUnits,
    #[error("延时覆盖必须在 0..={MAX_DELAY_OVERRIDE_SECONDS} 秒")]
    InvalidDelay,
    #[error("求评配置无效:{0}")]
    InvalidReviewConfig(&'static str),
}

fn name_matches(part: &SkuPart, spec_name: &str) -> bool {
    // 规格名同时接受平台标签与属性 ID(不同来源的变体配置习惯不一)
    !spec_name.is_empty() && (part.property_label == spec_name || part.property_id == spec_name)
}

fn value_matches(part: &SkuPart, spec_value: &str) -> bool {
    part.value_label == spec_value || part.value_id == spec_value
}

/// 变体命中:变体的 (spec_name→值集合) 对出现在订单 sku_pairs 中才命中;
/// spec_name 与值集合全空 = 兜底变体(适用全部规格)。
pub fn variant_matches(variant: &RuleVariant, parts: &[SkuPart]) -> bool {
    let has_values = variant.spec_values.iter().any(|v| !v.is_empty());
    if variant.spec_name.is_empty() && !has_values {
        // 兜底变体:无规格限定 = 适用该规则全部规格(与「无变体」规则级语义对齐)
        return true;
    }
    if variant.spec_name.is_empty() {
        // 有值无名:无法与订单规格对齐,不猜测
        return false;
    }
    parts
        .iter()
        .any(|p| name_matches(p, &variant.spec_name)
            && variant
                .spec_values
                .iter()
                .any(|v| !v.is_empty() && value_matches(p, v)))
}

/// 候选的匹配层级;与本单范围无关的候选返回 None(调用方跳过)。
pub fn tier_of(candidate: &RuleCandidate, scope: &MatchScope<'_>) -> Option<ScopeTier> {
    if candidate.item_id.is_empty() {
        // 账号级哨兵(item_id=''):覆盖全部商品,任意规格
        Some(ScopeTier::Account)
    } else if candidate.item_id != scope.item_id {
        None // 其他商品的规则不参与本单裁决
    } else if candidate.sku_key == scope.sku_key {
        Some(ScopeTier::ItemExact)
    } else if candidate.sku_key.is_empty() {
        Some(ScopeTier::ItemAnySku)
    } else {
        None // 同商品其他规格组合的规则不参与本单裁决
    }
}

/// 优先级选取 + 变体命中 + 账号级确认门禁 + 需重新配置旁路(T034 匹配核心)。
/// 输入任意顺序;层级(商品精确→商品任意规格→账号级)优先于优先级数字。
pub fn select(scope: &MatchScope<'_>, candidates: &[RuleCandidate]) -> Decision {
    let mut ordered: Vec<(ScopeTier, &RuleCandidate)> = candidates
        .iter()
        .filter_map(|c| tier_of(c, scope).map(|tier| (tier, c)))
        .collect::<Vec<_>>();
    // 层级 → 优先级升序 → id 稳定排序(数字越小越高;同层取第一条)
    ordered.sort_by(|(ta, a), (tb, b)| {
        ta.cmp(tb)
            .then(a.priority.cmp(&b.priority))
            .then(a.id.cmp(&b.id))
    });
    // 同层同优先级多条启用:唯一索引正常时不可达,防御性拒绝任选(D1 语义)
    for pair in ordered.windows(2) {
        let ((ta, a), (tb, b)) = (pair[0], pair[1]);
        if ta == tb && a.priority == b.priority && a.id != b.id {
            return Decision::Ambiguous {
                rule_ids: vec![a.id.clone(), b.id.clone()],
            };
        }
    }
    for (_, c) in ordered {
        // 变体均不适用本单规格 → 该规则对本单不适用,回退下一候选
        let hit = c
            .variants
            .iter()
            .find(|v| variant_matches(v, scope.sku_parts));
        if !c.variants.is_empty() && hit.is_none() {
            continue;
        }
        if c.needs_reconfiguration {
            // 引用缺失:规则已选中(最高优先),旁路执行转待处理,不落下一层
            return Decision::NeedsReconfiguration {
                rule_id: c.id.clone(),
            };
        }
        if c.item_id.is_empty() && !c.all_items_confirmed {
            // 账号级未显式确认「适用于全部商品」:需确认·暂不发货(FR-032)
            return Decision::NeedsConfirmation {
                rule_id: c.id.clone(),
            };
        }
        return Decision::Execute {
            rule_id: c.id.clone(),
            variant: hit.cloned(),
        };
    }
    Decision::NoRule
}

/// 同范围同触发同优先级冲突判定(保存侧 422 rule_conflict 语义,D1)。
/// enabled = 既有启用规则的 (id, priority) 清单;rule_id 自身排除(更新场景)。
pub fn same_priority_conflict(enabled: &[(String, i64)], rule_id: &str, priority: i64) -> bool {
    enabled
        .iter()
        .any(|(id, p)| *p == priority && id != rule_id)
}

pub fn validate_priority(priority: i64) -> Result<i64, RulesExtError> {
    if (MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
        Ok(priority)
    } else {
        Err(RulesExtError::InvalidPriority)
    }
}

pub fn validate_units_per_item(units: i64) -> Result<i64, RulesExtError> {
    if (MIN_UNITS_PER_ITEM..=MAX_UNITS_PER_ITEM).contains(&units) {
        Ok(units)
    } else {
        Err(RulesExtError::InvalidUnits)
    }
}

pub fn validate_delay_override(seconds: Option<i64>) -> Result<Option<i64>, RulesExtError> {
    match seconds {
        None => Ok(None),
        Some(s) if (0..=MAX_DELAY_OVERRIDE_SECONDS).contains(&s) => Ok(Some(s)),
        Some(_) => Err(RulesExtError::InvalidDelay),
    }
}

pub fn validate_review_config(config: &ReviewConfig) -> Result<(), RulesExtError> {
    if config.wait_hours < 1 {
        return Err(RulesExtError::InvalidReviewConfig("发货后等待小时数须 ≥1"));
    }
    if config.interval_hours < 1 {
        return Err(RulesExtError::InvalidReviewConfig("再次求评间隔小时数须 ≥1"));
    }
    if !(1..=10).contains(&config.max_count) {
        return Err(RulesExtError::InvalidReviewConfig("最多次数须为 1..=10"));
    }
    if config.text.trim().is_empty() {
        return Err(RulesExtError::InvalidReviewConfig("求评文案不能为空"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(pid: &str, vid: &str, plabel: &str, vlabel: &str) -> SkuPart {
        SkuPart {
            property_id: pid.into(),
            value_id: vid.into(),
            property_label: plabel.into(),
            value_label: vlabel.into(),
        }
    }

    fn scope<'a>(item: &'a str, sku: &'a str, parts: &'a [SkuPart]) -> MatchScope<'a> {
        MatchScope {
            item_id: item,
            sku_key: sku,
            sku_parts: parts,
        }
    }

    fn candidate(id: &str, item: &str, sku: &str, priority: i64) -> RuleCandidate {
        RuleCandidate {
            id: id.into(),
            item_id: item.into(),
            sku_key: sku.into(),
            priority,
            all_items_confirmed: true,
            needs_reconfiguration: false,
            variants: Vec::new(),
        }
    }

    fn pool_variant(name: &str, values: &[&str], pool: &str, units: i64) -> RuleVariant {
        RuleVariant {
            spec_name: name.into(),
            spec_values: values.iter().map(|s| s.to_string()).collect(),
            source: VariantSource::CardPool,
            card_pool_id: Some(pool.into()),
            template_id: None,
            template_bindings: None,
            units_per_item: units,
            delay_override_seconds: None,
        }
    }

    // ---- 变体匹配 ----

    #[test]
    fn variant_pair_must_appear_in_order_sku_pairs() {
        let red = pool_variant("颜色", &["红色", "蓝色"], "pool-red", 1);
        // ⊆:订单含 颜色=红色 → 命中
        let parts = vec![part("p1", "v1", "颜色", "红色")];
        assert!(variant_matches(&red, &parts));
        // 订单 颜色=绿色 → 不命中
        let parts = vec![part("p1", "v2", "颜色", "绿色")];
        assert!(!variant_matches(&red, &parts));
        // 订单含更多规格对(超集)仍命中
        let parts = vec![
            part("p1", "v1", "颜色", "红色"),
            part("p2", "v9", "套餐", "基础版"),
        ];
        assert!(variant_matches(&red, &parts));
        // 订单不含该规格名 → 不命中
        let parts = vec![part("p2", "v9", "套餐", "基础版")];
        assert!(!variant_matches(&red, &parts));
        // 空订单 → 不命中(具体变体)
        assert!(!variant_matches(&red, &[]));
    }

    #[test]
    fn variant_matches_by_platform_ids_when_labels_missing() {
        // 事实快照可能只带 ID(标签缺失):规格名/值按 ID 同样可命中
        let by_id = pool_variant("1627207", &["3232483"], "pool-x", 1);
        let parts = vec![part("1627207", "3232483", "", "")];
        assert!(variant_matches(&by_id, &parts));
        // 混合:变体用标签、事实只有 ID → 不命中(不猜测)
        let by_label = pool_variant("颜色", &["红色"], "pool-x", 1);
        assert!(!variant_matches(&by_label, &parts));
    }

    #[test]
    fn catch_all_variant_applies_to_any_spec() {
        let fallback = pool_variant("", &[""], "pool-any", 1);
        assert!(variant_matches(&fallback, &[]), "空订单命中兜底变体");
        let parts = vec![part("p1", "v1", "颜色", "红色")];
        assert!(variant_matches(&fallback, &parts));
    }

    // ---- 优先级选取与层级 ----

    #[test]
    fn same_scope_picks_smallest_priority() {
        let lo = candidate("r-low", "item-1", "single", 50);
        let hi = candidate("r-high", "item-1", "single", 100);
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        // 输入乱序也必须裁决一致
        let d = select(&s, &[hi.clone(), lo.clone()]);
        assert_eq!(
            d,
            Decision::Execute {
                rule_id: "r-low".into(),
                variant: None,
            }
        );
    }

    #[test]
    fn item_exact_beats_any_sku_beats_account_regardless_of_priority() {
        // 层级(商品精确 → 商品任意规格 → 账号级)优先于优先级数字
        let account = candidate("r-acct", "", "", 1);
        let any_sku = candidate("r-any", "item-1", "", 1);
        let exact = candidate("r-exact", "item-1", "single", 999);
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        let d = select(&s, &[account, any_sku, exact]);
        assert_eq!(
            d,
            Decision::Execute {
                rule_id: "r-exact".into(),
                variant: None,
            },
            "商品精确规格规则优先于账号级回退(即使优先级数字更大)"
        );
        // 无精确规则时:商品任意规格层优先于账号级
        let any_sku = candidate("r-any", "item-1", "", 999);
        let account = candidate("r-acct", "", "", 1);
        let d = select(&s, &[account, any_sku]);
        assert_eq!(
            d,
            Decision::Execute {
                rule_id: "r-any".into(),
                variant: None,
            }
        );
    }

    #[test]
    fn same_tier_same_priority_is_ambiguous() {
        let a = candidate("r-a", "item-1", "single", 100);
        let b = candidate("r-b", "item-1", "single", 100);
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        match select(&s, &[a, b]) {
            Decision::Ambiguous { rule_ids } => {
                assert_eq!(rule_ids.len(), 2);
            }
            other => panic!("同层级同优先级应判歧义,实际 {other:?}"),
        }
    }

    #[test]
    fn unrelated_scope_candidates_are_ignored() {
        // 其他商品/其他规格键的候选不参与裁决(仓储已过滤,此处防御)
        let other_item = candidate("r-other", "item-2", "single", 1);
        let other_sku = candidate("r-sku2", "item-1", "combo:other", 1);
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        assert_eq!(select(&s, &[other_item, other_sku]), Decision::NoRule);
    }

    // ---- 账号级确认门禁与需重新配置旁路 ----

    #[test]
    fn account_level_without_confirmation_needs_confirmation() {
        let mut acct = candidate("r-acct", "", "", 100);
        acct.all_items_confirmed = false;
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        assert_eq!(
            select(&s, &[acct.clone()]),
            Decision::NeedsConfirmation {
                rule_id: "r-acct".into()
            }
        );
        // 确认后执行
        let mut confirmed = acct;
        confirmed.all_items_confirmed = true;
        assert_eq!(
            select(&s, &[confirmed]),
            Decision::Execute {
                rule_id: "r-acct".into(),
                variant: None,
            }
        );
    }

    #[test]
    fn needs_reconfiguration_bypasses_execution() {
        let mut broken = candidate("r-broken", "item-1", "single", 100);
        broken.needs_reconfiguration = true;
        let parts: Vec<SkuPart> = Vec::new();
        let s = scope("item-1", "single", &parts);
        assert_eq!(
            select(&s, &[broken]),
            Decision::NeedsReconfiguration {
                rule_id: "r-broken".into()
            }
        );
    }

    // ---- 变体命中注入与落空回退 ----

    #[test]
    fn variant_hit_returns_effect_and_overrides_rule_default() {
        let mut rule = candidate("r-var", "item-1", "", 100);
        rule.variants = vec![
            pool_variant("颜色", &["红色"], "pool-red", 2),
            pool_variant("颜色", &["蓝色"], "pool-blue", 3),
        ];
        let parts = vec![part("p1", "v2", "颜色", "蓝色")];
        let s = scope("item-1", "combo:p1=v2", &parts);
        match select(&s, &[rule]) {
            Decision::Execute { rule_id, variant } => {
                assert_eq!(rule_id, "r-var");
                let v = variant.expect("命中变体应携带来源");
                assert_eq!(v.card_pool_id.as_deref(), Some("pool-blue"));
                assert_eq!(v.units_per_item, 3);
            }
            other => panic!("应变体命中,实际 {other:?}"),
        }
    }

    #[test]
    fn variant_mismatch_falls_through_to_account_fallback() {
        // 商品规则变体均不适用本单规格 → 回退下一候选(账号级)
        let mut item_rule = candidate("r-item", "item-1", "", 1);
        item_rule.variants = vec![pool_variant("颜色", &["红色"], "pool-red", 1)];
        let account = candidate("r-acct", "", "", 100);
        let parts = vec![part("p1", "v2", "颜色", "蓝色")];
        let s = scope("item-1", "combo:p1=v2", &parts);
        assert_eq!(
            select(&s, &[item_rule, account]),
            Decision::Execute {
                rule_id: "r-acct".into(),
                variant: None,
            }
        );
    }

    #[test]
    fn rule_without_variants_applies_to_all_specs() {
        let plain = candidate("r-plain", "item-1", "single", 100);
        let parts = vec![part("p1", "v1", "颜色", "红色")];
        let s = scope("item-1", "single", &parts);
        assert_eq!(
            select(&s, &[plain]),
            Decision::Execute {
                rule_id: "r-plain".into(),
                variant: None,
            }
        );
    }

    // ---- 同优先级冲突判定(保存侧) ----

    #[test]
    fn same_priority_conflict_for_save() {
        let enabled = vec![("r-a".to_string(), 100), ("r-b".to_string(), 200)];
        assert!(same_priority_conflict(&enabled, "r-new", 100), "同优先级冲突");
        assert!(!same_priority_conflict(&enabled, "r-new", 300), "不同优先级允许并存");
        assert!(
            !same_priority_conflict(&enabled, "r-a", 100),
            "更新自身排除"
        );
    }

    // ---- 校验边界 ----

    #[test]
    fn validation_bounds() {
        assert!(validate_priority(DEFAULT_PRIORITY).is_ok());
        assert!(validate_priority(MIN_PRIORITY).is_ok());
        assert!(validate_priority(MAX_PRIORITY).is_ok());
        assert!(validate_priority(0).is_err());
        assert!(validate_priority(MAX_PRIORITY + 1).is_err());

        assert!(validate_units_per_item(1).is_ok());
        assert!(validate_units_per_item(MAX_UNITS_PER_ITEM).is_ok());
        assert!(validate_units_per_item(0).is_err());
        assert!(validate_units_per_item(MAX_UNITS_PER_ITEM + 1).is_err());

        assert!(validate_delay_override(None).is_ok());
        assert!(validate_delay_override(Some(0)).is_ok());
        assert!(validate_delay_override(Some(MAX_DELAY_OVERRIDE_SECONDS)).is_ok());
        assert!(validate_delay_override(Some(-1)).is_err());
        assert!(validate_delay_override(Some(MAX_DELAY_OVERRIDE_SECONDS + 1)).is_err());
    }

    #[test]
    fn review_config_bounds() {
        let ok = ReviewConfig {
            wait_hours: 24,
            interval_hours: 12,
            max_count: 3,
            text: "感谢购买,期待好评".into(),
        };
        assert!(validate_review_config(&ok).is_ok());
        let mut bad = ok.clone();
        bad.wait_hours = 0;
        assert!(validate_review_config(&bad).is_err());
        let mut bad = ok.clone();
        bad.interval_hours = 0;
        assert!(validate_review_config(&bad).is_err());
        let mut bad = ok.clone();
        bad.max_count = 0;
        assert!(validate_review_config(&bad).is_err());
        let mut bad = ok.clone();
        bad.max_count = 11;
        assert!(validate_review_config(&bad).is_err());
        let mut bad = ok.clone();
        bad.text = "  ".into();
        assert!(validate_review_config(&bad).is_err());
    }

    #[test]
    fn trigger_type_roundtrip() {
        assert_eq!(TriggerType::default(), TriggerType::OrderPaid);
        assert_eq!(TriggerType::parse("order_paid"), Some(TriggerType::OrderPaid));
        assert_eq!(
            TriggerType::parse("review_missing_timeout"),
            Some(TriggerType::ReviewMissingTimeout)
        );
        assert_eq!(
            TriggerType::parse("buyer_reviewed"),
            Some(TriggerType::BuyerReviewed)
        );
        assert_eq!(TriggerType::parse("bogus"), None);
        assert_eq!(TriggerType::parse(TriggerType::BuyerReviewed.as_str()), Some(TriggerType::BuyerReviewed));
        assert_eq!(VariantSource::parse("card_pool"), Some(VariantSource::CardPool));
        assert_eq!(VariantSource::parse("template"), Some(VariantSource::Template));
        assert_eq!(VariantSource::parse("fixed_text"), None);
    }
}
