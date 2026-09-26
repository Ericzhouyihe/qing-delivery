//! 007 US5 通知发送适配器(T060):NotifySender 端口与七类实现。
//! webhook/bark/telegram/dingtalk(加签)/feishu(签名)/wecom 走 reqwest,
//! email 走 lettre(lettre 0.11 异步 SMTP;STARTTLS/SSL/明文按 encryption 互斥)。
//! 全部 10 秒超时;HTTP 2xx=Accepted、4xx/5xx=NotSent(带响应摘录)、
//! 超时/网络错=Unknown(参考 SendOutcome 语义;宪章红线:网络绝不进 DbThread 事务)。
//! payload 构造为纯函数(单测不发网);签名算法在 domain::notify(T056 已测)。

use std::collections::BTreeMap;
use std::time::Duration;

use crate::domain::notify::{
    ChannelConfig, ChannelKind, NotifyEventData, dingtalk_signed_url, feishu_sign,
};

/// 单渠道发送上限(research D8:单渠道 10s 超时)
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// 响应摘录长度(错误提示只保留前缀,防泄漏大响应体)
const RESPONSE_EXCERPT: usize = 200;

/// 发送结果三态(参考 SendOutcome 语义;由调用方落 notification_deliveries)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState {
    /// 渠道明确接纳(HTTP 2xx / SMTP 250)
    Accepted,
    /// 确定未送达(4xx/5xx;附响应摘录或原因)
    NotSent(String),
    /// 结果未知(超时/网络错误;禁止盲目自动重发,FR-054)
    Unknown(String),
}

impl SendState {
    /// 落库状态字符串(data-model CHECK 集合)。
    pub fn as_str(&self) -> &'static str {
        match self {
            SendState::Accepted => "accepted",
            SendState::NotSent(_) => "not_sent",
            SendState::Unknown(_) => "unknown",
        }
    }

    pub fn error_hint(&self) -> Option<String> {
        match self {
            SendState::Accepted => None,
            SendState::NotSent(h) | SendState::Unknown(h) => Some(h.clone()),
        }
    }
}

/// 单渠道发送端口:实现方完成一次投递;Err 仅用于本地构造失败
/// (配置缺失/非法等确定性失败,调用方按 NotSent 处理)。
#[async_trait::async_trait]
pub trait NotifySender: Send + Sync {
    async fn send(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String>;
}

// ---------- payload 纯函数(不发网;单测覆盖) ----------

/// webhook 通用载荷:{title, content, timestamp, event}。
pub fn webhook_payload(event: &NotifyEventData) -> serde_json::Value {
    serde_json::json!({
        "title": event.title,
        "content": event.content,
        "timestamp": event.timestamp,
        "event": event.event,
    })
}

/// Bark 载荷:POST {server_url}/push {device_key, title, body}。
pub fn bark_payload(device_key: &str, event: &NotifyEventData) -> serde_json::Value {
    serde_json::json!({
        "device_key": device_key,
        "title": event.title,
        "body": event.content,
    })
}

/// Telegram 载荷:POST /bot{token}/sendMessage {chat_id, text}。
pub fn telegram_payload(chat_id: &str, event: &NotifyEventData) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "text": format!("{}\n{}", event.title, event.content),
    })
}

/// 钉钉文本消息载荷:{msgtype:"text", text:{content}}。
pub fn dingtalk_payload(event: &NotifyEventData) -> serde_json::Value {
    serde_json::json!({
        "msgtype": "text",
        "text": { "content": format!("{}\n{}", event.title, event.content) },
    })
}

/// 企业微信文本消息载荷(与钉钉同构)。
pub fn wecom_payload(event: &NotifyEventData) -> serde_json::Value {
    serde_json::json!({
        "msgtype": "text",
        "text": { "content": format!("{}\n{}", event.title, event.content) },
    })
}

