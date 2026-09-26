//! 007 US5 通知用例(T061/T062):NotifyEvent 内部事件枚举(research D8)、
//! 渠道 CRUD(secrets 整包信封 AAD purpose=notify_secrets;编辑留空=不修改)、
//! 覆盖式账号绑定、订阅过滤 + 绑定覆盖的异步扇出(tokio::spawn,不阻塞事件
//! 管道;网络绝不进 DbThread 事务)、每渠道投递留痕、unknown → issue
//! kind=notify_unknown(无订单账号维度去重,FR-054)、系统设置/系统秘密存储
//! (ai 相关键本任务只建存储,T076/078 才消费)与测试投递。
//! 挂钩入口:on_runtime_changed / on_verification_required / on_system_error /
//! notify_delivery_result / try_issue_evented(supervisor 与交付服务 1—3 行接入)。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::adapters::notify::{NotifySender, SendState, SenderRegistry};
use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::notify as repo;
use crate::adapters::sqlite::repos::notify::ChannelRow;
use crate::adapters::sqlite::repos::{accounts, issues, notify::ChannelUpdate, orders};
use crate::adapters::windows::keys::DataKey;
use crate::domain::crypto::{self, Aad};
use crate::domain::ids;
use crate::domain::notify::{
    ChannelConfig, ChannelKind, EventKind, NotifyEventData, SystemSmtp, check_channel_config,
    check_secrets, event_types_json, parse_event_types_lenient, parse_event_types_strict,
    subscribes,
};
use crate::domain::time_util::{format_rfc3339, utc_now_ms};

/// 渠道秘密信封 AAD 用途(data-model 指定,不得改动)。
const SECRETS_PURPOSE: &str = "notify_secrets";
/// 系统秘密信封 AAD 用途(settings_sys 同口径复用,不得改动)。
pub(crate) const SYSTEM_SECRETS_PURPOSE: &str = "system_secrets";
/// 主题/正文长度上限(投递留痕与外部载荷截断)。
const SUBJECT_CHARS: usize = 200;
const CONTENT_CHARS: usize = 2000;
/// notify_unknown 待处理允许的动作(research D8:终止渠道排查)。
const NOTIFY_UNKNOWN_ACTIONS: &str = "[\"terminate\"]";

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("{0}")]
    Invalid(String),
    #[error("通知渠道不存在")]
    NotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error("通知服务暂不可用:{0}")]
    Unavailable(String),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

// ---------- 输入/输出 DTO(秘密永不回显,只 *_configured) ----------

