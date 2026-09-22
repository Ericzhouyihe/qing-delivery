//! 入站事件管道(T025):事件信封持久化去重、按序处理同帧全部条目、
//! 付款/状态信号进入订单事实摄取;坏条目记 ProtocolIssue 后继续(§3)。

use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{inbound, orders};
use crate::application::ports::platform::PlatformEvent;
use crate::domain::ids;

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// 单个规范事件的处理结果。
#[derive(Debug, PartialEq)]
pub enum EventOutcome {
    /// 首次见到的稳定事件 ID,已处理
    Processed,
    /// 稳定事件 ID 重复:无新任务(FR-014)
    Duplicate,
    /// 坏条目:记录 ProtocolIssue 后继续,不吞后续有效条目
    BadEntry(String),
    /// 付款候选:订单已建立/更新,等待资格核验(核验由交付用例负责)
    PaymentSignal { order_id: String, created: bool },
    /// 状态信号:是否推进了状态
    StateSignal { order_id: String, advanced: bool },
    /// 非订单事件(授权/运行状态/回显等),由对应模块消费
    Observed,
}

#[derive(Clone)]
pub struct EventPipeline {
    db: DbThread,
}

impl EventPipeline {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 处理一个事件:先去重持久化,再按类型分派。
    /// 同帧多事件由调用方逐个调用;单条失败不阻断其余(§3)。
    pub async fn process(&self, event: &PlatformEvent) -> Result<EventOutcome, EventError> {
        let meta = meta_of(event);
        let digest = digest_of(event);
        let local_id = ids::new_id("evt");
        let account = meta.account_id.clone();
        let source_id = meta.source_event_id.clone();
        let digest_for_record = digest.clone();

        let recorded = self
            .db
            .call(move |conn| {
                inbound::record_event(
                    conn,
                    &local_id,
                    &account,
                    source_id.as_deref(),
                    &digest_for_record,
                )
            })
            .await??;
        let Some(_row) = recorded else {
            return Ok(EventOutcome::Duplicate);
        };

        let outcome: Result<EventOutcome, EventError> = match event {
            PlatformEvent::OrderPaymentSignal { order_id, .. } => {
                self.ingest_order_signal(meta.account_id.clone(), order_id.clone())
                    .await
            }
            PlatformEvent::OrderStateSignal {
                order_id,
                platform_state,
                ..
            } => {
                let order_id = order_id.clone();
                let new_state = platform_state.clone();
                let account = meta.account_id.clone();
                let advanced = self
                    .db
                    .call(move |conn| {
                        let row = orders::find_by_external(conn, "xianyu", &account, &order_id)?;
                        match row {
                            Some(existing) => {
                                orders::apply_state_signal(conn, &existing.id, &new_state)
                                    .map(|a| (Some(existing.id), a))
                            }
                            None => Ok((None, false)),
                        }
                    })
                    .await??;
                match advanced {
                    (Some(order_db_id), advanced) => {
                        let fact_id = ids::new_id("fct");
                        let ord_for_fact = order_db_id.clone();
                        let ord_for_result = order_db_id.clone();
                        let source_event = meta.source_event_id.clone();
                        let received_at = meta.received_at_ms;
                        let evidence = format!("{{\"platform_state\":\"{platform_state}\"}}");
                        let digest_owned = digest.clone();
                        self.db
                            .call(move |conn| {
                                orders::append_fact(
                                    conn,
                                    &orders::NewOrderFact {
                                        id: &fact_id,
                                        order_id: &ord_for_fact,
                                        source: "order_state_signal",
                                        source_event_id: source_event.as_deref(),
                                        observed_at: received_at,
                                        normalized_evidence: &evidence,
                                        evidence_digest: &digest_owned,
                                    },
                                )
                            })
                            .await??;
                        Ok(EventOutcome::StateSignal {
                            order_id: ord_for_result,
                            advanced,
                        })
                    }
                    (None, _) => Ok(EventOutcome::Observed),
                }
            }
            PlatformEvent::OutgoingMessageEvidence { .. } => Ok(EventOutcome::Observed),
            PlatformEvent::AuthorizationChanged { .. }
            | PlatformEvent::AccountRuntimeChanged { .. } => Ok(EventOutcome::Observed),
            PlatformEvent::TraceGap { reason, .. }
            | PlatformEvent::ProtocolIssue { reason, .. } => {
                Ok(EventOutcome::BadEntry(reason.clone()))
            }
        };
        outcome
    }

