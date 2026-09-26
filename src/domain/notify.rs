//! 通知渠道域(T056/T059,007 US5):事件订阅(空=全部)、七类渠道枚举、
//! 钉钉加签与飞书签名纯函数(HMAC-SHA256,基于 sha2 手写 HMAC——
//! research D11 未批准 hmac crate,自实现并以内建已知答案验证)、
//! 渠道配置与秘密键集合校验、发送侧值类型(NotifyEventData/ChannelConfig)。
//! 纯业务决策无副作用(与 chat 域同风格);网络发送在 adapters::notify。

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// 事件类型合法值(contracts §5;空订阅=全部)
pub const EVENT_KINDS: [&str; 6] = [
    "account_offline",
    "account_recovered",
    "security_verification",
    "manual_intervention_required",
    "delivery_result",
    "system_error",
];

/// 渠道类型合法值(data-model CHECK 集合,FR-050 七类)
pub const CHANNEL_KINDS: [&str; 7] = [
    "webhook", "email", "dingtalk", "feishu", "wecom", "bark", "telegram",
];

/// 通知事件类型(research D8 枚举的域侧投影)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    AccountOffline,
    AccountRecovered,
    SecurityVerification,
    ManualInterventionRequired,
    DeliveryResult,
    SystemError,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::AccountOffline => "account_offline",
            EventKind::AccountRecovered => "account_recovered",
            EventKind::SecurityVerification => "security_verification",
            EventKind::ManualInterventionRequired => "manual_intervention_required",
            EventKind::DeliveryResult => "delivery_result",
            EventKind::SystemError => "system_error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "account_offline" => Some(EventKind::AccountOffline),
            "account_recovered" => Some(EventKind::AccountRecovered),
            "security_verification" => Some(EventKind::SecurityVerification),
            "manual_intervention_required" => Some(EventKind::ManualInterventionRequired),
            "delivery_result" => Some(EventKind::DeliveryResult),
            "system_error" => Some(EventKind::SystemError),
            _ => None,
        }
    }
}

/// 渠道类型(FR-050 七类)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChannelKind {
    Webhook,
    Email,
    Dingtalk,
    Feishu,
    Wecom,
    Bark,
    Telegram,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Webhook => "webhook",
            ChannelKind::Email => "email",
            ChannelKind::Dingtalk => "dingtalk",
            ChannelKind::Feishu => "feishu",
            ChannelKind::Wecom => "wecom",
            ChannelKind::Bark => "bark",
            ChannelKind::Telegram => "telegram",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "webhook" => Some(ChannelKind::Webhook),
            "email" => Some(ChannelKind::Email),
            "dingtalk" => Some(ChannelKind::Dingtalk),
            "feishu" => Some(ChannelKind::Feishu),
            "wecom" => Some(ChannelKind::Wecom),
            "bark" => Some(ChannelKind::Bark),
            "telegram" => Some(ChannelKind::Telegram),
            _ => None,
        }
    }
}

/// 订阅匹配:event_types 为空 = 订阅全部;否则精确命中该事件类型(FR-051)。
pub fn subscribes(subscriptions: &[EventKind], event: EventKind) -> bool {
    subscriptions.is_empty() || subscriptions.contains(&event)
}

/// 请求侧严格解析:非法值整体拒绝(422 invalid_request 语义)。
pub fn parse_event_types_strict(values: &[String]) -> Result<Vec<EventKind>, String> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        let kind = EventKind::parse(v)
            .ok_or_else(|| format!("非法事件类型:{v}(合法值:{})", EVENT_KINDS.join(", ")))?;
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    Ok(out)
}

/// 存储侧宽松解析:忽略无法识别的项(存储值始终来自严格解析,此处防御)。
pub fn parse_event_types_lenient(json: &str) -> Vec<EventKind> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| EventKind::parse(s))
        .collect()
}

/// 把解析后的事件列表序列化为存储 JSON(空=全部)。
pub fn event_types_json(subscriptions: &[EventKind]) -> String {
    serde_json::to_string(
        &subscriptions
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into())
}

// ---------- 签名算法(纯函数;时间戳由调用方传入) ----------

