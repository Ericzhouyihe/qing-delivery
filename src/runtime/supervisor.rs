//! serve 运行时编排(T093):账号值守任务、事件分发、漏单补偿与停止语义。
//! 管理页面关闭后核心交付持续运行(宪章 II);supervisor 只做编排,
//! 协议在适配器、业务在应用服务、状态推进经事件管道持久化。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::credentials;
use crate::adapters::windows::keys::DataKey;
use crate::application::delivery::service::DeliveryService;
use crate::application::events::{EventOutcome, EventPipeline};
use crate::application::manual::actions::ManualService;
use crate::application::ports::platform::{
    EventMeta, PlatformAdapter, PlatformEvent, RequestContext,
};
use crate::application::recovery::compensate::CompensationService;
use crate::application::recovery::trace::TraceScanService;
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

/// 账号连接参数:凭证已由运行时解密,传输层不触碰业务表。
pub struct AccountRun {
    pub account_id: String,
    pub cookie_jar_json: String,
    pub external_id: String,
    pub credential_generation: i64,
    pub control_generation: i64,
}

/// 账号传输层:运行一个账号的平台连接直到停止(含重连策略);
/// 状态变化经 AccountRuntimeChanged 事件上报,不直接写业务表。
#[async_trait::async_trait]
pub trait AccountTransport: Send + Sync {
    async fn run_account(
        &self,
        run: AccountRun,
        events: mpsc::Sender<PlatformEvent>,
        shutdown: watch::Receiver<bool>,
    );
}

/// 运行时对外句柄:人工动作服务(注入 AppState)与停止控制。
pub struct RuntimeHandles<A: PlatformAdapter + 'static> {
    pub manual: Arc<ManualService<A>>,
    pub delivery: Arc<DeliveryService<A>>,
    shutdown: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

impl<A: PlatformAdapter + 'static> RuntimeHandles<A> {
    /// 停止:不再接受新任务;等待收尾最长 15 秒(T053/宪章 IV)。
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(15), self.join).await;
    }
}

const SUPERVISOR_TICK: Duration = Duration::from_secs(3);
const TRACE_INTERVAL: Duration = Duration::from_secs(60);

/// 启动运行时:事件分发、账号值守、漏单补偿与追溯扫描。
/// 各服务为无状态 DB 包装,以同源(db/key/adapter)构造等价实例共享行为。
#[allow(clippy::type_complexity)]
pub fn spawn<A>(
    db: DbThread,
    key: DataKey,
    adapter: A,
    transport: Option<std::sync::Arc<dyn AccountTransport>>,
) -> RuntimeHandles<A>
where
    A: PlatformAdapter + Clone + 'static,
{
    let delivery = std::sync::Arc::new(DeliveryService::new(
        db.clone(),
        key.clone(),
        adapter.clone(),
    ));
    let manual = std::sync::Arc::new(ManualService::new(
        db.clone(),
        key.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter.clone()),
    ));
    let trace = std::sync::Arc::new(TraceScanService::new(
        db.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter.clone()),
    ));
    let compensate = std::sync::Arc::new(CompensationService::new(
        db.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter),
    ));
    let pipeline = EventPipeline::new(db.clone());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (event_tx, event_rx) = mpsc::channel(1024);

    // 事件分发:持久化去重 → 状态推进 → 付款候选触发交付
    let dispatch_delivery = delivery.clone();
    let dispatch_db = db.clone();
    tokio::spawn(async move {
        dispatch_loop(event_rx, pipeline, dispatch_delivery, dispatch_db).await;
    });

    // 账号值守:启停传输任务(其 JoinHandle 即运行时收尾点)
    let sup_join = tokio::spawn(account_supervisor_loop(
        db.clone(),
        key,
        transport,
        event_tx,
        shutdown_rx.clone(),
    ));

    // 补偿与追溯:漏通知 120 秒规则由 CompensationService 内部执行
    tokio::spawn(async move {
        recovery_loop(compensate, trace, db, shutdown_rx).await;
    });

    RuntimeHandles {
        manual,
        delivery,
        shutdown: shutdown_tx,
        join: sup_join,
    }
}