    async fn ingest_order_signal(
        &self,
        account_id: String,
        external_order_id: String,
    ) -> Result<EventOutcome, EventError> {
        let order_db_id = ids::new_id("ord");
        let account = account_id.clone();
        let external = external_order_id.clone();
        let created = self
            .db
            .call(move |conn| {
                let (row, created) =
                    orders::find_or_create(conn, &order_db_id, "xianyu", &account, &external)?;
                let fact_id = crate::domain::ids::new_id("fct");
                orders::append_fact(
                    conn,
                    &orders::NewOrderFact {
                        id: &fact_id,
                        order_id: &row.id,
                        source: "payment_signal",
                        source_event_id: None,
                        observed_at: crate::domain::time_util::utc_now_ms(),
                        normalized_evidence: &format!(
                            "{{\"signal\":\"payment_signal\",\"order\":\"{external}\"}}"
                        ),
                        evidence_digest: "",
                    },
                )?;
                Ok::<_, rusqlite::Error>((row.id, created))
            })
            .await??;
        Ok(EventOutcome::PaymentSignal {
            order_id: created.0,
            created: created.1,
        })
    }
}

fn meta_of(event: &PlatformEvent) -> &crate::application::ports::platform::EventMeta {
    match event {
        PlatformEvent::AuthorizationChanged { meta, .. }
        | PlatformEvent::AccountRuntimeChanged { meta, .. }
        | PlatformEvent::OrderPaymentSignal { meta, .. }
        | PlatformEvent::OrderStateSignal { meta, .. }
        | PlatformEvent::OutgoingMessageEvidence { meta, .. }
        | PlatformEvent::TraceGap { meta, .. }
        | PlatformEvent::ProtocolIssue { meta, .. } => meta,
    }
}

fn digest_of(event: &PlatformEvent) -> String {
    let repr = format!("{event:?}");
    hex::encode(Sha256::digest(repr.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;
    use crate::application::ports::platform::EventMeta;

    fn meta(account: &str, event_id: Option<&str>) -> EventMeta {
        EventMeta {
            account_id: account.into(),
            credential_generation: 0,
            source_event_id: event_id.map(|s| s.to_string()),
            platform_event_at_ms: None,
            received_at_ms: 1_760_000_000_000,
        }
    }

    fn payment(order: &str, event_id: &str) -> PlatformEvent {
        PlatformEvent::OrderPaymentSignal {
            meta: meta("acct-1", Some(event_id)),
            order_id: order.into(),
            buyer_hint: Some("buyer-a".into()),
        }
    }

    async fn setup() -> (tempfile::TempDir, EventPipeline) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("events.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        // orders.account_id 外键约束:先建账号
        db.call(|conn| {
            crate::adapters::sqlite::repos::accounts::upsert_identity(
                conn,
                "acct-1",
                "xianyu",
                "seller-1",
                "测试卖家",
            )
        })
        .await
        .unwrap()
        .unwrap();
        (dir, EventPipeline::new(db))
    }

    #[tokio::test]
    async fn duplicate_source_events_are_ignored() {
        let (_d, p) = setup().await;
        let first = p.process(&payment("ORD-1", "evt-1")).await.unwrap();
        assert!(matches!(
            first,
            EventOutcome::PaymentSignal { created: true, .. }
        ));
        let second = p.process(&payment("ORD-1", "evt-1")).await.unwrap();
        assert_eq!(second, EventOutcome::Duplicate);
    }

    #[tokio::test]
    async fn terminal_state_not_regressed_by_stale_signal() {
        let (_d, p) = setup().await;
        p.process(&payment("ORD-2", "evt-2")).await.unwrap();
        // 退款(终态)先到
        let refunded = PlatformEvent::OrderStateSignal {
            meta: meta("acct-1", Some("evt-2-r")),
            order_id: "ORD-2".into(),
            platform_state: "refunded".into(),
        };
        assert!(matches!(
            p.process(&refunded).await.unwrap(),
            EventOutcome::StateSignal { advanced: true, .. }
        ));
        // 旧的待发货信号不能倒退:事实仍记录,但状态不推进
        let stale = PlatformEvent::OrderStateSignal {
            meta: meta("acct-1", Some("evt-2-s")),
            order_id: "ORD-2".into(),
            platform_state: "pending_ship".into(),
        };
        let out = p.process(&stale).await.unwrap();
        assert!(
            matches!(
                out,
                EventOutcome::StateSignal {
                    advanced: false,
                    ..
                }
            ),
            "旧状态信号不得推进状态,实际:{out:?}"
        );
    }

    #[tokio::test]
    async fn bad_entry_does_not_block() {
        let (_d, p) = setup().await;
        let gap = PlatformEvent::ProtocolIssue {
            meta: meta("acct-1", Some("evt-gap")),
            reason: "decode failed".into(),
        };
        assert!(matches!(
            p.process(&gap).await.unwrap(),
            EventOutcome::BadEntry(_)
        ));
        // 后续有效事件继续处理
        let ok = p.process(&payment("ORD-3", "evt-3")).await.unwrap();
        assert!(matches!(ok, EventOutcome::PaymentSignal { .. }));
    }
}
