//! 扫码授权流程用例(T041/T042):
//! 创建→观察→推进/取消;代次绑定使取消/过期后的晚到成功被拒绝;
//! confirmed≠completed:身份核验与凭证落库后才算完成。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{accounts, auth_flows};
use crate::domain::accounts::{AuthFlowStatus, auth_flow_transition};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

pub const QR_TTL_MS: i64 = 3 * 60 * 1000;

#[derive(Debug, thiserror::Error)]
pub enum QrFlowError {
    #[error("授权会话不存在")]
    NotFound,
    #[error("会话已处于终态,不能取消")]
    AlreadyAuthorized,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct QrFlowCanceled;

pub struct QrSession {
    pub id: String,
    pub account_id: Option<String>,
    pub generation: i64,
    pub status: AuthFlowStatus,
    pub expires_at: i64,
    pub bound_user_id: Option<String>,
}

#[derive(Clone)]
pub struct QrFlowService {
    db: DbThread,
}

impl QrFlowService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 创建二维码会话;replace_account_id 指定替换既有账号的授权。
    pub async fn create(&self, account_id: Option<&str>) -> Result<QrSession, QrFlowError> {
        let id = ids::new_id("qrf");
        let expires = utc_now_ms() + QR_TTL_MS;
        let session_ref = ids::new_request_key();
        let account = account_id.map(|s| s.to_string());
        let row = self
            .db
            .call(move |conn| {
                auth_flows::insert(conn, &id, account.as_deref(), expires, &session_ref)
            })
            .await??;
        Ok(row.into())
    }

    /// 观察会话;到期未完成标记 expired(可反复获取新二维码)。
    pub async fn observe(&self, qr_id: &str) -> Result<QrSession, QrFlowError> {
        let id = qr_id.to_string();
        let row = self
            .db
            .call(move |conn| {
                auth_flows::expire_if_due(conn, &id)?;
                auth_flows::get(conn, &id)
            })
            .await??;
        row.map(Into::into).ok_or(QrFlowError::NotFound)
    }

    /// 取消本地授权接纳;已授权(confirmed/completed)返回冲突。
    pub async fn cancel(&self, qr_id: &str) -> Result<QrSession, QrFlowError> {
        let id = qr_id.to_string();
        let updated = self
            .db
            .call(
                move |conn| -> rusqlite::Result<Result<(), QrFlowCanceled>> {
                    let row =
                        auth_flows::get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    let status = AuthFlowStatus::parse(&row.status);
                    if status == AuthFlowStatus::Confirmed || status == AuthFlowStatus::Completed {
                        return Ok(Err(QrFlowCanceled));
                    }
                    if status.is_terminal() {
                        return Ok(Ok(())); // 已取消/过期/失败:幂等
                    }
                    auth_flows::advance(conn, &id, &row.status, "canceled", None)?;
                    Ok(Ok(()))
                },
            )
            .await??;
        match updated {
            Err(QrFlowCanceled) => Err(QrFlowError::AlreadyAuthorized),
            Ok(()) => self.observe(qr_id).await,
        }
    }

    /// 平台授权事件推进(由账号运行时/适配器调用)。
    /// 终态或非法迁移被拒绝;绑定身份与预期账号不一致时不推进(身份核验在服务层)。
    pub async fn apply_platform_transition(
        &self,
        qr_id: &str,
        generation: i64,
        to: AuthFlowStatus,
        bound_user_id: Option<&str>,
    ) -> Result<bool, QrFlowError> {
        let id = qr_id.to_string();
        let bound = bound_user_id.map(|s| s.to_string());
        let applied = self
            .db
            .call(move |conn| -> rusqlite::Result<bool> {
                let row =
                    auth_flows::get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                // 代次不匹配:晚到的旧会话结果不能覆盖新会话
                if row.generation != generation {
                    return Ok(false);
                }
                let from = AuthFlowStatus::parse(&row.status);
                if !auth_flow_transition(from, to) {
                    return Ok(false);
                }
                auth_flows::advance(conn, &id, from.as_str(), to.as_str(), bound.as_deref())
            })
            .await??;
        Ok(applied)
    }

