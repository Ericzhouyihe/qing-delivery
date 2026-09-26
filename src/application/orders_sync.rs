//! 一键订单同步(007 US6,T068):把交付追溯包装为后台任务——逐启用账号
//! 拉取已售列表、逐单走既有核验/交付管线;在账号边界兑现取消意图。
//! 报告 created/restored/reassigned/ineligible/failed 全字段,覆盖缺口如实标注,
//! 不宣称无漏单(宪章 I:扫描不是发货指令,执行前逐单重新核验)。

use rusqlite::OptionalExtension;

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::jobs as jobs_repo;
use crate::adapters::sqlite::repos::orders;
use crate::adapters::windows::keys::DataKey;
use crate::application::delivery::service::{DeliveryService, HandleOutcome};
use crate::application::jobs::{JobRow, JobService};
use crate::application::ports::platform::{PlatformAdapter, RequestContext};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

/// 追溯分页上限(与 recovery/trace 相同防线)。
pub const MAX_TRACE_PAGES: u32 = 100;
pub const DEFAULT_WINDOW_DAYS: i64 = 7;

#[derive(Debug, thiserror::Error)]
pub enum OrderSyncError {
    #[error("账号不可用或未启用")]
    AccountUnavailable,
    #[error("订单不存在或不属于该账号")]
    NotFound,
    #[error("任务创建失败")]
    JobCreate,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Delivery(#[from] crate::application::delivery::service::DeliveryError),
}

/// 单笔同步结果(镜像 HandleOutcome 语义,供传输层直译)。
#[derive(Debug)]
pub enum SingleSyncOutcome {
    Delivered { delivery_id: String },
    Ineligible { delivery_id: String, reason: String },
    NotSent {
        delivery_id: String,
        retry_scheduled: bool,
        reason: Option<String>,
    },
    Unknown { delivery_id: String },
    AlreadyHandled,
    Busy,
}

pub struct OrderSyncService<A: PlatformAdapter> {
    db: DbThread,
    key: DataKey,
    adapter: A,
}

/// 传输层对象安全出口(mock/live 同一构造;无协议构建为 None 时端点如实报不支持)。
#[async_trait::async_trait]
pub trait OrderSyncOps: Send + Sync {
    async fn start(
        &self,
        account_ids: Vec<String>,
        window_days: i64,
    ) -> Result<JobRow, OrderSyncError>;
    async fn sync_single(
        &self,
        account_id: &str,
        order_db_id: &str,
    ) -> Result<SingleSyncOutcome, OrderSyncError>;
}

struct AccountRef {
    id: String,
    status: String,
    runtime_enabled: bool,
    control_epoch: i64,
    credential_epoch: i64,
}

impl<A: PlatformAdapter> OrderSyncService<A> {
    pub fn new(db: DbThread, key: DataKey, adapter: A) -> Self {
        Self { db, key, adapter }
    }

    fn delivery(&self) -> DeliveryService<A>
    where
        A: Clone,
    {
        DeliveryService::new(self.db.clone(), self.key.clone(), self.adapter.clone())
    }

    /// 启动一键同步任务:创建 job(202 语义)后后台执行,立即返回任务句柄。
    /// account_ids 为空 = 覆盖全部启用账号。
    pub async fn start(
        &self,
        account_ids: Vec<String>,
        window_days: i64,
    ) -> Result<JobRow, OrderSyncError>
    where
        A: Clone + Send + 'static,
    {
        let jobs = JobService::new(self.db.clone());
        let job = jobs.create("order_sync", None).await.map_err(|_| OrderSyncError::JobCreate)?;
        let db = self.db.clone();
        let key = clone_key(&self.key);
        let adapter = self.adapter.clone();
        let job_id = job.id.clone();
        let job_version = job.version;
        let window_days = window_days.clamp(1, 92);
        let explicit_ids = if account_ids.is_empty() { None } else { Some(account_ids) };
        tokio::spawn(async move {
            run_sync_job(db, key, adapter, job_id, job_version, explicit_ids, window_days).await;
        });
        Ok(job)
    }

