//! 007 US7 系统设置用例(T076):AI 配置读写。
//! - 明文键(system_settings):ai_api_url、ai_model;
//! - 机密键(system_secrets 信封,AAD purpose=system_secrets 与 US5 SMTP 同口径):
//!   ai_api_key——明文只在内存与信封,读取端点只回 `ai_key_configured` 布尔,
//!   编辑留空=不修改(非空=信封重封);
//! - smtp 系列键由 US5 NotifyService 承担,本服务不重复管理。
//! repo 层复用 repos::notify 的 system settings 函数(T061 已建)。

use crate::adapters::aiclient::AiConfig;
use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::notify as repo;
use crate::adapters::windows::keys::DataKey;
use crate::application::notify::SYSTEM_SECRETS_PURPOSE;
use crate::domain::crypto::{self, Aad};

/// 明文/机密键名(data-model §system_settings)。
pub const KEY_AI_API_URL: &str = "ai_api_url";
pub const KEY_AI_MODEL: &str = "ai_model";
pub const KEY_AI_API_KEY: &str = "ai_api_key";
/// 模型名长度上限(防滥用)。
const MODEL_MAX_CHARS: usize = 200;
/// API Key 长度上限(防滥用)。
const API_KEY_MAX_CHARS: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{0}")]
    Invalid(String),
    #[error("系统机密解密失败")]
    Crypto,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// 读取端点 DTO:机密只回 *_configured 布尔,永不回明文。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemSettingsSummary {
    pub ai_api_url: String,
    pub ai_model: String,
    pub ai_key_configured: bool,
}

fn system_secret_aad(key: &str) -> Aad {
    Aad {
        purpose: SYSTEM_SECRETS_PURPOSE.into(),
        entity_id: key.into(),
        content_version: None,
    }
}

#[derive(Clone)]
pub struct SettingsService {
    db: DbThread,
    key: DataKey,
}

impl SettingsService {
    pub fn new(db: DbThread, key: DataKey) -> Self {
        Self { db, key }
    }

    /// 读取系统 AI 配置摘要(机密脱敏)。
    pub async fn get_system(&self) -> Result<SystemSettingsSummary, SettingsError> {
        let (url, model, key_configured) = self
            .db
            .call(move |conn| -> rusqlite::Result<(Option<String>, Option<String>, bool)> {
                let url = repo::get_setting(conn, KEY_AI_API_URL)?;
                let model = repo::get_setting(conn, KEY_AI_MODEL)?;
                let key_configured = repo::get_secret_envelope(conn, KEY_AI_API_KEY)?.is_some();
                Ok((url, model, key_configured))
            })
            .await??;
        Ok(SystemSettingsSummary {
            ai_api_url: url.unwrap_or_default(),
            ai_model: model.unwrap_or_default(),
            ai_key_configured: key_configured,
        })
    }

