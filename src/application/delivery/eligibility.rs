//! 交付资格核验(T030):执行前必须核对全部条件(data-model「订单与资格」)。
//! 纯函数:输入快照化的账号/订单/规则/商品事实,输出资格判定;
//! 不满足时给出可读原因,不发、不消耗重试预算。

use crate::domain::orders::snapshot::{Ineligibility, OrderSnapshot};

/// 资格核验所需的最小事实集合(由调用方从仓储快照化读取)。
pub struct EligibilityInput<'a> {
    pub account_status: &'a str,
    pub runtime_enabled: bool,
    pub auto_delivery_enabled: bool,
    /// 恢复隔离/隔离期中的账号不允许自动交付(FR-017、data-model)
    pub restore_quarantined: bool,
    /// 账号首次监控生效时间;paid_at 早于它且非显式接管 → 历史
    pub monitor_since_ms: Option<i64>,
    pub snapshot: &'a OrderSnapshot,
    /// 商品在售状态(on_sale/off_sale/unknown)
    pub item_listing_state: &'a str,
    /// 命中的启用规则;None 表示无规则或歧义
    pub enabled_rule: Option<&'a str>,
    /// 规则范围是否存在多条启用(执行时歧义检测,FR-008)
    pub rule_ambiguous: bool,
    /// 007 US3(T034):命中规则需确认(账号级未确认)或需重新配置(引用缺失)
    /// → 需配置详情,转待处理不执行;Some 时优先于规则命中判定
    pub rule_needs_config: Option<&'a str>,
    /// 显式历史接管(FR-017):允许 paid_at 早于监控起点,但不放宽其他核验
    pub allow_historical: bool,
}

#[derive(Debug, PartialEq)]
pub enum Eligibility {
    /// 资格成立,附命中规则 ID
    Eligible(String),
    /// 不合格:进入待处理,不发送
    NotEligible(Reason),
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Reason {
    #[error("账号未在线(当前:{0})")]
    AccountNotOnline(String),
    #[error("账号运行开关关闭")]
    RuntimeDisabled,
    #[error("自动交付开关关闭")]
    AutoDeliveryDisabled,
    #[error("账号处于恢复隔离")]
    RestoreQuarantined,
    #[error("订单事实不完整:{0}")]
    IncompleteFacts(Ineligibility),
    #[error("可信付款时间早于监控生效时间(历史订单,需逐单接管)")]
    HistoricalOrder,
    #[error("商品已下架")]
    ItemOffSale,
    #[error("无启用规则匹配完整规格组合")]
    NoRule,
    #[error("规则范围歧义(多条启用),不得任选一条")]
    RuleAmbiguous,
    /// 007 US3(T034):规则需确认·暂不发货(账号级未确认)或需重新配置(引用缺失)
    #[error("{0}")]
    RuleNeedsConfig(String),
}

pub fn verify(input: &EligibilityInput<'_>) -> Eligibility {
    if input.restore_quarantined {
        return Eligibility::NotEligible(Reason::RestoreQuarantined);
    }
    if input.account_status != "online" {
        return Eligibility::NotEligible(Reason::AccountNotOnline(
            input.account_status.to_string(),
        ));
    }
    if !input.runtime_enabled {
        return Eligibility::NotEligible(Reason::RuntimeDisabled);
    }
    if !input.auto_delivery_enabled {
        return Eligibility::NotEligible(Reason::AutoDeliveryDisabled);
    }
    if let Err(e) = input.snapshot.completeness() {
        return Eligibility::NotEligible(Reason::IncompleteFacts(e));
    }
    // 监控起点比较可信 paid_at;缺 paid_at 已由完备性拦截
    if let (Some(paid), Some(monitor_since)) =
        (input.snapshot.paid_at_ms.verified(), input.monitor_since_ms)
        && *paid < monitor_since
        && !input.allow_historical
    {
        return Eligibility::NotEligible(Reason::HistoricalOrder);
    }
    if input.item_listing_state == "off_sale" {
        return Eligibility::NotEligible(Reason::ItemOffSale);
    }
    if input.rule_ambiguous {
        return Eligibility::NotEligible(Reason::RuleAmbiguous);
    }
    if let Some(detail) = input.rule_needs_config {
        // FR-032/data-model:需确认或需重新配置的规则旁路执行,转待处理
        return Eligibility::NotEligible(Reason::RuleNeedsConfig(detail.to_string()));
    }
    match input.enabled_rule {
        Some(rule_id) => Eligibility::Eligible(rule_id.to_string()),
        None => Eligibility::NotEligible(Reason::NoRule),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::Money;
    use crate::domain::orders::snapshot::{
        FieldStatus, PlatformOrderState, SnapshotBuilder, TradeType,
    };

    fn complete_snapshot() -> SnapshotBuilder {
        SnapshotBuilder {
            platform_order_id: "ORD-1".into(),
            seller_id: FieldStatus::Verified("seller-1".into()),
            buyer_id: FieldStatus::Verified("buyer-1".into()),
            item_id: FieldStatus::Verified("item-1".into()),
            trade_type: FieldStatus::Verified(TradeType::Ordinary),
            platform_state: FieldStatus::Verified(PlatformOrderState::PendingShip),
            paid_at_ms: FieldStatus::Verified(1_760_000_000_000),
            amount: FieldStatus::Verified(Money::new(100, "CNY").unwrap()),
            quantity: FieldStatus::Verified(1),
            sku_parts: FieldStatus::Missing,
            sku_single: true,
            conversation_verified: true,
            source: "detail".into(),
            observed_at_ms: 1_760_000_001_000,
            browser_supplemented: false,
        }
    }

    fn input<'a>(snapshot: &'a OrderSnapshot) -> EligibilityInput<'a> {
        EligibilityInput {
            allow_historical: false,
            account_status: "online",
            runtime_enabled: true,
            auto_delivery_enabled: true,
            restore_quarantined: false,
            monitor_since_ms: Some(1_700_000_000_000),
            snapshot,
            item_listing_state: "on_sale",
            enabled_rule: Some("rule-1"),
            rule_ambiguous: false,
            rule_needs_config: None,
        }
    }

