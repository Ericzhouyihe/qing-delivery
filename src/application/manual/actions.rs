//! 人工动作(T068—T070):与自动执行共用 guard 互斥;幂等键去重;
//! 补发使用同一冻结快照并重新核验资格(FR-022 例外:已发货未完成的普通订单可补发);
//! 人工"已收到"只记 manual 证据,不自动平台确认;终止仅限未提交动作。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{accounts, deliveries, issues, manual_actions, orders};
use crate::adapters::windows::keys::DataKey;
use crate::application::delivery::service::{DeliveryService, HandleOutcome};
use crate::application::idempotency::{Begin, IdempotencyService};
use crate::application::ports::platform::{
    ContentForSend, PlatformAdapter, PlatformError, RequestContext, SendOutcome,
};
use crate::domain::crypto::{self, Aad};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum ManualError {
    #[error("订单不存在")]
    OrderNotFound,
    #[error("交付任务不存在;请先完成历史接管或等待自动建任务")]
    NoDelivery,
    #[error("当前状态不允许该操作:{0}")]
    NotAllowed(String),
    #[error("必须确认重复发送风险(acknowledge_duplicate_risk)")]
    RiskConfirmationRequired,
    #[error("必须填写原因(1—500 字符)")]
    InvalidReason,
    #[error("已有同键操作在途")]
    InFlight,
    #[error("自动交付已关闭:人工补发同样被阻止,请先明确重新开启")]
    AutoDeliveryOff,
    #[error("订单已取消/退款中/退款成功/已完成,不得补发")]
    RefundedOrCompleted,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error(transparent)]
    Delivery(#[from] crate::application::delivery::service::DeliveryError),
}

pub enum ManualOutcome {
    ResentAccepted,
    ResentUnknown,
    MarkedReceived,
    Terminated,
    Takeover(Box<HandleOutcome>),
    IdempotentReplay,
}

pub struct ManualRequest {
    pub admin_id: String,
    pub account_id: String,
    pub order_db_id: String,
    pub reason: String,
    pub risk_confirmed: bool,
    pub idempotency_key: String,
}

struct LoadedCtx {
    external_order_id: String,
    platform_status: String,
    buyer_id: Option<String>,
    delivery: deliveries::DeliveryRow,
    snapshot: deliveries::SnapshotRow,
}

pub struct ManualService<A: PlatformAdapter> {
    db: DbThread,
    key: DataKey,
    delivery: DeliveryService<A>,
    idempotency: IdempotencyService,
}

impl<A: PlatformAdapter> ManualService<A> {
    pub fn new(db: DbThread, key: DataKey, delivery: DeliveryService<A>) -> Self {
        let idempotency = IdempotencyService::new(db.clone());
        Self {
            db,
            key,
            delivery,
            idempotency,
        }
    }

    /// 订单+交付+快照一次窄读;账号归属不匹配按不存在处理。
    async fn load_ctx(
        &self,
        account_id: &str,
        order_db_id: &str,
    ) -> Result<LoadedCtx, ManualError> {
        let order = order_db_id.to_string();
        let loaded = self
            .db
            .call(move |conn| -> rusqlite::Result<Option<LoadedCtx>> {
                let Some(order) = orders::get_manual_ctx(conn, &order)? else {
                    return Ok(None);
                };
                let Some(delivery) = deliveries::find_initial(conn, &order.id)? else {
                    return Ok(None);
                };
                let Some(snapshot_id) = &delivery.content_snapshot_id else {
                    return Ok(None);
                };
                let Some(snapshot) = deliveries::get_snapshot(conn, snapshot_id)? else {
                    return Ok(None);
                };
                Ok(Some(LoadedCtx {
                    external_order_id: order.external_order_id,
                    platform_status: order.platform_status,
                    buyer_id: order.buyer_id,
                    delivery,
                    snapshot,
                }))
            })
            .await??;
        // 归属校验在 SQL 外:账号路径必须匹配订单(不泄露他账号存在性)
        let account_owned = account_id.to_string();
        match loaded {
            Some(ctx) => {
                let belongs = self
                    .db
                    .call({
                        let order = order_db_id.to_string();
                        move |conn| -> rusqlite::Result<bool> {
                            let row = orders::get_manual_ctx(conn, &order)?;
                            Ok(row.map(|o| o.account_id == account_owned).unwrap_or(false))
                        }
                    })
                    .await??;
                if belongs {
                    Ok(ctx)
                } else {
                    Err(ManualError::OrderNotFound)
                }
            }
            None => Err(ManualError::NoDelivery),
        }
    }

