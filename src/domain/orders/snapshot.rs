//! OrderSnapshot:逐字段 verified/missing/conflict/unsupported 表达,
//! complete_for_delivery 只能由全部必要事实共同满足(platform-adapter §4)。
//! 数量缺失不是默认 1;金额缺失不当 0;paid_at 只能来自明确付款语义。

use crate::domain::money::Money;
use crate::domain::sku::SkuPart;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FieldStatus<T> {
    Verified(T),
    #[default]
    Missing,
    Conflict,
    Unsupported,
}

impl<T> FieldStatus<T> {
    pub fn verified(&self) -> Option<&T> {
        match self {
            FieldStatus::Verified(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_verified(&self) -> bool {
        matches!(self, FieldStatus::Verified(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderSnapshot {
    pub platform_order_id: String,
    pub seller_id: FieldStatus<String>,
    pub buyer_id: FieldStatus<String>,
    pub item_id: FieldStatus<String>,
    pub trade_type: FieldStatus<TradeType>,
    pub platform_state: FieldStatus<PlatformOrderState>,
    pub paid_at_ms: FieldStatus<i64>,
    pub amount: FieldStatus<Money>,
    pub quantity: FieldStatus<u32>,
    pub sku_parts: FieldStatus<Vec<SkuPart>>,
    /// 已证明单规格(平台明确事实)与规格未知必须区分
    pub sku_single: bool,
    pub conversation_verified: bool,
    pub source: String,
    pub observed_at_ms: i64,
    pub browser_supplemented: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeType {
    Ordinary,
    Bargain,
    Unknown,
    UnsupportedBundle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformOrderState {
    Unpaid,
    PendingShip,
    Shipped,
    Completed,
    Canceled,
    Refunding,
    Refunded,
    Unknown,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Ineligibility {
    #[error("买家身份未核验")]
    BuyerMissing,
    #[error("卖家/账号归属不一致")]
    SellerConflict,
    #[error("商品身份缺失或多商品组合不支持")]
    ItemUnsupported,
    #[error("交易类型不支持(仅普通交易)")]
    TradeTypeUnsupported,
    #[error("订单状态不可交付:{0:?}")]
    StateNotDeliverable(PlatformOrderState),
    #[error("付款时间不可信")]
    PaidAtMissing,
    #[error("金额未核验")]
    AmountMissing,
    #[error("数量未核验(缺失不是默认 1)")]
    QuantityMissing,
    #[error("规格不完整")]
    SkuIncomplete,
    #[error("会话身份未核验")]
    ConversationUnverified,
}

impl OrderSnapshot {
    /// 全部必要事实核验通过才算完整;任何缺失/冲突/不支持都返回具体原因。
    pub fn completeness(&self) -> Result<(), Ineligibility> {
        use FieldStatus::*;
        match (&self.seller_id, &self.buyer_id) {
            (Verified(_), Verified(_)) => {}
            (Conflict, _) | (_, Conflict) => return Err(Ineligibility::SellerConflict),
            _ => return Err(Ineligibility::BuyerMissing),
        }
        match &self.item_id {
            Verified(_) => {}
            Unsupported => return Err(Ineligibility::ItemUnsupported),
            _ => return Err(Ineligibility::ItemUnsupported),
        }
        match &self.trade_type {
            Verified(TradeType::Ordinary) => {}
            Verified(_) => return Err(Ineligibility::TradeTypeUnsupported),
            _ => return Err(Ineligibility::TradeTypeUnsupported),
        }
        match &self.platform_state {
            Verified(PlatformOrderState::PendingShip) => {}
            Verified(other) => return Err(Ineligibility::StateNotDeliverable(*other)),
            _ => {
                return Err(Ineligibility::StateNotDeliverable(
                    PlatformOrderState::Unknown,
                ));
            }
        }
        if !self.paid_at_ms.is_verified() {
            return Err(Ineligibility::PaidAtMissing);
        }
        if !self.amount.is_verified() {
            return Err(Ineligibility::AmountMissing);
        }
        if !self.quantity.is_verified() {
            return Err(Ineligibility::QuantityMissing);
        }
        match (&self.sku_parts, self.sku_single) {
            (Verified(_), _) => {}
            (Missing, false) | (Unsupported, _) => return Err(Ineligibility::SkuIncomplete),
            // Missing + single=true:平台明确单规格事实,可用空组合
            (Missing, true) => {}
            (Conflict, _) => return Err(Ineligibility::SkuIncomplete),
        }
        if !self.conversation_verified {
            return Err(Ineligibility::ConversationUnverified);
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct SnapshotBuilder {
    pub platform_order_id: String,
    pub seller_id: FieldStatus<String>,
    pub buyer_id: FieldStatus<String>,
    pub item_id: FieldStatus<String>,
    pub trade_type: FieldStatus<TradeType>,
    pub platform_state: FieldStatus<PlatformOrderState>,
    pub paid_at_ms: FieldStatus<i64>,
    pub amount: FieldStatus<Money>,
    pub quantity: FieldStatus<u32>,
    pub sku_parts: FieldStatus<Vec<SkuPart>>,
    pub sku_single: bool,
    pub conversation_verified: bool,
    pub source: String,
    pub observed_at_ms: i64,
    pub browser_supplemented: bool,
}

impl SnapshotBuilder {
    pub fn build(self) -> OrderSnapshot {
        OrderSnapshot {
            platform_order_id: self.platform_order_id,
            seller_id: self.seller_id,
            buyer_id: self.buyer_id,
            item_id: self.item_id,
            trade_type: self.trade_type,
            platform_state: self.platform_state,
            paid_at_ms: self.paid_at_ms,
            amount: self.amount,
            quantity: self.quantity,
            sku_parts: self.sku_parts,
            sku_single: self.sku_single,
            conversation_verified: self.conversation_verified,
            source: self.source,
            observed_at_ms: self.observed_at_ms,
            browser_supplemented: self.browser_supplemented,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            sku_parts: FieldStatus::Verified(vec![]),
            sku_single: true,
            conversation_verified: true,
            source: "ws_event".into(),
            observed_at_ms: 1_760_000_001_000,
            browser_supplemented: false,
        }
    }

    #[test]
    fn complete_snapshot_passes() {
        assert!(complete_snapshot().build().completeness().is_ok());
    }

    #[test]
    fn missing_quantity_is_not_default_one() {
        let mut b = complete_snapshot();
        b.quantity = FieldStatus::Missing;
        let err = b.build().completeness().unwrap_err();
        assert!(matches!(err, Ineligibility::QuantityMissing));
    }

    #[test]
    fn unpaid_or_refunded_rejected() {
        for state in [PlatformOrderState::Unpaid, PlatformOrderState::Refunding] {
            let mut b = complete_snapshot();
            b.platform_state = FieldStatus::Verified(state);
            assert!(b.build().completeness().is_err(), "{state:?} 不可交付");
        }
    }

    #[test]
    fn bargain_and_missing_sku_rejected() {
        let mut b = complete_snapshot();
        b.trade_type = FieldStatus::Verified(TradeType::Bargain);
        assert!(matches!(
            b.build().completeness(),
            Err(Ineligibility::TradeTypeUnsupported)
        ));

        let mut b = complete_snapshot();
        b.sku_parts = FieldStatus::Missing;
        b.sku_single = false;
        assert!(matches!(
            b.build().completeness(),
            Err(Ineligibility::SkuIncomplete)
        ));
    }
}