/// 事件元信息提取的辅助字段。
struct EventField {
    account_id: Option<String>,
    external_order_id: Option<String>,
    runtime_state: Option<String>,
}

fn fields_of(event: &PlatformEvent) -> (EventMeta, EventField) {
    let meta = match event {
        PlatformEvent::AuthorizationChanged { meta, .. }
        | PlatformEvent::AccountRuntimeChanged { meta, .. }
        | PlatformEvent::OrderPaymentSignal { meta, .. }
        | PlatformEvent::OrderStateSignal { meta, .. }
        | PlatformEvent::OutgoingMessageEvidence { meta, .. }
        | PlatformEvent::TraceGap { meta, .. }
        | PlatformEvent::ProtocolIssue { meta, .. } => meta.clone(),
    };
    let field = match event {
        PlatformEvent::OrderPaymentSignal { order_id, .. } => EventField {
            account_id: Some(meta.account_id.clone()),
            external_order_id: Some(order_id.clone()),
            runtime_state: None,
        },
        PlatformEvent::OrderStateSignal { order_id, .. } => EventField {
            account_id: Some(meta.account_id.clone()),
            external_order_id: Some(order_id.clone()),
            runtime_state: None,
        },
        PlatformEvent::AccountRuntimeChanged { state, .. } => EventField {
            account_id: Some(meta.account_id.clone()),
            external_order_id: None,
            runtime_state: Some(state.clone()),
        },
        _ => EventField {
            account_id: Some(meta.account_id.clone()),
            external_order_id: None,
            runtime_state: None,
        },
    };
    (meta, field)
}

async fn dispatch_loop<A: PlatformAdapter + 'static>(
    mut rx: mpsc::Receiver<PlatformEvent>,
    pipeline: EventPipeline,
    delivery: std::sync::Arc<DeliveryService<A>>,
    db: DbThread,
) {
    while let Some(event) = rx.recv().await {
        let (_meta, field) = fields_of(&event);
        // 观测状态(期望状态 runtime_enabled 由控制 API 管理,不在事件回写)
        if let Some(state) = field.runtime_state.clone()
            && let Some(account) = field.account_id.clone()
        {
            let _ = db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE accounts SET status = ?2, version = version + 1, updated_at = ?3
                         WHERE id = ?1",
                        rusqlite::params![account, state, utc_now_ms()],
                    )
                })
                .await;
        }
        match pipeline.process(&event).await {
            Ok(EventOutcome::PaymentSignal { .. })
                if field.account_id.is_some() && field.external_order_id.is_some() =>
            {
                let account = field.account_id.clone().unwrap_or_default();
                let order = field.external_order_id.clone().unwrap_or_default();
                // 全链核验:资格→唯一 initial→冻结→发送→分类;重复由幂等与唯一约束兜底
                if let Err(e) = delivery.handle_payment(&account, &order).await {
                    tracing::warn!(account, order, error = %e, "付款候选交付处理失败");
                }
            }
            Ok(_) | Err(_) => {}
        }
    }
}

