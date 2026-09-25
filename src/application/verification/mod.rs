//! 安全验证自动化(003):统一信号、处置结果与对象安全驱动端口(研究 D2/D3)。

pub mod gate;
pub mod service;

use serde_json::Value;

/// 触发信号来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalSource {
    Mtop,
    Ws,
    Qr,
    Manual,
}

impl SignalSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SignalSource::Mtop => "mtop",
            SignalSource::Ws => "ws",
            SignalSource::Qr => "qr",
            SignalSource::Manual => "manual",
        }
    }
}

/// 统一触发事件:仅明确信号构造(关键词集合收窄,SC-304 误报为零)。
#[derive(Clone, Debug)]
pub struct VerificationSignal {
    pub account_id: String,
    pub source: SignalSource,
    pub url: Option<String>,
    pub raw: String,
}

/// 处置结果:Driver 不决定重试(服务层职责,宪章 II)。
#[derive(Clone, Debug)]
pub enum SolveOutcome {
    Solved { cookie_jar: String },
    Failed { reason: String, stage: &'static str },
}

/// 对象安全驱动端口(沿 QrDriver/ItemSyncDriver 模式)。
pub trait VerificationDriver: Send + Sync {
    fn solve_boxed(
        &self,
        url: String,
        cookie_seed: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SolveOutcome> + Send>>;
}

/// 从响应 JSON 递归提取验证 URL(gotoUrl/url 类字段;实账号校准项 R11)。
pub fn extract_verification_url(v: &Value) -> Option<String> {
    fn walk(v: &Value, depth: u8) -> Option<String> {
        if depth > 6 {
            return None;
        }
        match v {
            Value::Object(m) => {
                for key in ["gotoUrl", "verifyUrl", "url"] {
                    if let Some(Value::String(s)) = m.get(key)
                        && s.starts_with("http")
                    {
                        return Some(s.clone());
                    }
                }
                for child in m.values() {
                    if let Some(found) = walk(child, depth + 1) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(a) => a.iter().find_map(|c| walk(c, depth + 1)),
            _ => None,
        }
    }
    walk(v, 0)
}

/// WS/文本信号关键词:收窄到 punish 系,普通消息不触发(误报为零)。
pub fn is_verification_text(text: &str) -> bool {
    text.contains("punish")
        || text.contains("rgv587")
        || text.contains("x5secdata")
        || text.contains("FAIL_SYS_USER_VALIDATE")
        || text.contains("滑块")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_提取_常见字段与深度限制() {
        assert_eq!(
            extract_verification_url(&json!({"data": {"gotoUrl": "https://v.example/x"}})),
            Some("https://v.example/x".into())
        );
        assert_eq!(
            extract_verification_url(&json!({"url": "http://a"})),
            Some("http://a".into())
        );
        assert_eq!(extract_verification_url(&json!({"url": "not-http"})), None);
    }

    #[test]
    fn 文本信号_关键词收窄() {
        assert!(is_verification_text("punish execute"));
        assert!(is_verification_text("请完成滑块验证"));
        assert!(!is_verification_text("买家:在吗?"));
        assert!(!is_verification_text("{\"type\":\"heartbeat\"}"));
    }
}

#[cfg(test)]
mod service_tests;
