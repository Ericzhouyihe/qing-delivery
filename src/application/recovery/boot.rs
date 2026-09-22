//! 启动恢复(T065):崩溃/退出时处于 dispatching 的尝试,重启后统一 unknown+required。
//! 重启不清除未知状态、不重置重试预算(FR-020);确认轴同样处理。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{deliveries, issues};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct BootReport {
    pub unknown_content: usize,
    pub unknown_confirmations: usize,
}

pub struct BootRecovery {
    db: DbThread,
}

impl BootRecovery {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// serve 启动顺序中的"恢复未决尝试状态"步骤(cli.md)。
    pub async fn run(&self) -> Result<BootReport, BootError> {
        let rows = self
            .db
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT a.id, a.delivery_id, a.action_kind, d.order_id, d.content_state,
                            d.confirmation_state, d.review_state
                     FROM attempts a JOIN deliveries d ON d.id = a.delivery_id
                     WHERE a.state = 'dispatching'",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?, // attempt id
                            r.get::<_, String>(1)?, // delivery id
                            r.get::<_, String>(2)?, // action_kind
                            r.get::<_, String>(3)?, // order id
                            r.get::<_, String>(4)?, // content_state
                            r.get::<_, String>(5)?, // confirmation_state
                            r.get::<_, String>(6)?, // review_state
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await??;

        let mut report = BootReport {
            unknown_content: 0,
            unknown_confirmations: 0,
        };
        for (attempt_id, delivery_id, action_kind, order_id, content, confirmation, review) in rows
        {
            // 无证据的未决尝试一律 unknown;宁可人工核对(data-model 并发 3)
            let reason = match action_kind.as_str() {
                "confirmation" => "平台确认结果未知(重启)",
                _ => "发送结果未知(重启)",
            };
            self.db
                .call({
                    let attempt_id = attempt_id.clone();
                    let delivery_id = delivery_id.clone();
                    let action_kind = action_kind.clone();
                    move |conn| -> rusqlite::Result<()> {
                        deliveries::finish_attempt(
                            conn,
                            &attempt_id,
                            "unknown",
                            Some("restart_pending"),
                            None,
                        )?;
                        let account: String = conn.query_row(
                            "SELECT account_id FROM orders WHERE id = ?1",
                            rusqlite::params![order_id],
                            |r| r.get(0),
                        )?;
                        if action_kind == "confirmation" {
                            conn.execute(
                                "UPDATE deliveries SET confirmation_state = 'unknown',
                                        review_state = 'required', version = version + 1,
                                        updated_at = ?2 WHERE id = ?1",
                                rusqlite::params![delivery_id, utc_now_ms()],
                            )?;
                        } else {
                            // 仅当内容仍处于 dispatching 才降级(可能已被并发结果覆盖)
                            conn.execute(
                                "UPDATE deliveries SET content_state = 'unknown',
                                        review_state = 'required', version = version + 1,
                                        updated_at = ?2
                                 WHERE id = ?1 AND content_state = 'dispatching'",
                                rusqlite::params![delivery_id, utc_now_ms()],
                            )?;
                        }
                        issues::open(
                            conn,
                            &ids::new_id("iss"),
                            Some(&order_id),
                            &account,
                            Some(&delivery_id),
                            "delivery_unknown",
                            reason,
                            "[\"resend\",\"mark_received\",\"terminate\"]",
                        )?;
                        // guard 保持人工解决状态:禁止自动重发
                        conn.execute(
                            "UPDATE order_execution_guards SET state = 'unknown_manual'
                             WHERE order_id = ?1",
                            rusqlite::params![order_id],
                        )?;
                        Ok(())
                    }
                })
                .await??;
            if action_kind == "confirmation" {
                report.unknown_confirmations += 1;
            } else {
                report.unknown_content += 1;
            }
            let _ = (content, confirmation, review);
        }
        Ok(report)
    }
}