/// 飞书文本消息载荷:签名校验开启时 body 附带 timestamp/sign。
pub fn feishu_payload(event: &NotifyEventData, timestamp: Option<&str>, sign: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "msg_type": "text",
        "content": { "text": format!("{}\n{}", event.title, event.content) },
    });
    if let (Some(ts), Some(sign)) = (timestamp, sign) {
        body["timestamp"] = serde_json::Value::String(ts.to_string());
        body["sign"] = serde_json::Value::String(sign.to_string());
    }
    body
}

fn excerpt(body: &str) -> String {
    body.chars().take(RESPONSE_EXCERPT).collect()
}

// ---------- HTTP 发送共用 ----------

/// 通用 JSON POST:2xx=Accepted、4xx/5xx=NotSent(响应摘录)、网络/超时=Unknown。
async fn post_json(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> SendState {
    let resp = client
        .post(url)
        .json(body)
        .timeout(SEND_TIMEOUT)
        .send()
        .await;
    match resp {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                SendState::Accepted
            } else {
                SendState::NotSent(format!("HTTP {} {}", status.as_u16(), excerpt(&text)))
            }
        }
        Err(e) if e.is_timeout() => SendState::Unknown(format!("请求超时({SEND_TIMEOUT:?}):{e}")),
        Err(e) => SendState::Unknown(format!("网络错误:{e}")),
    }
}

/// 共享 reqwest 客户端(连接池复用;单请求超时在每次调用上收紧)。
#[derive(Clone)]
pub struct HttpNotifySenders {
    client: reqwest::Client,
}

impl Default for HttpNotifySenders {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(SEND_TIMEOUT)
                .build()
                .expect("reqwest 客户端构建"),
        }
    }
}

impl HttpNotifySenders {
    /// webhook:POST {title/content/timestamp/event} 到 config.url。
    pub async fn send_webhook(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let url = channel.cfg_str("url").ok_or("webhook 渠道缺少 url")?.to_string();
        Ok(post_json(&self.client, &url, &webhook_payload(event)).await)
    }

    /// bark:POST {server_url}/push {device_key,title,body}(device_key 为秘密)。
    pub async fn send_bark(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let server = channel
            .cfg_str("server_url")
            .ok_or("bark 渠道缺少 server_url")?
            .trim_end_matches('/')
            .to_string();
        let key = channel.secret("device_key").ok_or("bark 渠道缺少 device_key")?;
        let url = format!("{server}/push");
        Ok(post_json(&self.client, &url, &bark_payload(key, event)).await)
    }

    /// telegram:POST api.telegram.org/bot{token}/sendMessage(chat_id 为明文配置)。
    pub async fn send_telegram(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let token = channel.secret("bot_token").ok_or("telegram 渠道缺少 bot_token")?;
        let chat = channel.cfg_str("chat_id").ok_or("telegram 渠道缺少 chat_id")?;
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        Ok(post_json(&self.client, &url, &telegram_payload(chat, event)).await)
    }

    /// dingtalk:webhook + 可选加签(secret 存在时拼 timestamp/sign 查询串)。
    pub async fn send_dingtalk(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let webhook = channel.cfg_str("url").ok_or("dingtalk 渠道缺少 url")?;
        let url = match channel.secret("secret") {
            Some(secret) => dingtalk_signed_url(
                webhook,
                chrono::Utc::now().timestamp_millis(),
                secret,
            ),
            None => webhook.to_string(),
        };
        Ok(post_json(&self.client, &url, &dingtalk_payload(event)).await)
    }

    /// feishu:签名校验开启时 body 附带 timestamp/sign(秒级时间戳)。
    pub async fn send_feishu(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let webhook = channel.cfg_str("url").ok_or("feishu 渠道缺少 url")?.to_string();
        let (ts, sign) = match channel.secret("secret") {
            Some(secret) => {
                let ts = chrono::Utc::now().timestamp();
                (Some(ts.to_string()), Some(feishu_sign(ts, secret)))
            }
            None => (None, None),
        };
        let body = feishu_payload(event, ts.as_deref(), sign.as_deref());
        Ok(post_json(&self.client, &webhook, &body).await)
    }