#[derive(Clone, Debug, Default)]
pub struct ChannelDraft {
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    /// 空(或全空值)= 创建时无秘密 / 编辑时不修改;非空 = 整包替换。
    pub secrets: BTreeMap<String, String>,
    pub event_types: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChannelSummary {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub secrets_configured: Vec<String>,
    pub event_types: Vec<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub encryption: String,
    pub from_address: String,
    pub from_name: Option<String>,
    pub username: Option<String>,
    /// None/空 = 不修改;非空 = 替换(信封)。
    pub password: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SmtpSummary {
    pub configured: bool,
    pub host: String,
    pub port: u16,
    pub encryption: String,
    pub from_address: String,
    pub from_name: Option<String>,
    pub username: Option<String>,
    pub password_configured: bool,
}

// ---------- 内部事件枚举(research D8) ----------

/// 通知事件:事件源挂钩点(supervisor/issues/delivery)→ dispatch 异步扇出。
#[derive(Clone, Debug)]
pub enum NotifyEvent {
    AccountOffline {
        account_id: String,
        account_name: Option<String>,
    },
    AccountRecovered {
        account_id: String,
        account_name: Option<String>,
    },
    SecurityVerification {
        account_id: String,
        account_name: Option<String>,
    },
    ManualInterventionRequired {
        account_id: Option<String>,
        issue_kind: String,
        reason: String,
        order_ref: Option<String>,
    },
    DeliveryResult {
        account_id: String,
        order_ref: String,
        /// not_sent / unknown(accepted 不通知:FR-051"交付结果"语义=
        /// 需要关注的失败/未知,成功降噪——显式取舍)
        outcome: String,
        summary: String,
    },
    SystemError {
        account_id: Option<String>,
        reason: String,
    },
}

impl NotifyEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            NotifyEvent::AccountOffline { .. } => EventKind::AccountOffline,
            NotifyEvent::AccountRecovered { .. } => EventKind::AccountRecovered,
            NotifyEvent::SecurityVerification { .. } => EventKind::SecurityVerification,
            NotifyEvent::ManualInterventionRequired { .. } => {
                EventKind::ManualInterventionRequired
            }
            NotifyEvent::DeliveryResult { .. } => EventKind::DeliveryResult,
            NotifyEvent::SystemError { .. } => EventKind::SystemError,
        }
    }

    /// 事件账号维度(绑定覆盖裁决用;None = 系统级 → 全部启用渠道)。
    pub fn account_id(&self) -> Option<&str> {
        match self {
            NotifyEvent::AccountOffline { account_id, .. }
            | NotifyEvent::AccountRecovered { account_id, .. }
            | NotifyEvent::SecurityVerification { account_id, .. }
            | NotifyEvent::DeliveryResult { account_id, .. } => Some(account_id),
            NotifyEvent::ManualInterventionRequired { account_id, .. }
            | NotifyEvent::SystemError { account_id, .. } => account_id.as_deref(),
        }
    }

    pub fn subject(&self) -> String {
        let raw = match self {
            NotifyEvent::AccountOffline { .. } => "账号掉线通知".to_string(),
            NotifyEvent::AccountRecovered { .. } => "账号恢复通知".to_string(),
            NotifyEvent::SecurityVerification { .. } => "账号需要安全验证".to_string(),
            NotifyEvent::ManualInterventionRequired { issue_kind, .. } => {
                format!("需要人工处理:{issue_kind}")
            }
            NotifyEvent::DeliveryResult { outcome, .. } => match outcome.as_str() {
                "unknown" => "交付结果未知".to_string(),
                _ => "交付失败".to_string(),
            },
            NotifyEvent::SystemError { .. } => "系统错误".to_string(),
        };
        raw.chars().take(SUBJECT_CHARS).collect()
    }

    pub fn body(&self) -> String {
        let raw = match self {
            NotifyEvent::AccountOffline {
                account_id,
                account_name,
            } => format!(
                "账号 {}({account_id})已离线,请检查登录状态。",
                who(account_name)
            ),
            NotifyEvent::AccountRecovered {
                account_id,
                account_name,
            } => format!(
                "账号 {}({account_id})已恢复在线。",
                who(account_name)
            ),
            NotifyEvent::SecurityVerification {
                account_id,
                account_name,
            } => format!(
                "账号 {}({account_id})触发平台安全验证,请在账号页处理。",
                who(account_name)
            ),
            NotifyEvent::ManualInterventionRequired {
                issue_kind,
                reason,
                order_ref,
                ..
            } => match order_ref {
                Some(order) => format!("待处理事项 {issue_kind}:{reason}(订单 {order})"),
                None => format!("待处理事项 {issue_kind}:{reason}"),
            },
            NotifyEvent::DeliveryResult {
                order_ref,
                outcome,
                summary,
                ..
            } => format!("订单 {order_ref} 交付{outcome}:{summary}"),
            NotifyEvent::SystemError { reason, .. } => format!("系统错误:{reason}"),
        };
        raw.chars().take(CONTENT_CHARS).collect()
    }

    fn data(&self) -> NotifyEventData {
        NotifyEventData {
            event: self.kind().as_str().to_string(),
            title: self.subject(),
            content: self.body(),
            timestamp: format_rfc3339(utc_now_ms()),
        }
    }
}

fn who(name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| "账号".into())
}

// ---------- 服务 ----------

#[derive(Clone)]
pub struct NotifyService {
    db: DbThread,
    key: DataKey,
    registry: SenderRegistry,
}

/// 绑定 PUT 校验结果(闭包内不抛 rusqlite 伪错误)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindCheck {
    Ok,
    AccountMissing,
    ChannelMissing,
}

impl NotifyService {
    pub fn new(db: DbThread, key: DataKey) -> Self {
        Self {
            db,
            key,
            registry: SenderRegistry::default(),
        }
    }

    /// 注入某类渠道发送实现(集成测试假发送方;生产默认真实实现)。
    pub fn with_sender(mut self, kind: ChannelKind, sender: Arc<dyn NotifySender>) -> Self {
        self.registry = self.registry.with_override(kind, sender);
        self
    }

    fn channel_aad(channel_id: &str) -> Aad {
        Aad {
            purpose: SECRETS_PURPOSE.into(),
            entity_id: channel_id.into(),
            content_version: None,
        }
    }

    fn system_secret_aad(key: &str) -> Aad {
        Aad {
            purpose: SYSTEM_SECRETS_PURPOSE.into(),
            entity_id: key.into(),
            content_version: None,
        }
    }

    /// 行 → 发送侧 ChannelConfig(秘密解密;email 复用系统 SMTP 时注入参数)。
    /// 返回 None:kind/配置存储损坏(防御,跳过该渠道并告警)。
    async fn channel_config(&self, row: &ChannelRow) -> Option<ChannelConfig> {
        let kind = ChannelKind::parse(&row.kind)?;
        let config: serde_json::Value = serde_json::from_str(&row.config_json).ok()?;
        let secrets = self.decrypt_channel_secrets(row).unwrap_or_default();
        let event_types = parse_event_types_lenient(&row.event_types_json);
        let system_smtp = if kind == ChannelKind::Email
            && config
                .get("use_system_smtp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            self.load_system_smtp().await
        } else {
            None
        };
        Some(ChannelConfig {
            id: row.id.clone(),
            kind,
            name: row.name.clone(),
            enabled: row.enabled,
            config,
            secrets,
            event_types,
            system_smtp,
        })
    }