    /// 更新系统 AI 配置:
    /// - ai_api_url/ai_model:None=不修改;Some 写入(空串=清除);
    /// - ai_api_key:None 或空串=不修改;非空=信封重封。
    pub async fn put_system(
        &self,
        ai_api_url: Option<&str>,
        ai_model: Option<&str>,
        ai_api_key: Option<&str>,
    ) -> Result<SystemSettingsSummary, SettingsError> {
        let url = ai_api_url.map(str::trim);
        if let Some(u) = url.filter(|u| !u.is_empty())
            && !u.starts_with("http://")
            && !u.starts_with("https://")
        {
            return Err(SettingsError::Invalid(
                "AI 服务地址必须是 http(s) URL".into(),
            ));
        }
        let model = ai_model.map(str::trim);
        if let Some(m) = model.filter(|m| !m.is_empty())
            && m.chars().count() > MODEL_MAX_CHARS
        {
            return Err(SettingsError::Invalid(format!(
                "模型名不能超过 {MODEL_MAX_CHARS} 字"
            )));
        }
        let api_key = ai_api_key.map(str::trim);
        if let Some(k) = api_key.filter(|k| !k.is_empty())
            && k.chars().count() > API_KEY_MAX_CHARS
        {
            return Err(SettingsError::Invalid(format!(
                "API Key 不能超过 {API_KEY_MAX_CHARS} 字"
            )));
        }
        // 信封在 DbThread 闭包外构造(密钥只在内存;留空=不修改)
        let key_envelope = api_key
            .filter(|k| !k.is_empty())
            .map(|k| {
                crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &system_secret_aad(KEY_AI_API_KEY),
                    k.as_bytes(),
                )
            });
        let url = url.map(str::to_string);
        let model = model.map(str::to_string);
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                if let Some(u) = url.as_deref() {
                    repo::put_setting(conn, KEY_AI_API_URL, u)?;
                }
                if let Some(m) = model.as_deref() {
                    repo::put_setting(conn, KEY_AI_MODEL, m)?;
                }
                if let Some(envelope) = key_envelope.as_ref() {
                    repo::put_secret_envelope(conn, KEY_AI_API_KEY, envelope)?;
                }
                Ok(())
            })
            .await??;
        self.get_system().await
    }

    /// 内部取值接口:装配 AI 客户端配置(HttpAiProvider/测试连接消费)。
    /// None = 未配置(缺地址或 Key),调用方按"未启用 AI"处理。
    pub async fn load_ai_config(&self) -> Result<Option<AiConfig>, SettingsError> {
        let summary = self.get_system().await?;
        if summary.ai_api_url.trim().is_empty() || !summary.ai_key_configured {
            return Ok(None);
        }
        let key_plain = self.read_ai_api_key().await?;
        Ok(Some(AiConfig {
            api_url: summary.ai_api_url.trim().to_string(),
            api_key: key_plain.unwrap_or_default(),
            model: summary.ai_model.trim().to_string(),
        }))
    }

    /// 解密读取 ai_api_key(仅内部消费;None=未配置)。
    pub async fn read_ai_api_key(&self) -> Result<Option<String>, SettingsError> {
        let envelope = self
            .db
            .call(move |conn| repo::get_secret_envelope(conn, KEY_AI_API_KEY))
            .await??;
        let Some(envelope) = envelope else {
            return Ok(None);
        };
        let plain = crypto::open(&self.key.key, &system_secret_aad(KEY_AI_API_KEY), &envelope)
            .map_err(|_| SettingsError::Crypto)?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|_| SettingsError::Crypto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (tempfile::TempDir, SettingsService) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = crate::adapters::windows::datadir::DataDir::resolve(Some(dir.path())).unwrap();
        let key = crate::adapters::windows::keys::load_or_create(&data_dir.root).unwrap();
        let db = DbThread::spawn(&data_dir.join("settings-sys.db")).unwrap();
        let dir_path = data_dir.root.clone();
        db.call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        let svc = SettingsService::new(
            db,
            DataKey {
                key_id: key.key_id,
                key: key.key,
            },
        );
        (dir, svc)
    }

    #[tokio::test]
    async fn ai_settings_roundtrip_key_masked_and_blank_keeps() {
        let (_dir, svc) = setup().await;
        let init = svc.get_system().await.unwrap();
        assert_eq!(
            init,
            SystemSettingsSummary {
                ai_api_url: String::new(),
                ai_model: String::new(),
                ai_key_configured: false,
            }
        );

        // 写入:url/model 明文、key 进信封
        let updated = svc
            .put_system(Some("https://ai.example.com/v1"), Some("gpt-x"), Some("sk-plain-1"))
            .await
            .unwrap();
        assert_eq!(updated.ai_api_url, "https://ai.example.com/v1");
        assert_eq!(updated.ai_model, "gpt-x");
        assert!(updated.ai_key_configured);
        // 摘要永不携带明文 key
        assert_ne!(format!("{updated:?}").contains("sk-plain-1"), true);

        // key 留空=不修改;url 空串=清除
        let updated = svc
            .put_system(Some(""), None, Some("  "))
            .await
            .unwrap();
        assert_eq!(updated.ai_api_url, "");
        assert!(updated.ai_key_configured, "留空 key 不清退既有信封");

        // 内部取值:key 缺失时 load 返回 None
        let cfg = svc.load_ai_config().await.unwrap();
        assert!(cfg.is_none(), "地址被清除后不产出配置");

        // 非法地址拒绝
        assert!(svc.put_system(Some("ftp://x"), None, None).await.is_err());
    }
}
