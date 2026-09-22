//! 幂等键用例:同键同参返回原结果,同键异参 409 冲突(http-api §1)。
//! command_receipts 随交付历史保留,不自动清理(FR-027)。

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum IdempotencyError {
    #[error("幂等键已用于不同请求参数")]
    Conflict,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Receipt {
    pub status: String,
    pub result_json: Option<String>,
}

pub enum Begin {
    /// 全新键:调用方执行操作后必须调用 complete。
    Fresh,
    /// 同键同参重放:直接返回原结果。
    Replay(Receipt),
}

fn request_hash(operation: &str, body_json: &str) -> String {
    hex::encode(Sha256::digest(
        format!("{operation}\u{1}{body_json}").as_bytes(),
    ))
}

/// (operation, request_hash, status, result_json)
type ReceiptRow = (String, String, String, Option<String>);

fn find(conn: &Connection, key: &str) -> rusqlite::Result<Option<ReceiptRow>> {
    conn.query_row(
        "SELECT operation, request_hash, status, result_json FROM command_receipts WHERE idempotency_key = ?1",
        params![key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
}

#[derive(Clone)]
pub struct IdempotencyService {
    db: DbThread,
}

impl IdempotencyService {
    pub fn new(db: DbThread) -> Self {
        Self {
            db: DbThread::clone(&db),
        }
    }

    /// 开始一次幂等操作;并发同键由主键约束串行化。
    pub async fn begin(
        &self,
        key: &str,
        actor_id: &str,
        operation: &str,
        body_json: &str,
        resource_id: Option<&str>,
    ) -> Result<Begin, IdempotencyError> {
        let hash = request_hash(operation, body_json);
        let key_owned = key.to_string();
        let actor = actor_id.to_string();
        let op = operation.to_string();
        let resource = resource_id.map(|s| s.to_string());
        let outcome = self
            .db
            .call(move |conn| -> rusqlite::Result<(bool, Option<Receipt>)> {
                if let Some((_, existing_hash, status, result_json)) = find(conn, &key_owned)? {
                    return Ok((
                        existing_hash != hash,
                        Some(Receipt { status, result_json }),
                    ));
                }
                conn.execute(
                    "INSERT INTO command_receipts(idempotency_key, actor_id, operation, request_hash,
                         resource_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                    params![key_owned, actor, op, hash, resource, utc_now_ms()],
                )?;
                Ok((false, None))
            })
            .await??;
        match outcome {
            (true, _) => Err(IdempotencyError::Conflict),
            (false, Some(receipt)) => Ok(Begin::Replay(receipt)),
            (false, None) => Ok(Begin::Fresh),
        }
    }

    /// 完成幂等操作,保存最终结果供重放。
    pub async fn complete(
        &self,
        key: &str,
        status: &str,
        result_json: &str,
    ) -> Result<(), IdempotencyError> {
        let key_owned = key.to_string();
        let status = status.to_string();
        let result = result_json.to_string();
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                conn.execute(
                    "UPDATE command_receipts SET status = ?1, result_json = ?2 WHERE idempotency_key = ?3",
                    params![status, result, key_owned],
                )?;
                Ok(())
            })
            .await??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (tempfile::TempDir, IdempotencyService) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("idem.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        (dir, IdempotencyService::new(db))
    }

    #[tokio::test]
    async fn same_key_same_body_replays_and_different_body_conflicts() {
        let (_dir, svc) = setup().await;
        match svc
            .begin("k1", "adm", "resend", "{\"a\":1}", None)
            .await
            .unwrap()
        {
            Begin::Fresh => {}
            _ => panic!("首次应为 Fresh"),
        }
        svc.complete("k1", "succeeded", "{\"job\":\"j1\"}")
            .await
            .unwrap();
        match svc
            .begin("k1", "adm", "resend", "{\"a\":1}", None)
            .await
            .unwrap()
        {
            Begin::Replay(r) => {
                assert_eq!(r.status, "succeeded");
                assert_eq!(r.result_json.as_deref(), Some("{\"job\":\"j1\"}"));
            }
            Begin::Fresh => panic!("同键同参应重放"),
        }
        assert!(matches!(
            svc.begin("k1", "adm", "resend", "{\"a\":2}", None).await,
            Err(IdempotencyError::Conflict)
        ));
    }
}
