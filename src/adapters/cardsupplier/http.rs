//! HTTP 取卡适配器(T011):reqwest 一次性请求,超时按配置;
/// retry_enabled 开启时对可重试失败(网络/5xx)重试一次。
/// response_path 点号取值为纯函数,单测覆盖;不访问数据库。

use std::time::Duration;

use crate::application::ports::cards::{CardSupplier, CardSupplyError};
use crate::domain::cards::ApiCardConfig;

#[derive(Clone)]
pub struct HttpCardSupplier {
    client: reqwest::Client,
}

impl Default for HttpCardSupplier {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCardSupplier {
    pub fn new() -> Self {
        // 连接层公共上限 60 秒(配置允许的最大值);单请求超时按配置收紧
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_millis(10_000))
                .build()
                .expect("reqwest 客户端构建"),
        }
    }
}

/// response_path 点号取值子集:"data.card" → body["data"]["card"];
/// 命中字符串/数字标量返回文本,其他类型(对象/数组/null)视为未命中。
pub fn extract_path(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;
    for seg in path.split('.').map(str::trim).filter(|s| !s.is_empty()) {
        current = current.get(seg)?;
    }
    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[async_trait::async_trait]
impl CardSupplier for HttpCardSupplier {
    async fn fetch(&self, cfg: &ApiCardConfig, request_key: &str) -> Result<String, CardSupplyError> {
        let violations = crate::domain::cards::check_api_config(cfg);
        if !violations.is_empty() {
            return Err(CardSupplyError::InvalidConfig(violations.join(";")));
        }
        let url: reqwest::Url = cfg
            .url
            .trim()
            .parse()
            .map_err(|e| CardSupplyError::InvalidConfig(format!("URL 解析失败:{e}")))?;
        // 重试开关:可重试失败(网络错误/5xx)时再试一次(不放大延迟,超时预算×2)
        let attempts = if cfg.retry_enabled { 2 } else { 1 };
        let mut last: Option<CardSupplyError> = None;
        for _ in 0..attempts {
            match self.fetch_once(cfg, url.clone(), request_key).await {
                Ok(card) => return Ok(card),
                Err(e) => {
                    let retryable = matches!(
                        &e,
                        CardSupplyError::Network(_) | CardSupplyError::Timeout(_) | CardSupplyError::Status(_)
                    );
                    last = Some(e);
                    if !retryable {
                        break;
                    }
                }
            }
        }
        Err(last.unwrap_or(CardSupplyError::EmptyCard))
    }
}

impl HttpCardSupplier {
    async fn fetch_once(
        &self,
        cfg: &ApiCardConfig,
        url: reqwest::Url,
        request_key: &str,
    ) -> Result<String, CardSupplyError> {
        let method = reqwest::Method::from_bytes(cfg.method.trim().to_ascii_uppercase().as_bytes())
            .map_err(|_| CardSupplyError::InvalidConfig("请求方法非法".into()))?;
        let mut req = self.client.request(method, url).query(&cfg.params);
        for (name, value) in &cfg.headers {
            req = req.header(name, value);
        }
        // 请求标识透传(审计对账;不携带机密语义)
        req = req.header("X-Request-Key", request_key);
        if let Some(body) = &cfg.body {
            let content_type = cfg
                .content_type
                .clone()
                .unwrap_or_else(|| "application/json".into());
            req = req.header(reqwest::header::CONTENT_TYPE, content_type);
            req = req.body(body.clone());
        }
        req = req.timeout(Duration::from_millis(cfg.timeout_ms as u64));
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                CardSupplyError::Timeout(cfg.timeout_ms as u64)
            } else {
                CardSupplyError::Network(e.to_string())
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CardSupplyError::Status(status.as_u16()));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| CardSupplyError::Network(e.to_string()))?;
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| CardSupplyError::Parse(e.to_string()))?;
        let card = extract_path(&value, &cfg.response_path)
            .ok_or_else(|| CardSupplyError::PathMissing(cfg.response_path.clone()))?;
        let trimmed = card.trim().to_string();
        if trimmed.is_empty() {
            return Err(CardSupplyError::EmptyCard);
        }
        Ok(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 点号取值_字符串数字命中() {
        let body = json!({ "data": { "card": "ABC-123", "count": 3, "ok": true } });
        assert_eq!(
            extract_path(&body, "data.card").as_deref(),
            Some("ABC-123")
        );
        assert_eq!(extract_path(&body, "data.count").as_deref(), Some("3"));
        assert_eq!(extract_path(&body, "data.ok").as_deref(), Some("true"));
    }

    #[test]
    fn 点号取值_未命中与类型不符() {
        let body = json!({ "data": { "card": "ABC-123", "nested": { "a": 1 } } });
        assert_eq!(extract_path(&body, "data.missing"), None, "路径不存在");
        assert_eq!(extract_path(&body, ""), None, "空路径不取整包");
        assert_eq!(extract_path(&body, "data"), None, "命中对象不算标量");
        assert_eq!(
            extract_path(&body, "data.nested"),
            None,
            "中间层命中对象不算标量"
        );
        assert_eq!(
            extract_path(&body, "data.nested.a"),
            Some("1".into()),
            "嵌套路径逐级下钻取末级数字"
        );
        assert_eq!(
            extract_path(&body, "data. card "),
            Some("ABC-123".into()),
            "路径段容忍空白"
        );
    }

    #[test]
    fn 配置非法即拒绝不发起请求() {
        let supplier = HttpCardSupplier::new();
        let cfg = ApiCardConfig {
            url: "not a url".into(),
            method: "GET".into(),
            timeout_ms: 5_000,
            response_path: "data.card".into(),
            ..Default::default()
        };
        // 同步校验先行:fetch 之前 check_api_config 必须已拒绝
        assert!(!crate::domain::cards::check_api_config(&cfg).is_empty());
        // fetch 路径同样拦截(无网络环境下也不会外呼)
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let out = rt.block_on(supplier.fetch(&cfg, "rk-1"));
        assert!(matches!(out, Err(CardSupplyError::InvalidConfig(_))));
    }
}
