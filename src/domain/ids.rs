//! 本地标识生成:前缀 + UUID,不透明字符串。
//! 平台 ID 按原字符串保存,不能用数值表达长整型(platform-adapter §2)。

pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, uuid::Uuid::new_v4().simple())
}

/// 幂等键/请求键:调用方为一次用户意图生成的随机键(http-api §1)。
pub fn new_request_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 时间有序 ID:毫秒时间戳十六进制前缀 + UUID(字典序≈时间序)。
/// 007 US4 chat_messages.id 用作"加载更早"游标的次序键(data-model 约定)。
pub fn new_time_ordered_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{}_{:016x}_{}", prefix, ms, uuid::Uuid::new_v4().simple())
}