/// HMAC-SHA256(research D11 未批准 hmac 依赖;标准 ipad/opad 构造,
/// 钉钉/飞书已知答案测试验证正确性)。
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64; // SHA-256 块大小
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let out = outer.finalize();
    let mut mac = [0u8; 32];
    mac.copy_from_slice(&out);
    mac
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 钉钉加签:HMAC-SHA256(密钥=secret,消息="{timestamp_ms}\n{secret}")
/// → base64 → application/x-www-form-urlencoded 编码(+→%2B、/→%2F、=→%3D),
/// 返回可直接拼接的 sign 值(官方开放平台算法)。
pub fn dingtalk_sign(timestamp_ms: i64, secret: &str) -> String {
    let string_to_sign = format!("{timestamp_ms}\n{secret}");
    let mac = hmac_sha256(secret.as_bytes(), string_to_sign.as_bytes());
    url_encode_component(&base64_encode(&mac))
}

/// 钉钉加签 webhook 拼接:已有查询串用 & 追加,否则以 ? 起头。
pub fn dingtalk_signed_url(webhook: &str, timestamp_ms: i64, secret: &str) -> String {
    let sep = if webhook.contains('?') { '&' } else { '?' };
    format!(
        "{webhook}{sep}timestamp={timestamp_ms}&sign={}",
        dingtalk_sign(timestamp_ms, secret)
    )
}

/// 飞书签名:HMAC-SHA256(密钥="{timestamp_sec}\n{secret}",消息为空)→ base64。
/// 注意与钉钉相反:飞书以 string_to_sign 作为 HMAC 密钥(官方算法)。
pub fn feishu_sign(timestamp_sec: i64, secret: &str) -> String {
    let string_to_sign = format!("{timestamp_sec}\n{secret}");
    let mac = hmac_sha256(string_to_sign.as_bytes(), b"");
    base64_encode(&mac)
}

/// application/x-www-form-urlencoded 的值编码(与 JS encodeURIComponent 在
/// base64 字符集上等价:+→%2B、/→%2F、=→%3D)。
fn url_encode_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

// ---------- 渠道配置与秘密键校验(FR-050;config 为明文非机密 JSON) ----------

fn cfg_str<'a>(config: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    config.get(key).and_then(|v| v.as_str()).map(str::trim)
}

fn check_http_url(value: &str, field: &str) -> Result<(), String> {
    let parsed: url::Url = value
        .parse()
        .map_err(|_| format!("配置字段 {field} 不是合法 URL:{value}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("配置字段 {field} 仅支持 http/https"));
    }
    Ok(())
}

/// 渠道明文配置校验:各 kind 的必填字段与取值约束。
pub fn check_channel_config(kind: ChannelKind, config: &serde_json::Value) -> Result<(), String> {
    if !config.is_object() {
        return Err("config 必须是对象".into());
    }
    match kind {
        ChannelKind::Webhook
        | ChannelKind::Dingtalk
        | ChannelKind::Feishu
        | ChannelKind::Wecom => {
            let url = cfg_str(config, "url").filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{} 渠道必填配置字段 url", kind.as_str()))?;
            check_http_url(url, "url")
        }
        ChannelKind::Bark => {
            let server = cfg_str(config, "server_url").filter(|s| !s.is_empty())
                .ok_or_else(|| "bark 渠道必填配置字段 server_url".to_string())?;
            check_http_url(server, "server_url")
        }
        ChannelKind::Telegram => {
            let chat = cfg_str(config, "chat_id").filter(|s| !s.is_empty())
                .ok_or_else(|| "telegram 渠道必填配置字段 chat_id".to_string())?;
            if chat.parse::<i64>().is_err() && !chat.starts_with('@') && !chat.starts_with('-') {
                return Err("telegram chat_id 应为数字、@频道名或 -100 开头群组".into());
            }
            Ok(())
        }
        ChannelKind::Email => {
            let to = cfg_str(config, "to_address").filter(|s| !s.is_empty())
                .ok_or_else(|| "email 渠道必填配置字段 to_address".to_string())?;
            if !to.contains('@') {
                return Err("email to_address 不是合法邮箱地址".into());
            }
            let use_system = config
                .get("use_system_smtp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if use_system {
                return Ok(()); // 复用系统 SMTP(host 等在系统设置侧校验)
            }
            let host = cfg_str(config, "smtp_host").filter(|s| !s.is_empty())
                .ok_or_else(|| "email 渠道未启用系统 SMTP 时必填 smtp_host".to_string())?;
            if host.is_empty() {
                return Err("smtp_host 不能为空".into());
            }
            let from = cfg_str(config, "from_address").filter(|s| !s.is_empty())
                .ok_or_else(|| "email 渠道未启用系统 SMTP 时必填 from_address".to_string())?;
            if !from.contains('@') {
                return Err("email from_address 不是合法邮箱地址".into());
            }
            if let Some(port) = config.get("smtp_port")
                && port.as_u64().is_none()
                && port.as_str().and_then(|s| s.parse::<u16>().ok()).is_none()
            {
                return Err("smtp_port 应为 1—65535 端口号".into());
            }
            Ok(())
        }
    }
}

/// 各渠道允许的秘密键集合(创建/编辑时的键名白名单;整包信封的明文键)。
pub fn allowed_secret_keys(kind: ChannelKind) -> &'static [&'static str] {
    match kind {
        ChannelKind::Webhook | ChannelKind::Wecom => &[],
        ChannelKind::Dingtalk | ChannelKind::Feishu => &["secret"],
        ChannelKind::Bark => &["device_key"],
        ChannelKind::Telegram => &["bot_token"],
        ChannelKind::Email => &["smtp_user", "smtp_password"],
    }
}