    /// 单笔同步:直接走既有交付管线(核验→唯一任务→幂等),30s 截止。
    pub async fn sync_single(
        &self,
        account_id: &str,
        order_db_id: &str,
    ) -> Result<SingleSyncOutcome, OrderSyncError>
    where
        A: Clone,
    {
        // 账号可用性优先判定(离线/停用 → AccountUnavailable,先于订单归属检查)
        let account = account_id.to_string();
        let acct = self
            .db
            .call(move |conn| query_account(conn, &account))
            .await??;
        match acct {
            Some(a) if a.runtime_enabled && a.status == "online" => {}
            _ => return Err(OrderSyncError::AccountUnavailable),
        }
        let order_id = order_db_id.to_string();
        let account = account_id.to_string();
        let external = self
            .db
            .call(move |conn| -> rusqlite::Result<Option<String>> {
                let Some(ctx) = orders::get_manual_ctx(conn, &order_id)? else {
                    return Ok(None);
                };
                if ctx.account_id != account {
                    return Ok(None);
                }
                Ok(Some(ctx.external_order_id))
            })
            .await??;
        let Some(external) = external else {
            return Err(OrderSyncError::NotFound);
        };
        let out = self
            .delivery()
            .handle_payment(account_id, &external)
            .await?;
        Ok(single_from_handle(out))
    }
}

fn clone_key(k: &DataKey) -> DataKey {
    DataKey {
        key_id: k.key_id.clone(),
        key: k.key,
    }
}

pub(crate) fn single_from_handle(out: HandleOutcome) -> SingleSyncOutcome {
    match out {
        HandleOutcome::Delivered { delivery_id } => SingleSyncOutcome::Delivered { delivery_id },
        HandleOutcome::Ineligible { delivery_id, reason } => {
            SingleSyncOutcome::Ineligible { delivery_id, reason }
        }
        HandleOutcome::NotSent {
            delivery_id,
            retry_scheduled,
            reason,
        } => SingleSyncOutcome::NotSent {
            delivery_id,
            retry_scheduled,
            reason,
        },
        HandleOutcome::Unknown { delivery_id } => SingleSyncOutcome::Unknown { delivery_id },
        HandleOutcome::AlreadyHandled => SingleSyncOutcome::AlreadyHandled,
        HandleOutcome::Busy => SingleSyncOutcome::Busy,
    }
}

fn query_account(conn: &rusqlite::Connection, id: &str) -> rusqlite::Result<Option<AccountRef>> {
    conn.query_row(
        "SELECT id, status, runtime_enabled, control_epoch, credential_epoch
         FROM accounts WHERE id = ?1",
        rusqlite::params![id],
        |r| {
            Ok(AccountRef {
                id: r.get(0)?,
                status: r.get(1)?,
                runtime_enabled: r.get::<_, i64>(2)? != 0,
                control_epoch: r.get(3)?,
                credential_epoch: r.get(4)?,
            })
        },
    )
    .optional()
}

/// 任务执行体:逐账号追溯+逐单 handle;账号边界检查 cancel_requested。
/// 分类口径:新建(created)/已存在且本轮推进交付(restored)/已存在且状态被修正
/// (reassigned,如不合格重开)/不合格(ineligible,按结果);错误进 failed[]。
async fn run_sync_job<A: PlatformAdapter>(
    db: DbThread,
    key: DataKey,
    adapter: A,
    job_id: String,
    job_version: i64,
    explicit_ids: Option<Vec<String>>,
    window_days: i64,
) where
    A: Clone + Send + 'static,
{
    let jobs = JobService::new(db.clone());
    let delivery = DeliveryService::new(db.clone(), key, adapter);
    let accounts = match list_accounts(&db, explicit_ids).await {
        Ok(a) => a,
        Err(e) => {
            let _ = db
                .call(move |conn| {
                    jobs_repo::finish(conn, &job_id, job_version, "failed", None, Some(&e))
                })
                .await;
            return;
        }
    };
    let mut reports: Vec<serde_json::Value> = Vec::new();
    let mut cancelled = false;
    for acct in accounts {
        // 取消意图在账号边界兑现:已处理账号保留,未开始账号不再追溯
        if let Ok(row) = jobs.get(&job_id).await
            && row.cancel_requested
        {
            cancelled = true;
            break;
        }
        if !acct.runtime_enabled || acct.status != "online" {
            reports.push(serde_json::json!({
                "account_id": acct.id, "offline": true, "orders_seen": 0,
            }));
            continue;
        }
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: acct.id.clone(),
            credential_generation: acct.credential_epoch,
            control_generation: acct.control_epoch,
            deadline_ms: utc_now_ms() + 30_000,
        };
        let to = utc_now_ms();
        let from = to - window_days * 86_400_000;
        let traced = delivery.trace(&ctx, from, to, MAX_TRACE_PAGES).await;
        let report = match traced {
            Ok((candidates, coverage)) => {
                let mut created = 0i64;
                let mut restored = 0i64;
                let mut reassigned = 0i64;
                let mut ineligible = 0i64;
                let mut failed: Vec<serde_json::Value> = Vec::new();
                for external in &candidates {
                    // 预探存在性:created/restored/reassigned 口径基准
                    let existed = order_exists(&db, &acct.id, external).await;
                    match delivery.handle_payment(&acct.id, external).await {
                        Ok(out) => match out {
                            HandleOutcome::Delivered { .. }
                            | HandleOutcome::NotSent { .. }
                            | HandleOutcome::Unknown { .. } => {
                                if existed { restored += 1 } else { created += 1 }
                            }
                            HandleOutcome::Ineligible { .. } => {
                                ineligible += 1;
                                if existed { reassigned += 1 } else { created += 1 }
                            }
                            HandleOutcome::AlreadyHandled | HandleOutcome::Busy => {}
                        },
                        Err(e) => failed.push(serde_json::json!({
                            "order_id": external, "reason": e.to_string(),
                        })),
                    }
                }
                serde_json::json!({
                    "account_id": acct.id,
                    "orders_seen": candidates.len(),
                    "created": created,
                    "restored": restored,
                    "reassigned": reassigned,
                    "ineligible": ineligible,
                    "failed": failed,
                    "coverage": {
                        "complete": coverage.coverage_complete,
                        "from": coverage.observed_from_ms.unwrap_or(from),
                        "to": coverage.observed_to_ms.unwrap_or(to),
                    },
                })
            }
            Err(e) => serde_json::json!({
                "account_id": acct.id,
                "orders_seen": 0,
                "failed": [{ "order_id": null, "reason": format!("追溯失败:{e}") }],
                "coverage": null,
            }),
        };
        reports.push(report);
    }
    let state = if cancelled { "cancelled" } else { "succeeded" };
    let result = serde_json::json!({ "accounts": reports }).to_string();
    // finish 用最新版本(CAS):request_cancel 会推进版本,过期的 job_version 会落空
    if let Ok(row) = jobs.get(&job_id).await
        && !matches!(
            row.state.as_str(),
            "succeeded" | "failed" | "cancelled" | "needs_review"
        )
    {
        let expected = row.version;
        let _ = db
            .call(move |conn| {
                jobs_repo::finish(conn, &job_id, expected, state, Some(&result), None)
            })
            .await;
    }
}

