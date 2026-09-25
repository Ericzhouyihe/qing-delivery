//! 资料服务(004/T004):载凭证→拉取→落库;风控转 003;Cookie 顺带吸收(FR-004/005)。

use std::sync::Arc;

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::{accounts, credentials};
use crate::adapters::windows::keys::DataKey;
use crate::adapters::xianyu::cookies::CookieJar;
use crate::adapters::xianyu::mtop::client::MtopClient;

use crate::adapters::xianyu::profile::fetch_profile;

pub struct ProfileService {
    db: DbThread,
    key: DataKey,
    verification: Option<Arc<crate::application::verification::service::VerificationService>>,
}

#[derive(Debug, PartialEq)]
pub enum ProfileRefresh {
    Updated {
        nickname: String,
        avatar_url: Option<String>,
    },
    KeptOld {
        reason: String,
    },
    VerificationTriggered,
}

impl ProfileService {
    pub fn new(
        db: DbThread,
        key: DataKey,
        verification: Option<Arc<crate::application::verification::service::VerificationService>>,
    ) -> Self {
        Self {
            db,
            key,
            verification,
        }
    }

    /// 拉取并落库;风控错误构造信号送 003(FR-005),不自行重试。
    pub async fn refresh(&self, account_id: &str) -> Result<ProfileRefresh, String> {
        let account = account_id.to_string();
        let key = self.key.clone();
        // 载入加密凭证 → 构造带 Cookie 的 mtop 客户端
        let loaded: Result<
            Option<crate::adapters::sqlite::repos::credentials::CredentialBlob>,
            rusqlite::Error,
        > = self
            .db
            .call(move |conn| credentials::load(conn, &account, &key.key))
            .await
            .map_err(|e| format!("db:{e}"))?;
        let blob = match loaded {
            Ok(Some(b)) => b,
            Ok(None) => {
                return Ok(ProfileRefresh::KeptOld {
                    reason: "账号无凭证(未接入或已删除)".into(),
                });
            }
            Err(e) => {
                return Ok(ProfileRefresh::KeptOld {
                    reason: format!("凭证读取失败:{e}"),
                });
            }
        };
        let jar = CookieJar::from_json(&blob.cookie_jar_json).unwrap_or_default();
        let jar = Arc::new(tokio::sync::Mutex::new(jar));
        let mtop = MtopClient::new(jar.clone());
        match fetch_profile(&mtop).await {
            Ok(p) => {
                let account = account_id.to_string();
                let nick = if p.nickname.is_empty() {
                    None
                } else {
                    Some(p.nickname.clone())
                };
                let avatar = if p.avatar_url.is_empty() {
                    None
                } else {
                    Some(p.avatar_url.clone())
                };
                let avatar_for_db = avatar.clone();
                let updated = self
                    .db
                    .call(move |conn| {
                        accounts::set_profile(
                            conn,
                            &account,
                            nick.as_deref(),
                            avatar_for_db.as_deref(),
                        )
                    })
                    .await
                    .map_err(|e| format!("db:{e}"))?
                    .map_err(|e| format!("资料落库失败:{e}"))?;
                if !updated {
                    return Ok(ProfileRefresh::KeptOld {
                        reason: "账号不存在".into(),
                    });
                }
                // FR-004:Cookie 顺带吸收(mtop 客户端已吸收响应 Set-Cookie;此处持久化)
                let account2 = account_id.to_string();
                let key2 = self.key.clone();
                let jar_json = jar.lock().await.to_json();
                let next_gen = blob.generation + 1;
                let _ = self
                    .db
                    .call(move |conn| {
                        credentials::save(
                            conn,
                            &account2,
                            &key2.key,
                            &key2.key_id,
                            &jar_json,
                            next_gen,
                        )
                    })
                    .await;
                Ok(ProfileRefresh::Updated {
                    nickname: p.nickname,
                    avatar_url: avatar.clone(),
                })
            }
            Err(crate::application::ports::platform::PlatformError::VerificationRequired) => {
                if let Some(service) = self.verification.as_ref() {
                    service.handle_signal(crate::application::verification::VerificationSignal {
                        account_id: account_id.to_string(),
                        source: crate::application::verification::SignalSource::Mtop,
                        url: None,
                        raw: "资料拉取触发风控".into(),
                    });
                    Ok(ProfileRefresh::VerificationTriggered)
                } else {
                    Ok(ProfileRefresh::KeptOld {
                        reason: "资料拉取遇风控(验证自动化未接入)".into(),
                    })
                }
            }
            Err(e) => Ok(ProfileRefresh::KeptOld {
                reason: format!("资料拉取失败:{e:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (DbThread, DataKey, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("t.db")).unwrap();
        let mig = dir.path().to_path_buf();
        let _migrate = db
            .call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &mig))
            .await
            .unwrap();
        (
            db,
            DataKey {
                key_id: "k".into(),
                key: [0u8; 32],
            },
            dir,
        )
    }

    #[tokio::test]
    async fn 无凭证_保留旧值并说明() {
        let (db, key, _d) = setup().await;
        let svc = ProfileService::new(db, key, None);
        let r = svc.refresh("missing").await.unwrap();
        assert!(matches!(r, ProfileRefresh::KeptOld { .. }));
    }
}
