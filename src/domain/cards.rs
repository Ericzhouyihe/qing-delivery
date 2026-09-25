//! 卡密域(T005/T008):条目状态机、池类型与 API 取卡配置校验。
//! 纯业务决策:状态机不执行副作用,校验保存与执行前同源(与 rules 同风格)。
//! 卡密明文只存在于:创建/追加入参、发送前解密与加密快照;本模块不接触明文存储。

pub const MIN_DELAY_SECONDS: i64 = 0;
pub const MAX_DELAY_SECONDS: i64 = 3600;
/// API 取卡超时:1—60 秒(data-model api_config 约定)
pub const MIN_API_TIMEOUT_MS: i64 = 1_000;
pub const MAX_API_TIMEOUT_MS: i64 = 60_000;

/// 卡密组类型(四枚举,data-model CHECK 集合)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardPoolKind {
    /// 批量库存:多条卡密条目,原子预留
    Data,
    /// 固定文本:单条固定内容信封
    Text,
    /// 固定图片:单条固定内容信封
    Image,
    /// 接口取卡:外部 API 按需取卡
    Api,
}

impl CardPoolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CardPoolKind::Data => "data",
            CardPoolKind::Text => "text",
            CardPoolKind::Image => "image",
            CardPoolKind::Api => "api",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "data" => Some(CardPoolKind::Data),
            "text" => Some(CardPoolKind::Text),
            "image" => Some(CardPoolKind::Image),
            "api" => Some(CardPoolKind::Api),
            _ => None,
        }
    }

    /// 批量导入"类型"列取值:英文枚举名 + 常用中文别名。
    /// api 不在导入范围(整份配置无法用一行表达)。
    pub fn parse_label(s: &str) -> Option<Self> {
        match s.trim() {
            "data" | "批量" | "批量库存" => Some(CardPoolKind::Data),
            "text" | "文本" | "固定文本" => Some(CardPoolKind::Text),
            "image" | "图片" | "固定图片" => Some(CardPoolKind::Image),
            "api" | "接口" => Some(CardPoolKind::Api),
            _ => None,
        }
    }
}

/// 卡密条目状态(data-model CHECK 集合)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardEntryState {
    Available,
    Reserved,
    Used,
    Disabled,
    PendingApi,
}

impl CardEntryState {
    pub fn as_str(self) -> &'static str {
        match self {
            CardEntryState::Available => "available",
            CardEntryState::Reserved => "reserved",
            CardEntryState::Used => "used",
            CardEntryState::Disabled => "disabled",
            CardEntryState::PendingApi => "pending_api",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "available" => Some(CardEntryState::Available),
            "reserved" => Some(CardEntryState::Reserved),
            "used" => Some(CardEntryState::Used),
            "disabled" => Some(CardEntryState::Disabled),
            "pending_api" => Some(CardEntryState::PendingApi),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("非法卡密条目状态迁移:{from} → {to}(available→reserved→used;释放仅 reserved→available;used 永不复用)")]
pub struct IllegalCardTransition {
    pub from: &'static str,
    pub to: &'static str,
}

/// 预留:仅 available 条目可被绑定订单(领域层校验;SQL 状态谓词双保险)。
pub fn reserve_transition(from: CardEntryState) -> Result<CardEntryState, IllegalCardTransition> {
    match from {
        CardEntryState::Available => Ok(CardEntryState::Reserved),
        other => Err(IllegalCardTransition {
            from: other.as_str(),
            to: "reserved",
        }),
    }
}

/// 释放:仅 reserved 可回到 available(确定未发送才允许;used 永不复用)。
pub fn release_transition(from: CardEntryState) -> Result<CardEntryState, IllegalCardTransition> {
    match from {
        CardEntryState::Reserved => Ok(CardEntryState::Available),
        other => Err(IllegalCardTransition {
            from: other.as_str(),
            to: "available",
        }),
    }
}

/// 扣减:仅 reserved → used(确定成功;used 永不复用)。
pub fn consume_transition(from: CardEntryState) -> Result<CardEntryState, IllegalCardTransition> {
    match from {
        CardEntryState::Reserved => Ok(CardEntryState::Used),
        other => Err(IllegalCardTransition {
            from: other.as_str(),
            to: "used",
        }),
    }
}

/// 延迟秒校验:0—3600(data-model CHECK)。
pub fn check_delay_seconds(v: i64) -> Result<(), String> {
    if (MIN_DELAY_SECONDS..=MAX_DELAY_SECONDS).contains(&v) {
        Ok(())
    } else {
        Err(format!(
            "延迟秒必须在 {MIN_DELAY_SECONDS}—{MAX_DELAY_SECONDS} 之间,当前 {v}"
        ))
    }
}

/// API 取卡配置(kind=api 的整份 JSON 信封明文结构,data-model)。
/// headers/params/body 可能携带令牌等机密:回显只经 ApiConfigSummary,永不整包返回。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ApiCardConfig {
    pub url: String,
    /// GET / POST
    pub method: String,
    /// 1_000—60_000 毫秒
    pub timeout_ms: i64,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub params: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    /// 点号取值路径(如 data.card),非空
    pub response_path: String,
    #[serde(default)]
    pub retry_enabled: bool,
}