    /// wecom:群机器人 webhook,免签文本消息。
    pub async fn send_wecom(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let url = channel.cfg_str("url").ok_or("wecom 渠道缺少 url")?.to_string();
        Ok(post_json(&self.client, &url, &wecom_payload(event)).await)
    }
}

// ---------- email(lettre 异步 SMTP) ----------

/// 按 encryption 构建 SMTP 传输(none/starttls/ssl 互斥;不建立连接,纯构造)。
pub fn build_smtp_transport(
    host: &str,
    port: u16,
    encryption: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, String> {
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::transport::smtp::extension::ClientId;
    use lettre::AsyncSmtpTransport;

    let builder = match encryption.to_ascii_lowercase().as_str() {
        "none" | "" => AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(host),
        "starttls" => AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(host)
            .map_err(|e| format!("STARTTLS 传输构建失败:{e}"))?,
        "ssl" | "tls" => {
            let params = TlsParameters::new(host.to_string())
                .map_err(|e| format!("TLS 参数构建失败:{e}"))?;
            AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(host)
                .tls(Tls::Wrapper(params))
        }
        other => return Err(format!("不支持的加密方式 {other}(none/starttls/ssl)")),
    };
    let builder = builder
        .port(port)
        .hello_name(ClientId::Domain("localhost".to_string()))
        .timeout(Some(SEND_TIMEOUT));
    let builder = match (username, password) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => {
            builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                u.to_string(),
                p.to_string(),
            ))
        }
        _ => builder,
    };
    Ok(builder.build())
}

/// 构造邮件(纯函数;from/to 解析失败即确定性错误)。
pub fn build_email_message(
    from_address: &str,
    from_name: Option<&str>,
    to_address: &str,
    event: &NotifyEventData,
) -> Result<lettre::Message, String> {
    let from = match from_name.filter(|n| !n.trim().is_empty()) {
        Some(name) => lettre::message::Mailbox::new(
            Some(name.to_string()),
            from_address
                .parse()
                .map_err(|_| format!("发件地址非法:{from_address}"))?,
        ),
        None => lettre::message::Mailbox::new(
            None,
            from_address
                .parse()
                .map_err(|_| format!("发件地址非法:{from_address}"))?,
        ),
    };
    let to: lettre::message::Mailbox = to_address
        .parse()
        .map_err(|_| format!("收件地址非法:{to_address}"))?;
    lettre::Message::builder()
        .from(from)
        .to(to)
        .subject(&event.title)
        .body(event.content.clone())
        .map_err(|e| format!("邮件构造失败:{e}"))
}

/// email 发送:独立 SMTP(config 字段)或复用系统 SMTP(channel.system_smtp)。
#[derive(Clone, Default)]
pub struct EmailSender;

#[async_trait::async_trait]
impl NotifySender for EmailSender {
    async fn send(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        let use_system = channel
            .config
            .get("use_system_smtp")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let (host, port, encryption, username, password, from_address, from_name) = if use_system {
            let smtp = channel
                .system_smtp
                .as_ref()
                .ok_or("系统 SMTP 未配置,无法发送")?;
            (
                smtp.host.clone(),
                smtp.port,
                smtp.encryption.clone(),
                smtp.username.clone(),
                smtp.password.clone(),
                smtp.from_address.clone(),
                smtp.from_name.clone(),
            )
        } else {
            let host = channel
                .cfg_str("smtp_host")
                .ok_or("email 渠道缺少 smtp_host")?
                .to_string();
            let port = channel
                .config
                .get("smtp_port")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                })
                .unwrap_or(587) as u16;
            let encryption = channel
                .cfg_str("encryption")
                .unwrap_or("starttls")
                .to_string();
            (
                host,
                port,
                encryption,
                channel.secret("smtp_user").map(str::to_string),
                channel.secret("smtp_password").map(str::to_string),
                channel
                    .cfg_str("from_address")
                    .ok_or("email 渠道缺少 from_address")?
                    .to_string(),
                channel.cfg_str("from_name").map(str::to_string),
            )
        };
        let to = channel
            .cfg_str("to_address")
            .ok_or("email 渠道缺少 to_address")?
            .to_string();
        let message = build_email_message(&from_address, from_name.as_deref(), &to, event)?;
        let transport =
            build_smtp_transport(&host, port, &encryption, username.as_deref(), password.as_deref())?;
        use lettre::AsyncTransport;
        Ok(match transport.send(message).await {
            Ok(_) => SendState::Accepted,
            Err(e) => {
                let msg = e.to_string();
                // lettre 明确的会话拒绝(5xx/认证失败)是确定性未送达;
                // 连接中断/超时类按 Unknown 处理
                if msg.contains("timed out") || msg.contains("connect") {
                    SendState::Unknown(format!("SMTP 连接失败:{msg}"))
                } else {
                    SendState::NotSent(format!("SMTP 拒绝:{msg}"))
                }
            }
        })
    }
}