/// 各渠道创建时的必填秘密键(bark 设备号/telegram 机器人 token;
/// 钉钉·飞书加签为可选;email 独立 SMTP 凭据可选——匿名中继允许为空)。
pub fn required_secret_keys(kind: ChannelKind) -> &'static [&'static str] {
    match kind {
        ChannelKind::Bark => &["device_key"],
        ChannelKind::Telegram => &["bot_token"],
        _ => &[],
    }
}

/// 秘密键校验:键名必须在白名单内;require_required 时必填键非空。
pub fn check_secrets(
    kind: ChannelKind,
    provided: &BTreeMap<String, String>,
    require_required: bool,
) -> Result<(), String> {
    let allowed = allowed_secret_keys(kind);
    for key in provided.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{} 渠道不接受秘密字段 {key}(允许:{})",
                kind.as_str(),
                if allowed.is_empty() { "无".to_string() } else { allowed.join(", ") }
            ));
        }
    }
    if require_required {
        for key in required_secret_keys(kind) {
            let value = provided.get(*key).map(|v| v.trim()).unwrap_or("");
            if value.is_empty() {
                return Err(format!("{} 渠道必填秘密字段 {key}", kind.as_str()));
            }
        }
    }
    Ok(())
}

// ---------- 发送侧值类型(adapters::notify 消费) ----------

/// 一次通知的载荷数据(title/content 由应用层事件构造;测试投递 event="test")。
#[derive(Clone, Debug, PartialEq)]
pub struct NotifyEventData {
    pub event: String,
    pub title: String,
    pub content: String,
    pub timestamp: String,
}

/// 系统 SMTP 参数(system_settings/system_secrets 解析后的内存视图;
/// email 渠道 use_system_smtp=true 时由服务层注入 ChannelConfig)。
#[derive(Clone, Debug, PartialEq)]
pub struct SystemSmtp {
    pub host: String,
    pub port: u16,
    pub encryption: String, // none | starttls | ssl
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub from_name: Option<String>,
}

/// 发送时点的渠道视图:明文配置 + 解密后的秘密字典(秘密明文仅存在于
/// 内存与该结构,绝不落库、绝不回显 API)。
#[derive(Clone, Debug)]
pub struct ChannelConfig {
    pub id: String,
    pub kind: ChannelKind,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub secrets: BTreeMap<String, String>,
    pub event_types: Vec<EventKind>,
    pub system_smtp: Option<SystemSmtp>,
}

