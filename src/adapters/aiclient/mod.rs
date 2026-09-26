//! 007 US7 AI 客户端适配器(T075):OpenAI 兼容 HTTP 客户端。
//! - `chat`:`POST {base}/chat/completions`(base 已以 /chat/completions 结尾
//!   则直接用,无需补全),取 choices[0].message.content;30s 超时;
//! - `models`:`GET {base 去掉 /chat/completions}/models`,取 data[].id;
//! - `test`:最小对话"你好"(回复截 60 字),返回 {model, latency_ms, reply}。
//! 错误结构化(网络/状态码/解析);Display 脱敏——API Key 只进
//! Authorization 头,绝不进入 URL、错误信息或日志(宪章红线)。
//! URL 拼接与响应解析均为纯函数(单测不发网)。

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// AI 上游单次调用超时(research D7:30s)。
pub const AI_TIMEOUT: Duration = Duration::from_secs(30);
/// 测试连接回复摘要截断长度。
pub const TEST_REPLY_EXCERPT: usize = 60;
/// 上游错误响应体摘录上限(防大响应体泄漏进错误信封)。
const ERROR_EXCERPT: usize = 200;

/// 一次 AI 调用的完整配置(系统设置装配;key 仅在内存)。
#[derive(Clone, Debug)]
pub struct AiConfig {
    /// 服务地址(可带或不带 /chat/completions 后缀)
    pub api_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
        }
    }
}

/// 测试连接报告:{model, latency_ms, reply}。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestReport {
    pub model: String,
    pub latency_ms: u64,
    pub reply: String,
}

/// 结构化错误;Display 只含脱敏信息(无 API Key)。
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI 服务地址无效(需 http(s) URL)")]
    InvalidUrl,
    #[error("AI 服务网络错误:{0}")]
    Network(String),
    #[error("AI 服务返回 HTTP {0}:{1}")]
    Status(u16, String),
    #[error("AI 响应解析失败:{0}")]
    Parse(String),
}

// ---------- 纯函数:URL 拼接(单测覆盖) ----------

fn normalize_base(base: &str) -> Option<&str> {
    let trimmed = base.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return None;
    }
    Some(trimmed.trim_end_matches('/'))
}

/// chat 端点:base 以 /chat/completions 结尾直接用,否则拼接。
pub fn chat_endpoint(base: &str) -> Result<String, AiError> {
    let normalized = normalize_base(base).ok_or(AiError::InvalidUrl)?;
    if normalized.ends_with("/chat/completions") {
        Ok(normalized.to_string())
    } else {
        Ok(format!("{normalized}/chat/completions"))
    }
}

/// models 端点:base 去掉 /chat/completions 后缀再拼 /models。
pub fn models_endpoint(base: &str) -> Result<String, AiError> {
    let normalized = normalize_base(base).ok_or(AiError::InvalidUrl)?;
    let stripped = normalized
        .strip_suffix("/chat/completions")
        .unwrap_or(normalized);
    Ok(format!("{stripped}/models"))
}

// ---------- 纯函数:响应解析(单测覆盖) ----------

fn excerpt(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 解析 chat/completions 响应:choices[0].message.content(字符串或缺失 → Parse)。
pub fn parse_chat_content(body: &str) -> Result<String, AiError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AiError::Parse(format!("非法 JSON:{e}")))?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| AiError::Parse("缺少 choices[0].message.content".into()))?;
    Ok(content.to_string())
}

/// 解析 models 响应:data[].id。
pub fn parse_model_ids(body: &str) -> Result<Vec<String>, AiError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AiError::Parse(format!("非法 JSON:{e}")))?;
    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| AiError::Parse("缺少 data 数组".into()))?;
    let ids = data
        .iter()
        .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    Ok(ids)
}

// ---------- HTTP 客户端 ----------

pub struct AiHttpClient {
    client: reqwest::Client,
}