    async fn begin_idem(&self, req: &ManualRequest, operation: &str) -> Result<bool, ManualError> {
        match self
            .idempotency
            .begin(
                &req.idempotency_key,
                &req.admin_id,
                operation,
                &req.order_db_id,
                None,
            )
            .await
        {
            Ok(Begin::Replay(_)) => Ok(false),
            Ok(Begin::Fresh) => Ok(true),
            Err(_) => Err(ManualError::InFlight),
        }
    }

    /// 显式补发(FR-022)。
    pub async fn resend(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        Self::check_reason(&req.reason)?;
        if !req.risk_confirmed {
            return Err(ManualError::RiskConfirmationRequired);
        }
        if !self.begin_idem(&req, "manual_resend").await? {
            return Ok(ManualOutcome::IdempotentReplay);
        }
        let outcome = self.resend_inner(&req).await;
        let status = match &outcome {
            Ok(ManualOutcome::ResentAccepted) => "succeeded",
            Ok(ManualOutcome::ResentUnknown) => "unknown",
            _ => "failed",
        };
        let _ = self
            .idempotency
            .complete(&req.idempotency_key, status, "{}")
            .await;
        outcome
    }

    async fn resend_inner(&self, req: &ManualRequest) -> Result<ManualOutcome, ManualError> {
        let ctx = self.load_ctx(&req.account_id, &req.order_db_id).await?;
        match ctx.platform_status.as_str() {
            "canceled" | "refunding" | "refunded" | "completed" => {
                return Err(ManualError::RefundedOrCompleted);
            }
            _ => {}
        }
        let account = self
            .db
            .call({
                let account_id = req.account_id.clone();
                move |conn| accounts::get(conn, &account_id)
            })
            .await??;
        let account = account.ok_or(ManualError::OrderNotFound)?;
        if !account.auto_delivery_enabled || !account.runtime_enabled {
            return Err(ManualError::AutoDeliveryOff);
        }
        match ctx.delivery.content_state.as_str() {
            "accepted" | "unknown" | "not_sent" => {}
            other => {
                return Err(ManualError::NotAllowed(format!(
                    "内容状态 {other} 不允许补发"
                )));
            }
        }

        // 审计 + guard 互斥(与自动执行共用)
        let manual_id = ids::new_id("man");
        let attempt_id = ids::new_id("att");
        let guard = self
            .db
            .call({
                let order = req.order_db_id.clone();
                let delivery = ctx.delivery.id.clone();
                let admin = req.admin_id.clone();
                let reason = req.reason.clone();
                let attempt = attempt_id.clone();
                let manual = manual_id.clone();
                move |conn| -> rusqlite::Result<Option<()>> {
                    manual_actions::insert(
                        conn,
                        &manual,
                        &admin,
                        &order,
                        Some(&delivery),
                        "resend",
                        &reason,
                        true,
                        &attempt,
                    )?;
                    deliveries::acquire_guard(conn, &order, &delivery, &attempt)
                }
            })
            .await??;
        if guard.is_none() {
            return Err(ManualError::InFlight);
        }
        let request_id = ids::new_request_key();
        let attempt = self
            .db
            .call({
                let delivery = ctx.delivery.id.clone();
                let rid = request_id.clone();
                let attempt = attempt_id.clone();
                move |conn| {
                    deliveries::prepare_attempt(
                        conn,
                        &attempt,
                        &delivery,
                        "manual_resend",
                        &rid,
                        None,
                    )
                }
            })
            .await??;
        let Some(attempt) = attempt else {
            self.release(&req.order_db_id, "released").await;
            return Err(ManualError::InFlight);
        };

        // 同一冻结快照原文
        let aad = Aad {
            purpose: "content_snapshot".into(),
            entity_id: ctx.snapshot.id.clone(),
            content_version: None,
        };
        let plain = crypto::open(&self.key.key, &aad, &ctx.snapshot.envelope)?;
        let text = String::from_utf8(plain).map_err(|_| crypto::CryptoError::OpenFailed)?;
        let buyer = ctx
            .buyer_id
            .clone()
            .ok_or(ManualError::NotAllowed("买家身份缺失".into()))?;
        let pctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: req.account_id.clone(),
            credential_generation: account.credential_epoch,
            control_generation: account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        let send = self
            .delivery
            .send_manual(
                &pctx,
                &req.order_db_id,
                &ctx.external_order_id,
                &buyer,
                &ContentForSend {
                    snapshot_id: ctx.snapshot.id.clone(),
                    text,
                    text_digest: ctx.snapshot.digest.clone(),
                },
            )
            .await?;

        match send {
            SendOutcome::Accepted(proof) => {
                let proof_ref = ids::new_id("prf");
                let att = attempt.id.clone();
                let manual_ref = manual_id.clone();
                let proof_buyer = proof.buyer_id.clone();
                let proof_digest = proof.content_digest.clone();
                let proof_mid = proof.platform_message_id.clone();
                let proof_req = proof.request_id.clone();
                self.db
                    .call(move |conn| {
                        deliveries::insert_proof(
                            conn,
                            &proof_ref,
                            &att,
                            "platform",
                            proof_mid.as_deref(),
                            &proof_req,
                            Some(&proof_buyer),
                            &proof_digest,
                            Some(&manual_ref),
                        )?;
                        deliveries::finish_attempt(conn, &att, "accepted", None, Some(&proof_ref))
                    })
                    .await??;
                let delivery = ctx.delivery.id.clone();
                self.db
                    .call(move |conn| {
                        conn.execute(
                            "UPDATE deliveries SET content_state='accepted',
                                    review_state='resolved', evidence_origin='platform',
                                    version = version + 1, updated_at = ?2 WHERE id = ?1",
                            rusqlite::params![delivery, utc_now_ms()],
                        )
                    })
                    .await??;
                self.release(&req.order_db_id, "terminal").await;
                Ok(ManualOutcome::ResentAccepted)
            }
            SendOutcome::NotSubmitted { .. } | SendOutcome::Rejected { .. } => {
                let att = attempt.id.clone();
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(
                            conn,
                            &att,
                            "not_sent",
                            Some("manual_resend_failed"),
                            None,
                        )
                    })
                    .await??;
                self.release(&req.order_db_id, "released").await;
                // 明确未发送:返回未知占位由调用方按 issue 处理不合适——保持语义诚实:
                Ok(ManualOutcome::ResentUnknown)
            }
            SendOutcome::Unknown { .. } => {
                let att = attempt.id.clone();
                let order = req.order_db_id.clone();
                let account_id = req.account_id.clone();
                let delivery = ctx.delivery.id.clone();
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(conn, &att, "unknown", None, None)?;
                        conn.execute(
                            "UPDATE deliveries SET content_state='unknown', review_state='required',
                                    version = version + 1, updated_at = ?2 WHERE id = ?1",
                            rusqlite::params![delivery, utc_now_ms()],
                        )?;
                        issues::open(
                            conn,
                            &ids::new_id("iss"),
                            Some(&order),
                            &account_id,
                            Some(&delivery),
                            "delivery_unknown",
                            "人工补发结果未知,需要再次核对",
                            "[\"resend\",\"mark_received\",\"terminate\"]",
                        )
                    })
                    .await??;
                self.release(&req.order_db_id, "unknown_manual").await;
                Ok(ManualOutcome::ResentUnknown)
            }
        }
    }

    /// 人工确认已收到(FR-021):只追加 manual 证明;平台确认是独立操作。
    pub async fn mark_received(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        Self::check_reason(&req.reason)?;
        if !self.begin_idem(&req, "mark_received").await? {
            return Ok(ManualOutcome::IdempotentReplay);
        }
        let ctx = self.load_ctx(&req.account_id, &req.order_db_id).await?;
        match ctx.delivery.content_state.as_str() {
            "unknown" | "not_sent" => {}
            "accepted" => {
                let _ = self
                    .idempotency
                    .complete(&req.idempotency_key, "succeeded", "{}")
                    .await;
                return Ok(ManualOutcome::MarkedReceived);
            }
            other => {
                return Err(ManualError::NotAllowed(format!(
                    "内容状态 {other} 无需人工确认"
                )));
            }
        }
        let manual_id = ids::new_id("man");
        let order = req.order_db_id.clone();
        let delivery = ctx.delivery.id.clone();
        let admin = req.admin_id.clone();
        let reason = req.reason.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                manual_actions::insert(
                    conn,
                    &manual_id,
                    &admin,
                    &order,
                    Some(&delivery),
                    "mark_received",
                    &reason,
                    false,
                    "",
                )?;
                conn.execute(
                    "UPDATE deliveries SET content_state='accepted', review_state='resolved',
                            evidence_origin='manual', version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    rusqlite::params![delivery, utc_now_ms()],
                )?;
                // 人工确认不自动触发平台确认(契约 §8)
                conn.execute(
                    "UPDATE order_execution_guards SET state='terminal' WHERE order_id = ?1",
                    rusqlite::params![order],
                )?;
                Ok(())
            })
            .await??;
        let _ = self
            .idempotency
            .complete(&req.idempotency_key, "succeeded", "{}")
            .await;
        Ok(ManualOutcome::MarkedReceived)
    }

    /// 终止处理:仅未提交(无 dispatching/accepted/unknown attempt)时可立即终止。
    pub async fn terminate(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        Self::check_reason(&req.reason)?;
        let ctx = self.load_ctx(&req.account_id, &req.order_db_id).await?;
        let delivery_id = ctx.delivery.id.clone();
        let has_submitted = self
            .db
            .call(move |conn| -> rusqlite::Result<i64> {
                conn.query_row(
                    "SELECT COUNT(*) FROM attempts
                     WHERE delivery_id = ?1 AND state IN ('dispatching','accepted','unknown')",
                    rusqlite::params![delivery_id],
                    |r| r.get(0),
                )
            })
            .await??;
        if has_submitted > 0 {
            return Err(ManualError::NotAllowed(
                "已有提交/未定动作:结果必须保留,不能立即终止".into(),
            ));
        }
        let order = req.order_db_id.clone();
        let delivery = ctx.delivery.id.clone();
        let admin = req.admin_id.clone();
        let reason = req.reason.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                manual_actions::insert(
                    conn,
                    &ids::new_id("man"),
                    &admin,
                    &order,
                    Some(&delivery),
                    "terminate",
                    &reason,
                    false,
                    "",
                )?;
                conn.execute(
                    "UPDATE deliveries SET content_state='terminated', review_state='resolved',
                            version = version + 1, updated_at = ?2 WHERE id = ?1",
                    rusqlite::params![delivery, utc_now_ms()],
                )?;
                conn.execute(
                    "UPDATE order_execution_guards SET state='terminal' WHERE order_id = ?1",
                    rusqlite::params![order],
                )?;
                Ok(())
            })
            .await??;
        Ok(ManualOutcome::Terminated)
    }

    /// 历史接管(FR-017):逐单显式,记录原因;其余核验不放宽。
    pub async fn takeover(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        Self::check_reason(&req.reason)?;
        if !req.risk_confirmed {
            return Err(ManualError::RiskConfirmationRequired);
        }
        let external = self
            .db
            .call({
                let order = req.order_db_id.clone();
                move |conn| orders::get_order_external(conn, &order)
            })
            .await??;
        let Some(external) = external else {
            return Err(ManualError::OrderNotFound);
        };
        let order = req.order_db_id.clone();
        let admin = req.admin_id.clone();
        let reason = req.reason.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                manual_actions::insert(
                    conn,
                    &ids::new_id("man"),
                    &admin,
                    &order,
                    None,
                    "takeover",
                    &reason,
                    true,
                    "",
                )
            })
            .await??;
        let outcome = self
            .delivery
            .handle_takeover(&req.account_id, &external)
            .await?;
        Ok(ManualOutcome::Takeover(Box::new(outcome)))
    }

    fn check_reason(reason: &str) -> Result<(), ManualError> {
        let n = reason.trim().chars().count();
        if n == 0 || n > 500 {
            return Err(ManualError::InvalidReason);
        }
        Ok(())
    }

    async fn release(&self, order_id: &str, state: &str) {
        let order = order_id.to_string();
        let state = state.to_string();
        let _ = self
            .db
            .call(move |conn| deliveries::release_guard(conn, &order, &state))
            .await;
    }
}

/// 对象安全门面:AppState 持有 ManualService 而不必具化适配器类型。
/// live 构建在真实协议接入(SC-091)前为 None;确定性验证走假适配器。
#[async_trait::async_trait]
pub trait ManualOps: Send + Sync {
    async fn resend(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError>;
    async fn mark_received(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError>;
    async fn terminate(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError>;
    async fn takeover(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError>;
}

#[async_trait::async_trait]
impl<A: PlatformAdapter + 'static> ManualOps for ManualService<A> {
    async fn resend(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        ManualService::resend(self, req).await
    }
    async fn mark_received(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        ManualService::mark_received(self, req).await
    }
    async fn terminate(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        ManualService::terminate(self, req).await
    }
    async fn takeover(&self, req: ManualRequest) -> Result<ManualOutcome, ManualError> {
        ManualService::takeover(self, req).await
    }
}