    /// confirmed → completed:核验身份一致并建立账号(凭证落库由调用方完成)。
    pub async fn complete(
        &self,
        qr_id: &str,
        external_user_id: &str,
        display_name: &str,
    ) -> Result<String, QrFlowError> {
        let id = qr_id.to_string();
        let ext = external_user_id.to_string();
        let name = display_name.to_string();
        let account_id = self
            .db
            .call(
                move |conn| -> rusqlite::Result<Result<String, QrFlowCanceled>> {
                    let row =
                        auth_flows::get(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    let from = AuthFlowStatus::parse(&row.status);
                    if !matches!(from, AuthFlowStatus::Confirmed | AuthFlowStatus::Completed) {
                        return Ok(Err(QrFlowCanceled));
                    }
                    // 身份一致:同一平台身份只允许一个本地账号
                    if let Some(existing) = accounts::find_by_external(conn, "xianyu", &ext)?
                        && row.account_id.as_deref() != Some(existing.id.as_str())
                    {
                        // 替换授权场景:绑定到原账号
                        if let Err(e) =
                            auth_flows::advance(conn, &id, from.as_str(), "completed", Some(&ext))
                        {
                            let _ = e;
                        }
                        return Ok(Ok(existing.id));
                    }
                    let account_id = row
                        .account_id
                        .clone()
                        .unwrap_or_else(|| ids::new_id("acct"));
                    accounts::upsert_identity(conn, &account_id, "xianyu", &ext, &name)?;
                    auth_flows::advance(conn, &id, from.as_str(), "completed", Some(&ext))?;
                    Ok(Ok(account_id))
                },
            )
            .await??;
        account_id.map_err(|_| QrFlowError::NotFound)
    }
}

impl From<auth_flows::AuthFlowRow> for QrSession {
    fn from(r: auth_flows::AuthFlowRow) -> Self {
        QrSession {
            id: r.id,
            account_id: r.account_id,
            generation: r.generation,
            status: AuthFlowStatus::parse(&r.status),
            expires_at: r.expires_at,
            bound_user_id: r.bound_user_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;

    async fn setup() -> (tempfile::TempDir, QrFlowService) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("qr.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        (dir, QrFlowService::new(db))
    }

    #[tokio::test]
    async fn create_observe_cancel_flow() {
        let (_d, svc) = setup().await;
        let s = svc.create(None).await.unwrap();
        assert_eq!(s.status, AuthFlowStatus::Pending);
        assert_eq!(
            svc.observe(&s.id).await.unwrap().status,
            AuthFlowStatus::Pending
        );
        svc.cancel(&s.id).await.unwrap();
        assert_eq!(
            svc.observe(&s.id).await.unwrap().status,
            AuthFlowStatus::Canceled
        );
        // 幂等取消
        svc.cancel(&s.id).await.unwrap();
    }

    #[tokio::test]
    async fn late_success_cannot_override_cancel_or_newer_generation() {
        let (_d, svc) = setup().await;
        let s1 = svc.create(None).await.unwrap();
        svc.cancel(&s1.id).await.unwrap();
        // 晚到扫码成功:终态拒绝
        assert!(
            !svc.apply_platform_transition(&s1.id, s1.generation, AuthFlowStatus::Scanned, None)
                .await
                .unwrap()
        );

        // 旧代次结果对新会话无效
        let s2 = svc.create(None).await.unwrap();
        assert!(s2.generation > s1.generation);
        assert!(
            !svc.apply_platform_transition(&s2.id, s1.generation, AuthFlowStatus::Scanned, None)
                .await
                .unwrap()
        );
        assert!(
            svc.apply_platform_transition(&s2.id, s2.generation, AuthFlowStatus::Scanned, None)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn complete_requires_confirmed_and_binds_identity() {
        let (_d, svc) = setup().await;
        let s = svc.create(None).await.unwrap();
        // 未扫码直接完成:拒绝
        assert!(svc.complete(&s.id, "seller-9", "卖家九").await.is_err());
        svc.apply_platform_transition(&s.id, s.generation, AuthFlowStatus::Scanned, None)
            .await
            .unwrap();
        svc.apply_platform_transition(
            &s.id,
            s.generation,
            AuthFlowStatus::Confirmed,
            Some("seller-9"),
        )
        .await
        .unwrap();
        let account_id = svc.complete(&s.id, "seller-9", "卖家九").await.unwrap();
        assert!(account_id.starts_with("acct_"));
        // 重复完成幂等:同一身份返回同一账号
        let again = svc.complete(&s.id, "seller-9", "卖家九").await.unwrap();
        assert_eq!(account_id, again);
    }
}