impl Default for AiHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AiHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(AI_TIMEOUT)
                .build()
                .expect("reqwest 客户端构造不会失败"),
        }
    }

    /// 一次对话补全;返回 assistant 文本。
    pub async fn chat(&self, cfg: &AiConfig, messages: &[ChatMessage]) -> Result<String, AiError> {
        let url = chat_endpoint(&cfg.api_url)?;
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&serde_json::json!({
                "model": cfg.model,
                "messages": messages,
            }))
            .send()
            .await
            .map_err(|e| AiError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AiError::Status(
                status.as_u16(),
                excerpt(body.trim(), ERROR_EXCERPT),
            ));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| AiError::Network(e.to_string()))?;
        parse_chat_content(&body)
    }

    /// 模型列表(与 chat 同一凭据;不含 model 字段)。
    pub async fn models(&self, cfg: &AiConfig) -> Result<Vec<String>, AiError> {
        let url = models_endpoint(&cfg.api_url)?;
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&cfg.api_key)
            .send()
            .await
            .map_err(|e| AiError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AiError::Status(
                status.as_u16(),
                excerpt(body.trim(), ERROR_EXCERPT),
            ));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| AiError::Network(e.to_string()))?;
        parse_model_ids(&body)
    }

    /// 测试连接:最小对话"你好",返回 {model, latency_ms, reply(截 60 字)}。
    pub async fn test(&self, cfg: &AiConfig) -> Result<TestReport, AiError> {
        let started = std::time::Instant::now();
        let reply = self
            .chat(
                cfg,
                &[ChatMessage::new("user", "你好")],
            )
            .await?;
        Ok(TestReport {
            model: cfg.model.clone(),
            latency_ms: started.elapsed().as_millis() as u64,
            reply: excerpt(reply.trim(), TEST_REPLY_EXCERPT),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- URL 拼接规则 ----------

    #[test]
    fn chat_endpoint_appends_suffix_when_missing() {
        assert_eq!(
            chat_endpoint("https://ai.example.com/v1").unwrap(),
            "https://ai.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint("https://ai.example.com").unwrap(),
            "https://ai.example.com/chat/completions"
        );
    }

    #[test]
    fn chat_endpoint_uses_as_is_when_suffixed() {
        // 契约:base 以 /chat/completions 结尾则直接用,无需补全
        assert_eq!(
            chat_endpoint("https://ai.example.com/v1/chat/completions").unwrap(),
            "https://ai.example.com/v1/chat/completions"
        );
        // 尾部斜杠容忍
        assert_eq!(
            chat_endpoint("https://ai.example.com/v1/").unwrap(),
            "https://ai.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoints_reject_non_http_bases() {
        for bad in ["ftp://x", "example.com/v1", "", "  "] {
            assert!(matches!(chat_endpoint(bad), Err(AiError::InvalidUrl)));
            assert!(matches!(models_endpoint(bad), Err(AiError::InvalidUrl)));
        }
    }

    #[test]
    fn models_endpoint_strips_chat_suffix() {
        assert_eq!(
            models_endpoint("https://ai.example.com/v1/chat/completions").unwrap(),
            "https://ai.example.com/v1/models"
        );
        assert_eq!(
            models_endpoint("https://ai.example.com/v1").unwrap(),
            "https://ai.example.com/v1/models"
        );
        assert_eq!(
            models_endpoint("https://ai.example.com/").unwrap(),
            "https://ai.example.com/models"
        );
    }

    // ---------- 响应解析 ----------

    #[test]
    fn parse_chat_content_normal() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好,很高兴为你服务"}}]}"#;
        assert_eq!(parse_chat_content(body).unwrap(), "你好,很高兴为你服务");
    }

    #[test]
    fn parse_chat_content_missing_or_malformed() {
        // choices 空
        assert!(matches!(
            parse_chat_content(r#"{"choices":[]}"#),
            Err(AiError::Parse(_))
        ));
        // 缺 message
        assert!(matches!(
            parse_chat_content(r#"{"choices":[{"delta":{}}]}"#),
            Err(AiError::Parse(_))
        ));
        // content 非字符串
        assert!(matches!(
            parse_chat_content(r#"{"choices":[{"message":{"content":123}}]}"#),
            Err(AiError::Parse(_))
        ));
        // 畸形 JSON
        assert!(matches!(parse_chat_content("{not json"), Err(AiError::Parse(_))));
        // 顶层缺 choices
        assert!(matches!(parse_chat_content("{}"), Err(AiError::Parse(_))));
    }

    #[test]
    fn parse_model_ids_normal_and_edge() {
        let body = r#"{"data":[{"id":"gpt-a"},{"id":"gpt-b"},{"object":"model"}]}"#;
        assert_eq!(parse_model_ids(body).unwrap(), vec!["gpt-a", "gpt-b"]);
        assert_eq!(parse_model_ids(r#"{"data":[]}"#).unwrap(), Vec::<String>::new());
        assert!(matches!(parse_model_ids(r#"{"object":"list"}"#), Err(AiError::Parse(_))));
        assert!(matches!(parse_model_ids("nope"), Err(AiError::Parse(_))));
    }

    #[test]
    fn error_display_carries_no_key_material() {
        let cases = [
            AiError::InvalidUrl,
            AiError::Network("connection refused".into()),
            AiError::Status(502, "bad gateway".into()),
            AiError::Parse("缺少字段".into()),
        ];
        let key = "sk-secret-key-value";
        for e in &cases {
            let text = e.to_string();
            assert!(!text.contains(key));
        }
    }
}
