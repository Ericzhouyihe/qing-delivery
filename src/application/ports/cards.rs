//! 卡密供给端口(T011,research D3):API 型卡组外部取卡。
//! 网络调用绝不进入 DbThread 闭包/事务;调用方在事务外取卡,
//! 成功后在事务内落 card_entries(origin='api')审计行并绑定订单。

use crate::domain::cards::ApiCardConfig;

#[derive(Debug, thiserror::Error)]
pub enum CardSupplyError {
    #[error("网络请求失败:{0}")]
    Network(String),
    #[error("请求超时(预算 {0} 毫秒)")]
    Timeout(u64),
    #[error("响应状态非成功:HTTP {0}")]
    Status(u16),
    #[error("响应体不是合法 JSON:{0}")]
    Parse(String),
    #[error("取值路径未命中:{0}")]
    PathMissing(String),
    #[error("取到的卡密为空")]
    EmptyCard,
    #[error("配置非法:{0}")]
    InvalidConfig(String),
}

/// 一次性取卡:request_key 由调用方生成,用于审计留痕(不落明文)。
#[async_trait::async_trait]
pub trait CardSupplier: Send + Sync {
    async fn fetch(&self, cfg: &ApiCardConfig, request_key: &str) -> Result<String, CardSupplyError>;
}