    #[test]
    fn full_conditions_pass() {
        let s = complete_snapshot().build();
        assert_eq!(verify(&input(&s)), Eligibility::Eligible("rule-1".into()));
    }

    #[test]
    fn switches_and_status_gate_delivery() {
        let s = complete_snapshot().build();
        let mut i = input(&s);
        i.auto_delivery_enabled = false;
        assert_eq!(
            verify(&i),
            Eligibility::NotEligible(Reason::AutoDeliveryDisabled)
        );

        let mut i = input(&s);
        i.account_status = "needs_verification";
        assert!(matches!(
            verify(&i),
            Eligibility::NotEligible(Reason::AccountNotOnline(_))
        ));

        let mut i = input(&s);
        i.restore_quarantined = true;
        assert_eq!(
            verify(&i),
            Eligibility::NotEligible(Reason::RestoreQuarantined)
        );
    }

    #[test]
    fn pre_monitor_payment_is_historical() {
        let mut b = complete_snapshot();
        b.paid_at_ms = FieldStatus::Verified(1_600_000_000_000);
        let s = b.build();
        let mut i = input(&s);
        i.monitor_since_ms = Some(1_700_000_000_000);
        assert_eq!(
            verify(&i),
            Eligibility::NotEligible(Reason::HistoricalOrder)
        );
    }

    #[test]
    fn off_sale_and_ambiguous_rule_rejected() {
        let s = complete_snapshot().build();
        let mut i = input(&s);
        i.item_listing_state = "off_sale";
        assert_eq!(verify(&i), Eligibility::NotEligible(Reason::ItemOffSale));

        let mut i = input(&s);
        i.rule_ambiguous = true;
        assert_eq!(verify(&i), Eligibility::NotEligible(Reason::RuleAmbiguous));

        let mut i = input(&s);
        i.enabled_rule = None;
        assert_eq!(verify(&i), Eligibility::NotEligible(Reason::NoRule));
    }

    /// 007 US3(T034):需确认/需重新配置的规则旁路执行,转待处理。
    #[test]
    fn needs_config_rule_bypasses_execution() {
        let s = complete_snapshot().build();
        let mut i = input(&s);
        i.rule_needs_config = Some("账号级规则未确认,需确认·暂不发货");
        assert_eq!(
            verify(&i),
            Eligibility::NotEligible(Reason::RuleNeedsConfig(
                "账号级规则未确认,需确认·暂不发货".into()
            ))
        );
    }
}