/// 按 kind 路由的默认发送方集合(生产默认;测试可经 with_sender 注入假实现)。
#[derive(Clone)]
pub struct SenderRegistry {
    http: HttpNotifySenders,
    email: EmailSender,
    overrides: BTreeMap<ChannelKind, std::sync::Arc<dyn NotifySender>>,
}

impl Default for SenderRegistry {
    fn default() -> Self {
        Self {
            http: HttpNotifySenders::default(),
            email: EmailSender,
            overrides: BTreeMap::new(),
        }
    }
}

impl SenderRegistry {
    /// 注入(测试/替身)某类渠道的发送实现;返回新注册表。
    pub fn with_override(
        mut self,
        kind: ChannelKind,
        sender: std::sync::Arc<dyn NotifySender>,
    ) -> Self {
        self.overrides.insert(kind, sender);
        self
    }

    pub fn sender(&self, kind: ChannelKind) -> std::sync::Arc<dyn NotifySender> {
        if let Some(s) = self.overrides.get(&kind) {
            return s.clone();
        }
        match kind {
            ChannelKind::Email => std::sync::Arc::new(self.email.clone()) as _,
            _ => std::sync::Arc::new(DispatchHttp {
                http: self.http.clone(),
                kind,
            }) as _,
        }
    }
}

/// 把 http 集合按 kind 分派成单渠道 sender 视图。
struct DispatchHttp {
    http: HttpNotifySenders,
    kind: ChannelKind,
}

