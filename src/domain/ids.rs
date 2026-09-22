//! 本地标识生成:前缀 + UUID,不透明字符串。
//! 平台 ID 按原字符串保存,不能用数值表达长整型(platform-adapter §2)。

pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, uuid::Uuid::new_v4().simple())
}

/// 幂等键/请求键:调用方为一次用户意图生成的随机键(http-api §1)。
pub fn new_request_key() -> String {
    uuid::Uuid::new_v4().to_string()
}
