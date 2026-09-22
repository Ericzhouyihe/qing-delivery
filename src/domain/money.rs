//! 金额:非负整数最小货币单位 + 币种,禁止浮点(platform-adapter §4)。
//! 缺失金额不当零;0 金额也必须由平台事实证明。

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Money {
    pub minor_units: i64,
    pub currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MoneyError {
    #[error("金额不能为负")]
    Negative,
    #[error("币种不能为空")]
    EmptyCurrency,
}

impl Money {
    pub fn new(minor_units: i64, currency: impl Into<String>) -> Result<Self, MoneyError> {
        if minor_units < 0 {
            return Err(MoneyError::Negative);
        }
        let currency = currency.into();
        if currency.is_empty() {
            return Err(MoneyError::EmptyCurrency);
        }
        Ok(Self {
            minor_units,
            currency,
        })
    }
}