struct AccountTask {
    stop: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

async fn account_supervisor_loop(
    db: DbThread,
    key: DataKey,
    transport: Option<std::sync::Arc<dyn AccountTransport>>,
    event_tx: mpsc::Sender<PlatformEvent>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut active: HashMap<String, AccountTask> = HashMap::new();
    let mut ticker = tokio::time::interval(SUPERVISOR_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
            _ = ticker.tick() => {
                let Some(transport) = transport.as_ref() else { continue };
                let runs = load_enabled_accounts(&db, &key).await;
                let wanted: Vec<String> = runs.iter().map(|r| r.account_id.clone()).collect();
                // 停掉不再启用的
                for task in active.values() {
                    let _ = task.stop.send(false); // 保持接收端活跃
                }
                for (id, task) in active.iter() {
                    if !wanted.contains(id) {
                        let _ = task.stop.send(true);
                    }
                }
                active.retain(|_, t| !t.join.is_finished());
                for run in runs {
                    let account_id = run.account_id.clone();
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        active.entry(account_id)
                    {
                        let (stop_tx, stop_rx) = watch::channel(false);
                        let transport = transport.clone();
                        let events = event_tx.clone();
                        let join = tokio::spawn(async move {
                            transport.run_account(run, events, stop_rx).await;
                        });
                        entry.insert(AccountTask {
                            stop: stop_tx,
                            join,
                        });
                    }
                }
            }
        }
    }
    // 停机收尾:通知全部账号停止并等待(≤15 秒;未知结果保留)
    for task in active.values() {
        let _ = task.stop.send(true);
    }
    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        for (_, task) in active {
            let _ = task.join.await;
        }
    })
    .await;
}

async fn recovery_loop<A: PlatformAdapter + 'static>(
    compensate: std::sync::Arc<CompensationService<A>>,
    trace: std::sync::Arc<TraceScanService<A>>,
    db: DbThread,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(TRACE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // 首个即时 tick 跳过:启动恢复由 boot 流程负责
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
            _ = tick.tick() => {
                if let Err(e) = compensate.run_once().await {
                    tracing::warn!(error = %e, "漏单补偿轮询失败");
                }
                for account in enabled_account_ids(&db).await {
                    let Some(ctx) = request_context(&db, &account).await else {
                        continue;
                    };
                    if let Err(e) = trace.run_once(&account, &ctx).await {
                        tracing::warn!(account, error = %e, "订单追溯扫描失败");
                    }
                }
            }
        }
    }
}

/// 启用中的账号及其解密凭证;无凭证(未接入)跳过。
async fn load_enabled_accounts(db: &DbThread, key: &DataKey) -> Vec<AccountRun> {
    let key_bytes = key.key;
    let rows = db
        .call(move |conn| -> rusqlite::Result<Vec<AccountRun>> {
            let mut stmt = conn.prepare(
                "SELECT id, external_user_id, control_epoch, credential_epoch
                 FROM accounts
                 WHERE runtime_enabled = 1 AND external_user_id != ''",
            )?;
            let runs = stmt
                .query_map([], |r| {
                    Ok(AccountRun {
                        account_id: r.get(0)?,
                        cookie_jar_json: String::new(),
                        external_id: r.get(1)?,
                        credential_generation: r.get(3)?,
                        control_generation: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut with_creds = Vec::new();
            for mut run in runs {
                if let Some(blob) = credentials::load(conn, &run.account_id, &key_bytes)? {
                    run.cookie_jar_json = blob.cookie_jar_json;
                    with_creds.push(run);
                }
            }
            Ok(with_creds)
        })
        .await;
    rows.ok().and_then(|r| r.ok()).unwrap_or_default()
}

async fn enabled_account_ids(db: &DbThread) -> Vec<String> {
    db.call(|conn| -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT id FROM accounts WHERE runtime_enabled = 1 AND external_user_id != ''",
        )?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .unwrap_or_default()
}

async fn request_context(db: &DbThread, account_id: &str) -> Option<RequestContext> {
    let account = account_id.to_string();
    let result = db
        .call(move |conn| -> rusqlite::Result<RequestContext> {
            conn.query_row(
                "SELECT control_epoch, credential_epoch FROM accounts WHERE id = ?1",
                rusqlite::params![account],
                |r| {
                    Ok(RequestContext {
                        operation_id: ids::new_id("op"),
                        account_id: account.clone(),
                        credential_generation: r.get(1)?,
                        control_generation: r.get(0)?,
                        deadline_ms: utc_now_ms() + 30_000,
                    })
                },
            )
        })
        .await;
    result.ok().and_then(|r| r.ok())
}