    fn decrypt_channel_secrets(&self, row: &ChannelRow) -> Option<BTreeMap<String, String>> {
        let envelope = repo::row_envelope(row)?;
        let plain = crypto::open(&self.key.key, &Self::channel_aad(&row.id), &envelope).ok()?;
        let map: BTreeMap<String, String> = serde_json::from_slice(&plain).ok()?;
        Some(map)
    }

    /// 解密后的秘密键名(脱敏回显:只键名,不回明文)。
    fn configured_keys(&self, row: &ChannelRow) -> Vec<String> {
        self.decrypt_channel_secrets(row)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn seal_channel_secrets(
        &self,
        channel_id: &str,
        secrets: &BTreeMap<String, String>,
    ) -> Option<crypto::Envelope> {
        if secrets.is_empty() {
            return None;
        }
        let plain = serde_json::to_vec(secrets).expect("秘密字典序列化");
        Some(crypto::seal(
            &self.key.key,
            &self.key.key_id,
            &Self::channel_aad(channel_id),
            &plain,
        ))
    }

    fn summary_of(&self, row: &ChannelRow) -> ChannelSummary {
        ChannelSummary {
            id: row.id.clone(),
            kind: row.kind.clone(),
            name: row.name.clone(),
            enabled: row.enabled,
            config: serde_json::from_str(&row.config_json).unwrap_or(serde_json::json!({})),
            secrets_configured: self.configured_keys(row),
            event_types: parse_event_types_lenient(&row.event_types_json)
                .iter()
                .map(|k| k.as_str().to_string())
                .collect(),
            version: row.version,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }

    /// 草稿校验(创建/更新共用):kind/名称/event_types/config/secrets 白名单;
    /// secrets_required=true 时必填秘密键须非空(创建或整包替换)。
    #[allow(clippy::type_complexity)]
    fn validate_draft(
        &self,
        draft: &ChannelDraft,
        secrets_required: bool,
    ) -> Result<(ChannelKind, Vec<EventKind>, BTreeMap<String, String>), NotifyError> {
        let kind = ChannelKind::parse(&draft.kind)
            .ok_or_else(|| NotifyError::Invalid(format!("非法渠道类型:{}", draft.kind)))?;
        if draft.name.trim().is_empty() {
            return Err(NotifyError::Invalid("渠道名称不能为空".into()));
        }
        let event_types =
            parse_event_types_strict(&draft.event_types).map_err(NotifyError::Invalid)?;
        check_channel_config(kind, &draft.config).map_err(NotifyError::Invalid)?;
        // 空串值视为未提供(编辑表单留空占位)
        let secrets: BTreeMap<String, String> = draft
            .secrets
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.clone(), v.trim().to_string()))
            .collect();
        check_secrets(kind, &secrets, secrets_required).map_err(NotifyError::Invalid)?;
        Ok((kind, event_types, secrets))
    }

    // ---------- 渠道 CRUD ----------

    pub async fn create_channel(&self, draft: ChannelDraft) -> Result<ChannelSummary, NotifyError> {
        let (kind, event_types, secrets) = self.validate_draft(&draft, true)?;
        let id = ids::new_id("nch");
        let envelope = self.seal_channel_secrets(&id, &secrets);
        let config_json = serde_json::to_string(&draft.config)
            .map_err(|e| NotifyError::Invalid(format!("config 序列化失败:{e}")))?;
        let row = self
            .db
            .call({
                let id = id.clone();
                let kind = kind.as_str().to_string();
                let name = draft.name.trim().to_string();
                let enabled = draft.enabled;
                let config_json = config_json.clone();
                let types_json = event_types_json(&event_types);
                move |conn| {
                    repo::insert_channel(
                        conn,
                        &id,
                        &kind,
                        &name,
                        enabled,
                        &config_json,
                        envelope.as_ref(),
                        &types_json,
                    )
                }
            })
            .await??;
        Ok(self.summary_of(&row))
    }

    pub async fn get_channel(&self, id: &str) -> Result<ChannelSummary, NotifyError> {
        let row = self
            .db
            .call({
                let id = id.to_string();
                move |conn| repo::get_channel(conn, &id)
            })
            .await??;
        row.map(|r| self.summary_of(&r))
            .ok_or(NotifyError::NotFound)
    }

    pub async fn list_channels(&self) -> Result<Vec<ChannelSummary>, NotifyError> {
        let rows = self.db.call(|conn| repo::list_channels(conn)).await??;
        Ok(rows.iter().map(|r| self.summary_of(r)).collect())
    }