impl ChannelConfig {
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(|s| s.as_str()).filter(|s| !s.is_empty())
    }

    pub fn cfg_str(&self, key: &str) -> Option<&str> {
        cfg_str(&self.config, key).filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- T056:签名算法已知答案(钉钉/飞书官方算法,node 独立实现对拍) ----------

    #[test]
    fn 签名_钉钉加签_已知答案() {
        // key=secret, msg="{ts}\n{secret}" → HMAC-SHA256 → base64 → URL 编码
        assert_eq!(
            dingtalk_sign(1_600_000_000_000, "secret123"),
            "DvrLEPjUDitKWZKJtUGilH9lqZjvvhI60t%2FGkdrlpGA%3D"
        );
        // 稳定性:同输入同输出;不含填充字符的 base64 不引入 %3D
        let again = dingtalk_sign(1_600_000_000_000, "secret123");
        assert_eq!(again, "DvrLEPjUDitKWZKJtUGilH9lqZjvvhI60t%2FGkdrlpGA%3D");
    }

    #[test]
    fn 签名_飞书_已知答案() {
        // key="{ts}\n{secret}", msg="" → HMAC-SHA256 → base64(不 URL 编码)
        assert_eq!(
            feishu_sign(1_600_000_000, "secret123"),
            "BdVlgDTCZ+awBHKqIx5uB+4K3io9ypcHaxVAUreNPUE="
        );
    }

    #[test]
    fn 签名_hmac_sha256_rfc已知向量() {
        // RFC 4231 测试向量 #1(验证手写 HMAC 构造正确)
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex::encode(mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // 向量 #2:key="Jefe", msg="what do ya want for nothing?"
        let mac2 = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac2),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn 签名_钉钉url拼接_有无关查询串均可() {
        let url = dingtalk_signed_url(
            "https://oapi.dingtalk.com/robot/send?access_token=abc",
            1_600_000_000_000,
            "secret123",
        );
        assert!(url.starts_with(
            "https://oapi.dingtalk.com/robot/send?access_token=abc&timestamp=1600000000000&sign="
        ));
        let bare = dingtalk_signed_url(
            "https://oapi.dingtalk.com/robot/send",
            1_600_000_000_000,
            "secret123",
        );
        assert!(bare.starts_with(
            "https://oapi.dingtalk.com/robot/send?timestamp=1600000000000&sign="
        ));
    }

    // ---------- T056:event_types 订阅匹配 ----------

    #[test]
    fn 订阅_空为全部_命中与未命中() {
        let none: Vec<EventKind> = vec![];
        // 空 = 订阅全部事件
        for kind in [
            EventKind::AccountOffline,
            EventKind::AccountRecovered,
            EventKind::SecurityVerification,
            EventKind::ManualInterventionRequired,
            EventKind::DeliveryResult,
            EventKind::SystemError,
        ] {
            assert!(subscribes(&none, kind), "空订阅应命中 {kind:?}");
        }
        let subs = vec![EventKind::AccountOffline, EventKind::DeliveryResult];
        assert!(subscribes(&subs, EventKind::AccountOffline), "命中");
        assert!(subscribes(&subs, EventKind::DeliveryResult), "命中");
        assert!(!subscribes(&subs, EventKind::SystemError), "未命中不发送");
        assert!(!subscribes(&subs, EventKind::AccountRecovered), "未命中不发送");
    }

    #[test]
    fn 订阅_严格解析拒绝非法值_宽松解析忽略_序列化往返() {
        let values = vec![
            "account_offline".to_string(),
            "delivery_result".to_string(),
        ];
        let parsed = parse_event_types_strict(&values).unwrap();
        assert_eq!(parsed.len(), 2);
        // 重复项去重
        let dup = vec!["account_offline".into(), "account_offline".into()];
        assert_eq!(parse_event_types_strict(&dup).unwrap().len(), 1);
        // 非法值整体拒绝
        let bad = vec!["account_offline".into(), "order_paid".into()];
        assert!(parse_event_types_strict(&bad).is_err());
        // 宽松解析忽略非法存储项(防御)
        assert_eq!(parse_event_types_lenient(r#"["account_offline","xx"]"#).len(), 1);
        assert_eq!(parse_event_types_lenient("not json").len(), 0);
        // 序列化往返
        let json = event_types_json(&parsed);
        assert_eq!(parse_event_types_lenient(&json), parsed);
        assert_eq!(event_types_json(&[]), "[]");
    }

    // ---------- T056:渠道 kind 七枚举 ----------

    #[test]
    fn 渠道类型_七枚举往返与非法值() {
        assert_eq!(CHANNEL_KINDS.len(), 7, "FR-050 七类渠道");
        for k in [
            ChannelKind::Webhook,
            ChannelKind::Email,
            ChannelKind::Dingtalk,
            ChannelKind::Feishu,
            ChannelKind::Wecom,
            ChannelKind::Bark,
            ChannelKind::Telegram,
        ] {
            assert_eq!(ChannelKind::parse(k.as_str()), Some(k));
        }
        assert!(ChannelKind::parse("sms").is_none());
        assert!(ChannelKind::parse("").is_none());
        // 事件类型六枚举
        assert_eq!(EVENT_KINDS.len(), 6);
        for name in EVENT_KINDS {
            assert!(EventKind::parse(name).is_some(), "{name} 应可解析");
        }
        assert!(EventKind::parse("order_paid").is_none());
    }

    // ---------- T056:secrets 字段名合法集 ----------

    #[test]
    fn 秘密键_白名单与必填集_按类型() {
        // webhook/wecom 无秘密字段
        assert!(allowed_secret_keys(ChannelKind::Webhook).is_empty());
        assert!(allowed_secret_keys(ChannelKind::Wecom).is_empty());
        // 钉钉/飞书:加签 secret 可选
        assert_eq!(allowed_secret_keys(ChannelKind::Dingtalk), &["secret"]);
        assert_eq!(allowed_secret_keys(ChannelKind::Feishu), &["secret"]);
        assert!(required_secret_keys(ChannelKind::Dingtalk).is_empty());
        // bark/telegram:必填
        assert_eq!(allowed_secret_keys(ChannelKind::Bark), &["device_key"]);
        assert_eq!(required_secret_keys(ChannelKind::Bark), &["device_key"]);
        assert_eq!(allowed_secret_keys(ChannelKind::Telegram), &["bot_token"]);
        assert_eq!(required_secret_keys(ChannelKind::Telegram), &["bot_token"]);
        // email:独立 SMTP 凭据可选(匿名中继允许)
        assert_eq!(
            allowed_secret_keys(ChannelKind::Email),
            &["smtp_user", "smtp_password"]
        );
        assert!(required_secret_keys(ChannelKind::Email).is_empty());
    }

    #[test]
    fn 秘密键_校验拒绝未知键与缺失必填() {
        let mut bad_key = BTreeMap::new();
        bad_key.insert("token".to_string(), "x".to_string());
        assert!(check_secrets(ChannelKind::Webhook, &bad_key, false).is_err());
        assert!(check_secrets(ChannelKind::Dingtalk, &bad_key, false).is_err());

        // telegram 创建缺 bot_token → 拒绝;编辑(不要求必填)→ 放行
        let empty = BTreeMap::new();
        assert!(check_secrets(ChannelKind::Telegram, &empty, true).is_err());
        assert!(check_secrets(ChannelKind::Telegram, &empty, false).is_ok());

        let mut ok = BTreeMap::new();
        ok.insert("bot_token".to_string(), "123:abc".to_string());
        assert!(check_secrets(ChannelKind::Telegram, &ok, true).is_ok());

        // 钉钉 secret 可选:空包创建合法
        assert!(check_secrets(ChannelKind::Dingtalk, &empty, true).is_ok());
        let mut ding = BTreeMap::new();
        ding.insert("secret".to_string(), "sec".to_string());
        assert!(check_secrets(ChannelKind::Dingtalk, &ding, true).is_ok());
    }

    // ---------- T056:渠道配置校验 ----------

    #[test]
    fn 配置校验_各类型必填字段() {
        let ok = serde_json::json!({ "url": "https://oapi.dingtalk.com/robot/send?access_token=x" });
        assert!(check_channel_config(ChannelKind::Dingtalk, &ok).is_ok());
        // 缺 url / 非 http(s) / 非对象
        assert!(check_channel_config(ChannelKind::Dingtalk, &serde_json::json!({})).is_err());
        assert!(check_channel_config(
            ChannelKind::Webhook,
            &serde_json::json!({ "url": "ftp://example.com" })
        )
        .is_err());
        assert!(check_channel_config(ChannelKind::Webhook, &serde_json::json!("x")).is_err());

        let bark = serde_json::json!({ "server_url": "https://api.day.app" });
        assert!(check_channel_config(ChannelKind::Bark, &bark).is_ok());
        assert!(check_channel_config(ChannelKind::Bark, &serde_json::json!({})).is_err());

        let tg = serde_json::json!({ "chat_id": "-100123456" });
        assert!(check_channel_config(ChannelKind::Telegram, &tg).is_ok());
        assert!(check_channel_config(ChannelKind::Telegram, &serde_json::json!({})).is_err());

        // email:系统 SMTP 只需收件人;独立 SMTP 需 host+from
        let sys = serde_json::json!({ "to_address": "a@b.c", "use_system_smtp": true });
        assert!(check_channel_config(ChannelKind::Email, &sys).is_ok());
        assert!(check_channel_config(
            ChannelKind::Email,
            &serde_json::json!({ "to_address": "a@b.c" })
        )
        .is_err());
        let standalone = serde_json::json!({
            "to_address": "a@b.c", "smtp_host": "smtp.example.com",
            "smtp_port": 465, "from_address": "shop@example.com"
        });
        assert!(check_channel_config(ChannelKind::Email, &standalone).is_ok());
        // 端口类型错误拒绝
        let bad_port = serde_json::json!({
            "to_address": "a@b.c", "smtp_host": "smtp.example.com",
            "smtp_port": "abc", "from_address": "shop@example.com"
        });
        assert!(check_channel_config(ChannelKind::Email, &bad_port).is_err());
    }
}