async fn list_accounts(
    db: &DbThread,
    explicit: Option<Vec<String>>,
) -> Result<Vec<AccountRef>, String> {
    let result = db
        .call(move |conn| -> rusqlite::Result<Vec<AccountRef>> {
            let mut refs = Vec::new();
            match explicit {
                Some(ids) => {
                    for id in &ids {
                        if let Some(a) = query_account(conn, id)? {
                            refs.push(a);
                        }
                    }
                }
                None => {
                    let mut stmt = conn.prepare(
                        "SELECT id, status, runtime_enabled, control_epoch, credential_epoch
                         FROM accounts WHERE runtime_enabled = 1 ORDER BY created_at, id",
                    )?;
                    let rows = stmt
                        .query_map([], |r| {
                            Ok(AccountRef {
                                id: r.get(0)?,
                                status: r.get(1)?,
                                runtime_enabled: r.get::<_, i64>(2)? != 0,
                                control_epoch: r.get(3)?,
                                credential_epoch: r.get(4)?,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    refs = rows;
                }
            }
            Ok(refs)
        })
        .await;
    result.map_err(|e| format!("数据库不可用:{e}")).and_then(|r| r.map_err(|e| e.to_string()))
}

async fn order_exists(db: &DbThread, account_id: &str, external: &str) -> bool {
    let account = account_id.to_string();
    let ext = external.to_string();
    db.call(move |conn| -> rusqlite::Result<bool> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM orders WHERE platform = 'xianyu' AND account_id = ?1
             AND external_order_id = ?2",
            rusqlite::params![account, ext],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    })
    .await
    .map(|r| r.unwrap_or(false))
    .unwrap_or(false)
}

#[async_trait::async_trait]
impl<A: PlatformAdapter + Clone + Send + 'static> OrderSyncOps for OrderSyncService<A> {
    async fn start(
        &self,
        account_ids: Vec<String>,
        window_days: i64,
    ) -> Result<JobRow, OrderSyncError> {
        OrderSyncService::start(self, account_ids, window_days).await
    }
    async fn sync_single(
        &self,
        account_id: &str,
        order_db_id: &str,
    ) -> Result<SingleSyncOutcome, OrderSyncError> {
        OrderSyncService::sync_single(self, account_id, order_db_id).await
    }
}