    /// 更新(整体替换 name/enabled/config/event_types;secrets 留空=不修改,
    /// 非空=整包替换);乐观锁 version;409 version_conflict。
    pub async fn update_channel(
        &self,
        id: &str,
        expected_version: i64,
        draft: ChannelDraft,
    ) -> Result<ChannelSummary, NotifyError> {
        // 替换整包时必填秘密键须齐(新包自身完整);留空则沿用旧包
        let provided = secrets_provided(&draft);
        let (_kind, event_types, secrets) = self.validate_draft(&draft, provided)?;
        let envelope = self.seal_channel_secrets(id, &secrets);
        let config_json = serde_json::to_string(&draft.config)
            .map_err(|e| NotifyError::Invalid(format!("config 序列化失败:{e}")))?;
        let outcome = self
            .db
            .call({
                let id = id.to_string();
                let name = draft.name.trim().to_string();
                let enabled = draft.enabled;
                let config_json = config_json.clone();
                let types_json = event_types_json(&event_types);
                move |conn| {
                    repo::update_channel(
                        conn,
                        &id,
                        &name,
                        enabled,
                        &config_json,
                        envelope.as_ref(),
                        &types_json,
                        expected_version,
                    )
                }
            })
            .await??;
        match outcome {
            ChannelUpdate::Updated => self.get_channel(id).await,
            ChannelUpdate::NotFound => Err(NotifyError::NotFound),
            ChannelUpdate::VersionConflict => Err(NotifyError::VersionConflict),
        }
    }

    pub async fn delete_channel(&self, id: &str) -> Result<bool, NotifyError> {
        self.db
            .call({
                let id = id.to_string();
                move |conn| repo::delete_channel(conn, &id)
            })
            .await?
            .map_err(NotifyError::from)
    }

    // ---------- 绑定(FR-052 覆盖式) ----------

    pub async fn put_bindings(
        &self,
        account_id: &str,
        channel_ids: &[String],
    ) -> Result<(), NotifyError> {
        let account = account_id.to_string();
        let channels = channel_ids.to_vec();
        let check = self
            .db
            .call({
                let account = account.clone();
                let channels = channels.clone();
                move |conn| -> rusqlite::Result<BindCheck> {
                    if accounts::get(conn, &account)?.is_none() {
                        return Ok(BindCheck::AccountMissing);
                    }
                    for cid in &channels {
                        if repo::get_channel(conn, cid)?.is_none() {
                            return Ok(BindCheck::ChannelMissing);
                        }
                    }
                    repo::put_bindings(conn, &account, &channels)?;
                    Ok(BindCheck::Ok)
                }
            })
            .await??;
        match check {
            BindCheck::Ok => Ok(()),
            BindCheck::AccountMissing => Err(NotifyError::Invalid("账号不存在,无法绑定".into())),
            BindCheck::ChannelMissing => {
                Err(NotifyError::Invalid("存在未知渠道,无法绑定".into()))
            }
        }
    }

    pub async fn get_bindings(&self, account_id: &str) -> Result<Vec<String>, NotifyError> {
        self.db
            .call({
                let account = account_id.to_string();
                move |conn| repo::list_bindings(conn, &account)
            })
            .await?
            .map_err(NotifyError::from)
    }

    // ---------- 系统设置与系统秘密(明文/信封存储;US7 AI 键同机制只建存储) ----------

    pub async fn get_system_smtp(&self) -> Result<SmtpSummary, NotifyError> {
        let (values, password_configured) = self
            .db
            .call(move |conn| -> rusqlite::Result<(Vec<(String, String)>, bool)> {
                let values = repo::list_settings(conn)?;
                let password_configured =
                    repo::get_secret_envelope(conn, "smtp_password")?.is_some();
                Ok((values, password_configured))
            })
            .await??;
        let map: BTreeMap<String, String> = values.into_iter().collect();
        Ok(SmtpSummary {
            configured: map
                .get("smtp_host")
                .is_some_and(|h| !h.trim().is_empty()),
            host: map.get("smtp_host").cloned().unwrap_or_default(),
            port: map
                .get("smtp_port")
                .and_then(|v| v.parse().ok())
                .unwrap_or(587),
            encryption: map.get("smtp_encryption").cloned().unwrap_or_default(),
            from_address: map.get("smtp_from_address").cloned().unwrap_or_default(),
            from_name: map.get("smtp_from_name").cloned(),
            username: map.get("smtp_user").cloned(),
            password_configured,
        })
    }

