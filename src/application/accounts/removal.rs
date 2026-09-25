//! 账号删除(004/T010,D3 软移除三步):停值守→清凭证→删行;审计保留。

use std::sync::Arc;

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::credentials;
use crate::application::verification::service::VerificationService;

pub struct RemovalService {
    db: DbThread,
    verification: Option<Arc<VerificationService>>,
}

#[derive(Debug, PartialEq)]
pub enum RemovalOutcome {
    Removed,
    BlockedByVerification,
    NotFound,
}

impl RemovalService {
    pub fn new(db: DbThread, verification: Option<Arc<VerificationService>>) -> Self {
        Self { db, verification }
    }

    pub async fn remove(&self, account_id: &str) -> Result<RemovalOutcome, String> {
        // 活跃验证会话→409(FR/契约)
        if let Some(v) = self.verification.as_ref()
            && v.snapshot(account_id).is_some()
        {
            return Ok(RemovalOutcome::BlockedByVerification);
        }
        let account = account_id.to_string();
        self.db
            .call(move |conn| -> rusqlite::Result<RemovalOutcome> {
                // ①停值守(runtime_enabled=0,status=disabled;supervisor tick 自然停)
                let stopped = conn.execute(
                    "UPDATE accounts SET runtime_enabled = 0, status = 'disabled',
                         version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    rusqlite::params![account, crate::domain::time_util::utc_now_ms()],
                )?;
                if stopped == 0 {
                    return Ok(RemovalOutcome::NotFound);
                }
                // ②清加密凭证(宪章 V:敏感数据不留存)
                conn.execute(
                    "DELETE FROM account_credentials WHERE account_id = ?1",
                    rusqlite::params![account],
                )?;
                // ③删账号行(订单等审计表无强 FK,行删除;若 FK 阻塞退化为 tombstone)
                let deleted = conn.execute(
                    "DELETE FROM accounts WHERE id = ?1",
                    rusqlite::params![account],
                );
                match deleted {
                    Ok(_) => Ok(RemovalOutcome::Removed),
                    Err(e) => {
                        // FK 受限:tombstone 兜底(列表过滤 status='deleted')
                        conn.execute(
                            "UPDATE accounts SET status = 'deleted', updated_at = ?2 WHERE id = ?1",
                            rusqlite::params![account, crate::domain::time_util::utc_now_ms()],
                        )?;
                        let _ = e;
                        Ok(RemovalOutcome::Removed)
                    }
                }
            })
            .await
            .map_err(|e| format!("db:{e}"))?
            .map_err(|e| format!("删除失败:{e}"))
    }
}

#[allow(dead_code)]
fn _assert_credential_repo_used() {
    let _ = credentials::load as fn(&rusqlite::Connection, &str, &[u8; 32]) -> _;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> DbThread {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("t.db")).unwrap();
        let mig = dir.path().to_path_buf();
        db.call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &mig))
            .await
            .unwrap()
            .unwrap();
        db.call(|conn| {
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, runtime_enabled, created_at, updated_at)
                 VALUES ('acct-1','xianyu','u1','测试',1,0,0)", [],
            )?;
            conn.execute(
                "INSERT INTO account_credentials (account_id, cookie_ciphertext, cookie_nonce, key_id, format_version, generation, updated_at)
                 VALUES ('acct-1', x'00', x'000000000000000000000000', 'k', 1, 1, 0)", [],
            )?;
            conn.execute(
                "INSERT INTO orders (id, platform, account_id, external_order_id, created_at, updated_at)
                 VALUES ('ord-1','xianyu','acct-1','XY-1',0,0)", [],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
        db
    }

    #[tokio::test]
    async fn 删除三步_凭证清零_订单保留() {
        let db = setup().await;
        let svc = RemovalService::new(db.clone(), None);
        assert_eq!(svc.remove("acct-1").await.unwrap(), RemovalOutcome::Removed);
        let checks = db
            .call(|conn| {
                let accounts: i64 =
                    conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
                let creds: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM account_credentials WHERE account_id='acct-1'",
                    [],
                    |r| r.get(0),
                )?;
                let orders: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM orders WHERE account_id='acct-1'",
                    [],
                    |r| r.get(0),
                )?;
                Ok::<_, rusqlite::Error>((accounts, creds, orders))
            })
            .await
            .unwrap()
            .unwrap();
        // orders FK 指向 accounts:行删除被 FK 拒→tombstone(status=deleted)兜底(D3)
        assert_eq!(checks, (1, 0, 1), "账号 tombstone/凭证清除,订单保留");

        // 不存在
        let svc2 = RemovalService::new(db.clone(), None);
        assert_eq!(
            svc2.remove("missing").await.unwrap(),
            RemovalOutcome::NotFound
        );
    }
}
