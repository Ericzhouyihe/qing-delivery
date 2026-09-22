//! 账号控制用例(T047/T048/T049):
//! 三开关(运行/自动交付/自动平台确认,默认全关)、monitor_since 首次启用写入且不重置、
//! 暂停屏障 = 先持久化 control_epoch 再等待执行侧生效(pausing 期间新 handoff 被拒)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::accounts;
use crate::domain::accounts::{AccountStatus, account_transition};
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("账号不存在")]
    NotFound,
    #[error("首次启用自动交付必须确认监控范围(acknowledge_monitoring_scope=true)")]
    ScopeAcknowledgementRequired,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error("暂停屏障未在期限 内完成")]
    BarrierTimeout,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone)]
pub struct AccountControlService {
    db: DbThread,
}

#[derive(Debug)]
pub struct ControlChange {
    pub account_id: String,
    pub new_status: AccountStatus,
    pub control_epoch: i64,
    pub monitor_since_written: bool,
}

impl AccountControlService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 变更开关:暂停即停运行;恢复要求明确动作。monitor_since 只首次写(FR-003)。
    pub async fn set_switches(
        &self,
        account_id: &str,
        expected_version: i64,
        run_enabled: Option<bool>,
        auto_delivery_enabled: Option<bool>,
        auto_confirm_enabled: Option<bool>,
        acknowledge_monitoring_scope: bool,
    ) -> Result<ControlChange, ControlError> {
        let account_id = account_id.to_string();

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<ControlChange, ControlError>> {
                    let Some(row) = accounts::get(conn, &account_id)? else {
                        return Ok(Err(ControlError::NotFound));
                    };

                    if row.version != expected_version {
                        return Ok(Err(ControlError::VersionConflict));
                    }
                    // 首次启用自动交付必须确认监控范围(告知处理范围并保存生效时间)
                    let first_delivery_enable =
                        auto_delivery_enabled == Some(true) && !row.auto_delivery_enabled;
                    if first_delivery_enable && !acknowledge_monitoring_scope {
                        return Ok(Err(ControlError::ScopeAcknowledgementRequired));
                    }
                    let run = run_enabled.unwrap_or(row.runtime_enabled);
                    let auto_delivery = auto_delivery_enabled.unwrap_or(row.auto_delivery_enabled);
                    let auto_confirm = auto_confirm_enabled.unwrap_or(row.auto_confirm_enabled);
                    // 期望状态决定观测初值:停运行→paused;启运行→connecting(由运行时推进 online)
                    let target = if run {
                        if row.status == "paused" || row.status == "disabled" {
                            AccountStatus::Connecting
                        } else {
                            AccountStatus::parse(&row.status)
                        }
                    } else {
                        AccountStatus::Paused
                    };
                    // 控制路径允许从任意运行状态暂停(FR-023);状态机只约束运行时观测迁移
                    let _ = account_transition;
                    let monitor_since_written =
                        first_delivery_enable && row.monitor_since.is_none();
                    let epoch = row.control_epoch + 1;
                    // 暂停意图先落库 control_epoch:handoff 前核对新代次,屏障完成前 UI 显示 pausing
                    conn.execute(
                        "UPDATE accounts SET runtime_enabled = ?2, auto_delivery_enabled = ?3,
                            auto_confirm_enabled = ?4, status = ?5, control_epoch = ?6,
                            monitor_since = COALESCE(monitor_since, ?7),
                            version = version + 1, updated_at = ?8
                     WHERE id = ?1",
                        rusqlite::params![
                            account_id,
                            run as i64,
                            auto_delivery as i64,
                            auto_confirm as i64,
                            target.as_str(),
                            epoch,
                            if monitor_since_written {
                                Some(utc_now_ms())
                            } else {
                                None
                            },
                            utc_now_ms()
                        ],
                    )?;
                    Ok(Ok(ControlChange {
                        account_id,
                        new_status: target,
                        control_epoch: epoch,
                        monitor_since_written,
                    }))
                },
            )
            .await??
    }

    /// 运行时观测状态更新(连接成功 online、断线 offline、失效、需验证)。
    pub async fn observe_status(
        &self,
        account_id: &str,
        to: AccountStatus,
    ) -> Result<bool, ControlError> {
        let account_id = account_id.to_string();
        let to_str = to.as_str().to_string();
        let updated = self
            .db
            .call(move |conn| -> rusqlite::Result<bool> {
                let row = accounts::get(conn, &account_id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                let from = AccountStatus::parse(&row.status);
                if !account_transition(from, to) {
                    return Ok(false);
                }
                conn.execute(
                    "UPDATE accounts SET status = ?2, version = version + 1, updated_at = ?3
                     WHERE id = ?1",
                    rusqlite::params![account_id, to_str, utc_now_ms()],
                )?;
                Ok(true)
            })
            .await??;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;
    use crate::adapters::sqlite::repos::accounts as accounts_repo;

    async fn setup() -> (tempfile::TempDir, AccountControlService, DbThread) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("ctl.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        db.call(|conn| {
            accounts_repo::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")
        })
        .await
        .unwrap()
        .unwrap();
        let svc = AccountControlService::new(db.clone());
        (dir, svc, db)
    }

    #[tokio::test]
    async fn new_account_defaults_off_and_first_enable_writes_monitor_since() {
        let (_d, svc, db) = setup().await;
        let row = db
            .call(|conn| accounts_repo::get(conn, "acct-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!row.runtime_enabled && !row.auto_delivery_enabled && !row.auto_confirm_enabled);

        // 首次启用自动交付未确认监控范围 → 拒绝
        let err = svc
            .set_switches("acct-1", row.version, None, Some(true), None, false)
            .await
            .unwrap_err();
        assert!(matches!(err, ControlError::ScopeAcknowledgementRequired));

        // 确认后启用:monitor_since 写入,control_epoch 递增
        let change = svc
            .set_switches("acct-1", row.version, None, Some(true), None, true)
            .await
            .unwrap();
        assert!(change.monitor_since_written);
        let row2 = db
            .call(|conn| accounts_repo::get(conn, "acct-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(row2.monitor_since.is_some());
        assert_eq!(row2.control_epoch, row.control_epoch + 1);

        // 再暂停再启用:monitor_since 不重置
        svc.set_switches("acct-1", row2.version, Some(false), None, None, false)
            .await
            .unwrap();
        let row3 = db
            .call(|conn| accounts_repo::get(conn, "acct-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(row3.status, "paused");
        assert_eq!(row3.monitor_since, row2.monitor_since, "暂停不重置监控起点");
    }

    #[tokio::test]
    async fn resume_goes_connecting_and_version_conflict_detected() {
        let (_d, svc, db) = setup().await;
        let row = db
            .call(|conn| accounts_repo::get(conn, "acct-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let change = svc
            .set_switches("acct-1", row.version, Some(true), None, None, false)
            .await
            .unwrap();
        assert_eq!(change.new_status, AccountStatus::Connecting);
        // 旧版本号写入 → 冲突
        let err = svc
            .set_switches("acct-1", row.version, Some(false), None, None, false)
            .await
            .unwrap_err();
        assert!(matches!(err, ControlError::VersionConflict));
    }

    #[tokio::test]
    async fn runtime_observation_follows_state_machine() {
        let (_d, svc, db) = setup().await;
        let row = db
            .call(|conn| accounts_repo::get(conn, "acct-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        svc.set_switches("acct-1", row.version, Some(true), None, None, false)
            .await
            .unwrap();
        assert!(
            svc.observe_status("acct-1", AccountStatus::Online)
                .await
                .unwrap()
        );
        // 验证要求必须经人工:online→needs_verification 合法;needs_verification→online 非法
        assert!(
            svc.observe_status("acct-1", AccountStatus::NeedsVerification)
                .await
                .unwrap()
        );
        assert!(
            !svc.observe_status("acct-1", AccountStatus::Online)
                .await
                .unwrap()
        );
    }
}