    pub async fn put_system_smtp(&self, cfg: &SmtpConfig) -> Result<SmtpSummary, NotifyError> {
        let host = cfg.host.trim();
        if host.is_empty() {
            return Err(NotifyError::Invalid("SMTP 服务器不能为空".into()));
        }
        if !matches!(cfg.encryption.as_str(), "none" | "starttls" | "ssl") {
            return Err(NotifyError::Invalid(
                "加密方式仅支持 none/starttls/ssl".into(),
            ));
        }
        if !cfg.from_address.contains('@') {
            return Err(NotifyError::Invalid("发件地址不是合法邮箱".into()));
        }
        // 信封在闭包外构造(密钥在内存;留空密码=不修改)
        let password_envelope = cfg
            .password
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &Self::system_secret_aad("smtp_password"),
                    p.as_bytes(),
                )
            });
        self.db
            .call({
                let host = host.to_string();
                let port = cfg.port.to_string();
                let encryption = cfg.encryption.clone();
                let from_address = cfg.from_address.clone();
                let from_name = cfg.from_name.clone().unwrap_or_default();
                let username = cfg.username.clone().unwrap_or_default();
                move |conn| -> rusqlite::Result<()> {
                    let pairs = [
                        ("smtp_host", host),
                        ("smtp_port", port),
                        ("smtp_encryption", encryption),
                        ("smtp_from_address", from_address),
                        ("smtp_from_name", from_name),
                        ("smtp_user", username),
                    ];
                    for (key, value) in pairs {
                        repo::put_setting(conn, key, &value)?;
                    }
                    if let Some(envelope) = password_envelope.as_ref() {
                        repo::put_secret_envelope(conn, "smtp_password", envelope)?;
                    }
                    Ok(())
                }
            })
            .await??;
        self.get_system_smtp().await
    }

    // ---------- 系统秘密通用存取(US7 ai_api_key 等同机制;本任务只建存储) ----------

    pub async fn read_system_secret(&self, key: &str) -> Result<Option<String>, NotifyError> {
        let secret_key = key.to_string();
        let raw = self
            .db
            .call(move |conn| repo::get_secret_envelope(conn, &secret_key))
            .await??;
        let Some(envelope) = raw else {
            return Ok(None);
        };
        let plain =
            crypto::open(&self.key.key, &Self::system_secret_aad(key), &envelope)
                .map_err(|e| NotifyError::Unavailable(e.to_string()))?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|e| NotifyError::Unavailable(e.to_string()))
    }

    pub async fn write_system_secret(&self, key: &str, value: &str) -> Result<(), NotifyError> {
        let envelope = crypto::seal(
            &self.key.key,
            &self.key.key_id,
            &Self::system_secret_aad(key),
            value.as_bytes(),
        );
        self.db
            .call({
                let key = key.to_string();
                move |conn| repo::put_secret_envelope(conn, &key, &envelope)
            })
            .await??;
        Ok(())
    }

    pub async fn system_setting(&self, key: &str) -> Result<Option<String>, NotifyError> {
        self.db
            .call({
                let key = key.to_string();
                move |conn| repo::get_setting(conn, &key)
            })
            .await?
            .map_err(NotifyError::from)
    }

    pub async fn set_system_setting(&self, key: &str, value: &str) -> Result<(), NotifyError> {
        self.db
            .call({
                let key = key.to_string();
                let value = value.to_string();
                move |conn| repo::put_setting(conn, &key, &value)
            })
            .await?
            .map_err(NotifyError::from)
    }

    /// 系统 SMTP → 发送侧参数(email 渠道 use_system_smtp 时注入)。
    async fn load_system_smtp(&self) -> Option<SystemSmtp> {
        let loaded = self.get_system_smtp().await.ok()?;
        if !loaded.configured {
            return None;
        }
        Some(SystemSmtp {
            host: loaded.host,
            port: loaded.port,
            encryption: loaded.encryption,
            username: loaded.username,
            password: self
                .read_system_secret("smtp_password")
                .await
                .ok()
                .flatten(),
            from_address: loaded.from_address,
            from_name: loaded.from_name,
        })
    }

    // ---------- 扇出(FR-051/FR-052/FR-054) ----------

    /// 事件派发入口:tokio::spawn 异步扇出,绝不阻塞事件管道
    /// (通知失败只留痕/转待处理,不影响主链路)。
    pub fn dispatch(&self, event: NotifyEvent) {
        let svc = self.clone();
        tokio::spawn(async move {
            svc.fanout(&event).await;
        });
    }

    /// 扇出本体(公开供集成测试等待完成):订阅过滤 + 账号绑定覆盖
    /// (事件带账号且有绑定 → 仅绑定渠道;无绑定/无账号 → 全部启用渠道),
    /// 逐渠道发送留痕;unknown → issue notify_unknown。
    pub async fn fanout(&self, event: &NotifyEvent) {
        let candidates: Vec<ChannelRow> = match self.fanout_candidates(event).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "通知扇出:候选渠道装载失败");
                return;
            }
        };
        if candidates.is_empty() {
            return;
        }
        let data = event.data();
        for row in candidates {
            let Some(config) = self.channel_config(&row).await else {
                tracing::warn!(channel = %row.id, "通知渠道存储损坏,跳过");
                continue;
            };
            let sender = self.registry.sender(config.kind);
            // 网络:仅在 DbThread 闭包之外执行(宪章红线)
            let result = sender.send(&config, &data).await;
            // Err = 本地构造失败(确定性未送达)→ NotSent
            let state = result.unwrap_or_else(SendState::NotSent);
            self.persist_delivery(&row.id, event, &data, &state).await;
        }
    }

    /// 候选渠道:启用 + 订阅匹配 + 绑定覆盖(单次 DbThread 闭包装载)。
    async fn fanout_candidates(&self, event: &NotifyEvent) -> Result<Vec<ChannelRow>, NotifyError> {
        let event_kind = event.kind();
        let account = event.account_id().map(str::to_string);
        let rows = self
            .db
            .call(move |conn| -> rusqlite::Result<Vec<ChannelRow>> {
                let mut rows = repo::list_enabled_channels(conn)?;
                if let Some(account) = account.as_deref() {
                    let bindings = repo::list_bindings(conn, account)?;
                    if !bindings.is_empty() {
                        rows.retain(|r| bindings.contains(&r.id));
                    }
                }
                Ok(rows
                    .into_iter()
                    .filter(|r| {
                        subscribes(&parse_event_types_lenient(&r.event_types_json), event_kind)
                    })
                    .collect())
            })
            .await??;
        Ok(rows)
    }

    /// 投递留痕 + unknown 转待处理(FR-054:结果未知不盲目自动重发)。
    async fn persist_delivery(
        &self,
        channel_id: &str,
        event: &NotifyEvent,
        data: &NotifyEventData,
        state: &SendState,
    ) {
        let delivery_id = ids::new_time_ordered_id("ndl");
        let hint = state.error_hint();
        let hint_for_issue = hint.clone();
        let persisted = self
            .db
            .call({
                let id = delivery_id.clone();
                let channel = channel_id.to_string();
                let event_kind = data.event.clone();
                let subject = data.title.clone();
                let state_str = state.as_str().to_string();
                let hint = hint.clone();
                move |conn| -> rusqlite::Result<()> {
                    repo::insert_delivery(
                        conn,
                        &id,
                        Some(&channel),
                        &event_kind,
                        &subject,
                        &state_str,
                        hint.as_deref(),
                    )
                }
            })
            .await;
        if let Err(e) = persisted {
            tracing::warn!(error = %e, "通知投递留痕失败");
        }
        if matches!(state, SendState::Unknown(_)) {
            self.open_notify_unknown_issue(channel_id, event, data, hint_for_issue)
                .await;
        }
    }

    /// unknown → issue kind=notify_unknown(reason_code=channel:{id};
    /// 同账号同 kind 同 reason 的 open 事项唯一,由 0006 的
    /// idx_issues_open_dedup_account 索引兜底 + issues::open 预查去重)。
    /// 事件无账号维度时只留痕不开事项(issues.account_id NOT NULL,
    /// 无法挂靠;审计已由 notification_deliveries 保留)。
    async fn open_notify_unknown_issue(
        &self,
        channel_id: &str,
        event: &NotifyEvent,
        data: &NotifyEventData,
        hint: Option<String>,
    ) {
        let Some(account_id) = event.account_id().map(str::to_string) else {
            tracing::warn!(
                channel = channel_id,
                "通知投递结果未知但事件无账号维度,仅留痕不开待处理"
            );
            return;
        };
        let reason_code = format!("channel:{channel_id}");
        let summary = format!(
            "通知投递结果未知(渠道 {channel_id},事件 {}):{}",
            data.event,
            hint.unwrap_or_else(|| "无原因说明".into())
        );
        let result = self
            .db
            .call({
                let id = ids::new_id("iss");
                let account = account_id.clone();
                let reason = reason_code.clone();
                let _summary = summary.clone();
                move |conn| {
                    issues::open(
                        conn,
                        &id,
                        None,
                        &account,
                        None,
                        "notify_unknown",
                        &reason,
                        NOTIFY_UNKNOWN_ACTIONS,
                    )
                }
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "notify_unknown 待处理创建失败");
        }
    }

    /// 测试投递(contracts §5 test 端点):即时发送一次,不经订阅过滤
    /// (管理员显式测试该渠道),同步返回结构化结果并留痕。
    pub async fn test_send(&self, channel_id: &str) -> Result<SendState, NotifyError> {
        let row = self
            .db
            .call({
                let id = channel_id.to_string();
                move |conn| repo::get_channel(conn, &id)
            })
            .await??;
        let row = row.ok_or(NotifyError::NotFound)?;
        let config = self
            .channel_config(&row)
            .await
            .ok_or_else(|| NotifyError::Unavailable("渠道存储损坏".into()))?;
        let data = NotifyEventData {
            event: "test".into(),
            title: "[测试] 通知渠道测试".into(),
            content: format!(
                "渠道「{}」测试投递;收到本消息表示链路畅通。",
                config.name
            ),
            timestamp: format_rfc3339(utc_now_ms()),
        };
        let sender = self.registry.sender(config.kind);
        let state = sender.send(&config, &data).await.unwrap_or_else(SendState::NotSent);
        self.persist_test_delivery(&row.id, &data, &state).await;
        Ok(state)
    }

    async fn persist_test_delivery(
        &self,
        channel_id: &str,
        data: &NotifyEventData,
        state: &SendState,
    ) {
        let id = ids::new_time_ordered_id("ndl");
        let hint = state.error_hint();
        let result = self
            .db
            .call({
                let channel = channel_id.to_string();
                let event = data.event.clone();
                let subject = data.title.clone();
                let state = state.as_str().to_string();
                move |conn| -> rusqlite::Result<()> {
                    repo::insert_delivery(
                        conn,
                        &id,
                        Some(&channel),
                        &event,
                        &subject,
                        &state,
                        hint.as_deref(),
                    )
                }
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "测试投递留痕失败");
        }
    }

    /// 最近投递记录(新→旧;审计/前端展示)。
    pub async fn list_deliveries(
        &self,
        limit: i64,
    ) -> Result<Vec<repo::DeliveryRow>, NotifyError> {
        self.db
            .call(move |conn| repo::list_deliveries(conn, limit))
            .await?
            .map_err(NotifyError::from)
    }

    // ---------- 挂钩入口(T062;调用点 1—3 行接入) ----------

    /// supervisor::dispatch_loop 挂钩:AccountRuntimeChanged →
    /// online=account_recovered,其余(offine 等)=account_offline;
    /// 携带账号显示名(spawn,不阻塞事件管道)。
    pub fn on_runtime_changed(&self, account_id: &str, state: &str) {
        let svc = self.clone();
        let account = account_id.to_string();
        let state = state.to_string();
        tokio::spawn(async move {
            let name = svc.account_display_name(&account).await;
            let event = if state == "online" {
                NotifyEvent::AccountRecovered {
                    account_id: account,
                    account_name: name,
                }
            } else {
                NotifyEvent::AccountOffline {
                    account_id: account,
                    account_name: name,
                }
            };
            svc.fanout(&event).await;
        });
    }

    /// supervisor::dispatch_loop 挂钩:AuthorizationChanged(verification_required)
    /// → security_verification(spawn)。
    pub fn on_verification_required(&self, account_id: &str) {
        let svc = self.clone();
        let account = account_id.to_string();
        tokio::spawn(async move {
            let name = svc.account_display_name(&account).await;
            svc.fanout(&NotifyEvent::SecurityVerification {
                account_id: account,
                account_name: name,
            })
            .await;
        });
    }

    /// ProtocolIssue 等系统级错误挂钩 → system_error(spawn;账号可缺)。
    pub fn on_system_error(&self, account_id: Option<&str>, reason: &str) {
        let svc = self.clone();
        let account = account_id.map(str::to_string);
        let reason = reason.to_string();
        tokio::spawn(async move {
            svc.fanout(&NotifyEvent::SystemError {
                account_id: account,
                reason,
            })
            .await;
        });
    }

    /// delivery classify_and_persist 终态挂钩:not_sent(terminal)/unknown
    /// → delivery_result(spawn;accepted 不通知——避免噪音,FR-051
    /// "交付结果"语义=需要关注的失败/未知,见 D8)。
    pub fn notify_delivery_result(
        &self,
        account_id: &str,
        order_ref: &str,
        outcome: &str,
        summary: &str,
    ) {
        let svc = self.clone();
        let event = NotifyEvent::DeliveryResult {
            account_id: account_id.to_string(),
            order_ref: order_ref.to_string(),
            outcome: outcome.to_string(),
            summary: summary.to_string(),
        };
        tokio::spawn(async move {
            svc.fanout(&event).await;
        });
    }

    /// 交付终态挂钩(内部订单 ID 版):解析平台订单号与金额脱敏摘要后派发
    /// delivery_result。金额脱敏:整数部分仅保留首位,小数完全遮蔽——
    /// 通知内容会进入第三方渠道(钉钉/飞书/服务器),不外泄精确成交价。
    pub fn notify_order_result(
        &self,
        account_id: &str,
        order_db_id: &str,
        outcome: &str,
        detail: &str,
    ) {
        let svc = self.clone();
        let account = account_id.to_string();
        let order = order_db_id.to_string();
        let outcome = outcome.to_string();
        let detail = detail.to_string();
        tokio::spawn(async move {
            let loaded: Option<(Option<String>, Option<i64>, Option<String>)> = svc
                .db
                .call({
                    let order = order.clone();
                    move |conn| orders::get_order_notify_ref(conn, &order)
                })
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten();
            let (order_ref, amount_minor, currency) = loaded
                .map(|(ext, minor, cur)| {
                    (ext.unwrap_or_else(|| order.clone()), minor, cur)
                })
                .unwrap_or_else(|| (order.clone(), None, None));
            let summary = format!(
                "{detail}({})",
                masked_amount(amount_minor, currency.as_deref())
            );
            svc.fanout(&NotifyEvent::DeliveryResult {
                account_id: account,
                order_ref,
                outcome,
                summary,
            })
            .await;
        });
    }

    /// issues::open 的应用层包装(research D8):新开 issue(去重未命中)时
    /// 派发 manual_intervention_required;去重命中(已存在同因 open)不重发,
    /// 避免重复轰炸。返回 (issue 行, 是否新建)。
    ///
    /// 迁移注明:既有调用点(delivery service 五处/manual/boot)暂未迁移——
    /// delivery 终态已由 notify_delivery_result 覆盖,重复挂钩会造成同一事件
    /// 双份通知;后续新增开 issue 的调用点应改用本包装(渐进迁移)。
    pub async fn try_issue_evented(
        &self,
        order_id: Option<&str>,
        account_id: &str,
        delivery_id: Option<&str>,
        kind: &str,
        reason_code: &str,
        allowed_actions: &str,
    ) -> Result<(issues::IssueRow, bool), NotifyError> {
        let new_id = ids::new_id("iss");
        let row = self
            .db
            .call({
                let id = new_id.clone();
                let order = order_id.map(str::to_string);
                let account = account_id.to_string();
                let delivery = delivery_id.map(str::to_string);
                let kind = kind.to_string();
                let reason = reason_code.to_string();
                let allowed = allowed_actions.to_string();
                move |conn| {
                    issues::open(
                        conn,
                        &id,
                        order.as_deref(),
                        &account,
                        delivery.as_deref(),
                        &kind,
                        &reason,
                        &allowed,
                    )
                }
            })
            .await??;
        // 去重命中时返回已存在行(id 不同);仅新建时通知
        let created = row.id == new_id;
        if created {
            let event = NotifyEvent::ManualInterventionRequired {
                account_id: Some(account_id.to_string()),
                issue_kind: kind.to_string(),
                reason: reason_code.to_string(),
                order_ref: order_id.map(str::to_string),
            };
            self.dispatch(event);
        }
        Ok((row, created))
    }

    async fn account_display_name(&self, account_id: &str) -> Option<String> {
        let loaded: Option<String> = self
            .db
            .call({
                let id = account_id.to_string();
                move |conn| accounts::get(conn, &id).map(|row| row.map(|a| a.display_name))
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten();
        loaded
            .filter(|n| !n.is_empty())
            .or_else(|| Some(account_id.to_string()))
    }
}

fn secrets_provided(draft: &ChannelDraft) -> bool {
    draft.secrets.values().any(|v| !v.trim().is_empty())
}

/// 金额脱敏摘要:整数部分仅保留首位数字,其余与小数全部遮蔽;
/// 金额未知时如实标注(不当作 0)。通知进入第三方渠道,不外泄精确成交价。
fn masked_amount(minor: Option<i64>, currency: Option<&str>) -> String {
    let Some(minor) = minor else {
        return "金额未知".into();
    };
    let currency = currency.filter(|c| !c.is_empty()).unwrap_or("¥");
    let yuan = (minor / 100).abs().to_string();
    let masked_int = match yuan.len() {
        0 => "0".to_string(),
        1 => yuan,
        n => format!("{}{}", &yuan[..1], "*".repeat(n - 1)),
    };
    let sign = if minor < 0 { "-" } else { "" };
    format!("{sign}{currency} {masked_int}.**")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 金额脱敏:首位保留、其余遮蔽;缺失金额不当作 0。
    #[test]
    fn 金额脱敏_保留首位遮蔽其余() {
        assert_eq!(masked_amount(Some(1234), Some("CNY")), "CNY 1*.**");
        assert_eq!(masked_amount(Some(9_999), None), "¥ 9*.**");
        assert_eq!(masked_amount(Some(50), Some("CNY")), "CNY 0.**");
        assert_eq!(masked_amount(None, Some("CNY")), "金额未知");
    }

    /// 事件文案:六类事件的 subject/body 不含交付正文/秘密。
    #[test]
    fn 事件文案_标题与正文构造() {
        let e = NotifyEvent::DeliveryResult {
            account_id: "acct-1".into(),
            order_ref: "EXT-9".into(),
            outcome: "unknown".into(),
            summary: "结果未知".into(),
        };
        assert_eq!(e.kind(), EventKind::DeliveryResult);
        assert_eq!(e.subject(), "交付结果未知");
        assert!(e.body().contains("EXT-9"));
        let sys = NotifyEvent::SystemError {
            account_id: None,
            reason: "WS 协议异常".into(),
        };
        assert_eq!(sys.account_id(), None);
        assert!(sys.subject().contains("系统错误"));
    }
}