/// API 配置必填字段校验(url 合法 http(s)、method ∈ GET/POST、
/// 超时 1—60 秒、response_path 非空);返回违例清单(与 check_content 同风格)。
pub fn check_api_config(cfg: &ApiCardConfig) -> Vec<String> {
    let mut violations = Vec::new();
    let url = cfg.url.trim();
    if url.is_empty() {
        violations.push("API 地址为空".to_string());
    } else {
        match url::Url::parse(url) {
            Ok(parsed) => {
                let ok_scheme = matches!(parsed.scheme(), "http" | "https");
                let has_host = parsed.host_str().map(|h| !h.is_empty()).unwrap_or(false);
                if !ok_scheme || !has_host {
                    violations.push("API 地址必须是带主机的 http(s) URL".to_string());
                }
            }
            Err(_) => violations.push("API 地址格式非法".to_string()),
        }
    }
    match cfg.method.trim().to_ascii_uppercase().as_str() {
        "GET" | "POST" => {}
        other => violations.push(format!("请求方法只支持 GET/POST,当前 {other}")),
    }
    if !(MIN_API_TIMEOUT_MS..=MAX_API_TIMEOUT_MS).contains(&cfg.timeout_ms) {
        violations.push(format!(
            "超时必须在 {}—{} 毫秒(1—60 秒)之间,当前 {}",
            MIN_API_TIMEOUT_MS, MAX_API_TIMEOUT_MS, cfg.timeout_ms
        ));
    }
    if cfg.response_path.trim().is_empty() {
        violations.push("取值路径(response_path)不能为空".to_string());
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 状态机_预留仅available() {
        assert_eq!(
            reserve_transition(CardEntryState::Available),
            Ok(CardEntryState::Reserved)
        );
        for from in [CardEntryState::Reserved, CardEntryState::Used, CardEntryState::Disabled] {
            assert!(reserve_transition(from).is_err(), "{from:?} 不可再预留");
        }
    }

    #[test]
    fn 状态机_释放仅reserved回available() {
        assert_eq!(
            release_transition(CardEntryState::Reserved),
            Ok(CardEntryState::Available)
        );
        // used 永不复用:used 不能释放回库存
        assert!(release_transition(CardEntryState::Used).is_err());
        assert!(release_transition(CardEntryState::Available).is_err());
    }

    #[test]
    fn 状态机_扣减仅reserved到used且used不可再迁移() {
        assert_eq!(
            consume_transition(CardEntryState::Reserved),
            Ok(CardEntryState::Used)
        );
        assert!(consume_transition(CardEntryState::Used).is_err(), "used 永不复用");
        assert!(consume_transition(CardEntryState::Available).is_err());
        // 字符串往返:全部状态可解析且名一致
        for s in [
            CardEntryState::Available,
            CardEntryState::Reserved,
            CardEntryState::Used,
            CardEntryState::Disabled,
            CardEntryState::PendingApi,
        ] {
            assert_eq!(CardEntryState::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn 延迟秒边界_0与3600合法_越界拒绝() {
        assert!(check_delay_seconds(0).is_ok());
        assert!(check_delay_seconds(3600).is_ok());
        assert!(check_delay_seconds(-1).is_err());
        assert!(check_delay_seconds(3601).is_err());
    }

    #[test]
    fn 类型四枚举_解析与别名() {
        for k in [
            CardPoolKind::Data,
            CardPoolKind::Text,
            CardPoolKind::Image,
            CardPoolKind::Api,
        ] {
            assert_eq!(CardPoolKind::parse(k.as_str()), Some(k));
            assert!(CardPoolKind::parse_label(k.as_str()).is_some());
        }
        assert!(CardPoolKind::parse("voice").is_none());
        assert_eq!(CardPoolKind::parse_label("批量库存"), Some(CardPoolKind::Data));
        assert_eq!(CardPoolKind::parse_label("文本"), Some(CardPoolKind::Text));
        assert_eq!(CardPoolKind::parse_label("图片"), Some(CardPoolKind::Image));
    }

    fn valid_config() -> ApiCardConfig {
        ApiCardConfig {
            url: "https://api.example.com/card".into(),
            method: "GET".into(),
            timeout_ms: 5_000,
            headers: vec![("Authorization".into(), "Bearer x".into())],
            params: vec![("sku".into(), "A".into())],
            body: None,
            content_type: None,
            response_path: "data.card".into(),
            retry_enabled: true,
        }
    }

    #[test]
    fn api配置_合法输入零违例() {
        assert!(check_api_config(&valid_config()).is_empty());
        let post = ApiCardConfig {
            method: "POST".into(),
            body: Some("{}".into()),
            content_type: Some("application/json".into()),
            ..valid_config()
        };
        assert!(check_api_config(&post).is_empty());
    }

    #[test]
    fn api配置_必填字段逐项拒绝() {
        // url:空 / 非 http(s) / 无法解析
        for url in ["", "ftp://example.com/x", "not a url"] {
            let cfg = ApiCardConfig {
                url: url.into(),
                ..valid_config()
            };
            assert!(!check_api_config(&cfg).is_empty(), "url={url} 应拒绝");
        }
        // method
        let cfg = ApiCardConfig {
            method: "PUT".into(),
            ..valid_config()
        };
        assert!(!check_api_config(&cfg).is_empty());
        // timeout:低于 1 秒 / 超过 60 秒
        for timeout_ms in [999, 60_001] {
            let cfg = ApiCardConfig {
                timeout_ms,
                ..valid_config()
            };
            assert!(!check_api_config(&cfg).is_empty(), "timeout={timeout_ms} 应拒绝");
        }
        assert!(check_delay_seconds(0).is_ok());
        // response_path 为空
        let cfg = ApiCardConfig {
            response_path: "  ".into(),
            ..valid_config()
        };
        assert!(!check_api_config(&cfg).is_empty());
    }
}