#[async_trait::async_trait]
impl NotifySender for DispatchHttp {
    async fn send(
        &self,
        channel: &ChannelConfig,
        event: &NotifyEventData,
    ) -> Result<SendState, String> {
        match self.kind {
            ChannelKind::Webhook => self.http.send_webhook(channel, event).await,
            ChannelKind::Bark => self.http.send_bark(channel, event).await,
            ChannelKind::Telegram => self.http.send_telegram(channel, event).await,
            ChannelKind::Dingtalk => self.http.send_dingtalk(channel, event).await,
            ChannelKind::Feishu => self.http.send_feishu(channel, event).await,
            ChannelKind::Wecom => self.http.send_wecom(channel, event).await,
            ChannelKind::Email => unreachable!("email 走 EmailSender"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> NotifyEventData {
        NotifyEventData {
            event: "account_offline".into(),
            title: "账号掉线".into(),
            content: "店铺「测试」已离线".into(),
            timestamp: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn 载荷_webhook四字段() {
        let p = webhook_payload(&event());
        assert_eq!(p["title"], "账号掉线");
        assert_eq!(p["content"], "店铺「测试」已离线");
        assert_eq!(p["event"], "account_offline");
        assert_eq!(p["timestamp"], "2026-01-01T00:00:00.000Z");
    }

    #[test]
    fn 载荷_bark设备号标题正文() {
        let p = bark_payload("dev-1", &event());
        assert_eq!(p["device_key"], "dev-1");
        assert_eq!(p["title"], "账号掉线");
        assert_eq!(p["body"], "店铺「测试」已离线");
    }

    #[test]
    fn 载荷_telegram_聊天id与拼接文本() {
        let p = telegram_payload("-100123", &event());
        assert_eq!(p["chat_id"], "-100123");
        assert_eq!(p["text"], "账号掉线\n店铺「测试」已离线");
    }

    #[test]
    fn 载荷_钉钉与企业微信文本消息() {
        let p = dingtalk_payload(&event());
        assert_eq!(p["msgtype"], "text");
        assert_eq!(p["text"]["content"], "账号掉线\n店铺「测试」已离线");
        assert_eq!(wecom_payload(&event()), p, "wecom 与钉钉同构");
    }

    #[test]
    fn 载荷_飞书签名可选注入() {
        let plain = feishu_payload(&event(), None, None);
        assert_eq!(plain["msg_type"], "text");
        assert!(plain.get("timestamp").is_none(), "未开签名不带时间戳");

        let signed = feishu_payload(&event(), Some("1600000000"), Some("SIGN="));
        assert_eq!(signed["timestamp"], "1600000000");
        assert_eq!(signed["sign"], "SIGN=");
        assert_eq!(signed["content"]["text"], "账号掉线\n店铺「测试」已离线");
    }

    #[test]
    fn 载荷_响应摘录截断() {
        let long = "x".repeat(500);
        assert!(excerpt(&long).chars().count() == RESPONSE_EXCERPT);
    }

    #[test]
    fn 邮件_构造与非法地址() {
        let msg = build_email_message("shop@example.com", Some("轻交付"), "a@b.c", &event()).unwrap();
        let subject = msg
            .headers()
            .get::<lettre::message::header::Subject>()
            .map(|s| s.as_ref().to_string())
            .unwrap_or_default();
        assert_eq!(subject, "账号掉线");
        // 非法地址 → 确定性错误(NotSent 语义)
        assert!(build_email_message("not-an-address", None, "a@b.c", &event()).is_err());
        assert!(build_email_message("shop@example.com", None, "bad", &event()).is_err());
    }

    #[test]
    fn 邮件_smtp三种加密互斥构造() {
        // 纯构造不联网:STARTTLS/SSL/明文三档
        assert!(build_smtp_transport("smtp.example.com", 587, "starttls", None, None).is_ok());
        assert!(build_smtp_transport("smtp.example.com", 465, "ssl", Some("u"), Some("p")).is_ok());
        assert!(build_smtp_transport("smtp.example.com", 25, "none", None, None).is_ok());
        assert!(build_smtp_transport("smtp.example.com", 25, "magic", None, None).is_err());
    }

    #[test]
    fn 状态映射_accepted_not_sent_unknown() {
        assert_eq!(SendState::Accepted.as_str(), "accepted");
        assert_eq!(
            SendState::NotSent("HTTP 400".into()).as_str(),
            "not_sent"
        );
        assert_eq!(SendState::Unknown("超时".into()).as_str(), "unknown");
        assert!(SendState::Accepted.error_hint().is_none());
        assert_eq!(
            SendState::NotSent("HTTP 400".into()).error_hint().as_deref(),
            Some("HTTP 400")
        );
    }

    #[test]
    fn 注册表_按类型路由与覆盖() {
        let registry = SenderRegistry::default();
        let _ = registry.sender(ChannelKind::Webhook);
        let _ = registry.sender(ChannelKind::Email);
        let tagged = SenderRegistry::default().with_override(
            ChannelKind::Webhook,
            std::sync::Arc::new(TaggedSender),
        );
        let _ = tagged.sender(ChannelKind::Dingtalk); // 未覆盖走真实
        let _ = tagged;
    }

    struct TaggedSender;

    #[async_trait::async_trait]
    impl NotifySender for TaggedSender {
        async fn send(
            &self,
            _channel: &ChannelConfig,
            _event: &NotifyEventData,
        ) -> Result<SendState, String> {
            Ok(SendState::Accepted)
        }
    }
}
