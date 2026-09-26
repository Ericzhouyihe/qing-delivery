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
use crate::application::chat::{ChatService, IncomingChatMessage};
use crate::application::delivery::service::DeliveryService;
use crate::application::events::{EventOutcome, EventPipeline};
use crate::application::manual::actions::ManualService;
use crate::application::ports::platform::{
    EventMeta, PlatformAdapter, PlatformEvent, RequestContext,
};
use crate::application::recovery::compensate::CompensationService;
use crate::application::recovery::trace::TraceScanService;
use crate::application::replies::ReviewReminderService;
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
/// 求评计划扫描间隔(007 T037,FR-035:默认每小时一次)
const REVIEW_REMINDER_INTERVAL: Duration = Duration::from_secs(3600);

/// 启动运行时:事件分发、账号值守、漏单补偿与追溯扫描。
/// 各服务为无状态 DB 包装,以同源(db/key/adapter)构造等价实例共享行为。
#[allow(clippy::type_complexity)]
pub fn spawn<A>(
    db: DbThread,
    key: DataKey,
    adapter: A,
    transport: Option<std::sync::Arc<dyn AccountTransport>>,
    verification: Option<
        std::sync::Arc<crate::application::verification::service::VerificationService>,
    >,
) -> RuntimeHandles<A>
where
    A: PlatformAdapter + Clone + 'static,
{
    // 007 T062(US5):通知服务(事件挂钩 + 交付终态通知;spawn 扇出)
    let notify = std::sync::Arc::new(crate::application::notify::NotifyService::new(
        db.clone(),
        key.clone(),
    ));
    let delivery = std::sync::Arc::new(
        DeliveryService::new(db.clone(), key.clone(), adapter.clone())
            .with_notify(notify.clone()),
    );
    let manual = std::sync::Arc::new(ManualService::new(
        db.clone(),
        key.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter.clone())
            .with_notify(notify.clone()),
    ));
    let trace = std::sync::Arc::new(TraceScanService::new(
        db.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter.clone())
            .with_notify(notify.clone()),
    ));
    let compensate = std::sync::Arc::new(CompensationService::new(
        db.clone(),
        DeliveryService::new(db.clone(), key.clone(), adapter.clone())
            .with_notify(notify.clone()),
    ));    // 007 T050:聊天用例(买家消息摄取 + 发送回执回填,事件分发消费)
    // 007 T077:AI 分支实装(HttpAiProvider:系统未配 Key 等价 NoAi 直落默认)
    let chat = std::sync::Arc::new(
        ChatService::new(db.clone(), adapter.clone()).with_ai(std::sync::Arc::new(
            crate::application::replies::HttpAiProvider::new(
                db.clone(),
                crate::application::settings_sys::SettingsService::new(db.clone(), key.clone()),
            ),
        )),
    );
    // 007 T037:求评计划服务(每小时扫描 accepted 订单,按 review_config 发送)
    let reminders = std::sync::Arc::new(ReviewReminderService::new(db.clone(), adapter));
    let pipeline = EventPipeline::new(db.clone());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (event_tx, event_rx) = mpsc::channel(1024);

    // 事件分发:持久化去重 → 状态推进 → 付款候选触发交付
    let dispatch_delivery = delivery.clone();
    let dispatch_db = db.clone();
    let dispatch_notify = notify.clone();
    tokio::spawn(async move {
        let dispatch_verify = verification.clone();
        dispatch_loop(
            event_rx,
            pipeline,
            dispatch_delivery,
            dispatch_db,
            dispatch_verify,
            chat,
            dispatch_notify,
        )
        .await;
    });

    // 账号值守:启停传输任务(其 JoinHandle 即运行时收尾点)
    let sup_join = tokio::spawn(account_supervisor_loop(
        db.clone(),
        key,
        transport,
        event_tx,
        shutdown_rx.clone(),
    ));

    // 007 T037:求评计划(每小时;启动即扫一轮补停机缺口)
    let reminder_shutdown = shutdown_rx.clone();
    // 补偿与追溯:漏通知 120 秒规则由 CompensationService 内部执行
    tokio::spawn(async move {
        recovery_loop(compensate, trace, db, shutdown_rx).await;
    });

    tokio::spawn(async move {
        reminder_loop(reminders, reminder_shutdown).await;
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
        | PlatformEvent::ProtocolIssue { meta, .. }
        | PlatformEvent::ChatMessageReceived { meta, .. } => meta.clone(),
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

async fn dispatch_loop<A: PlatformAdapter + Clone + 'static>(
    mut rx: mpsc::Receiver<PlatformEvent>,
    pipeline: EventPipeline,
    delivery: std::sync::Arc<DeliveryService<A>>,
    db: DbThread,
    verification: Option<
        std::sync::Arc<crate::application::verification::service::VerificationService>,
    >,
    chat: std::sync::Arc<ChatService<A>>,
    notify: std::sync::Arc<crate::application::notify::NotifyService>,
) {
    while let Some(event) = rx.recv().await {
        let (_meta, field) = fields_of(&event);
        // 观测状态(期望状态 runtime_enabled 由控制 API 管理,不在事件回写)
        if let Some(state) = field.runtime_state.clone()
            && let Some(account) = field.account_id.clone()
        {
            let _ = db
                .call({
                    let account = account.clone();
                    let state = state.clone();
                    move |conn| {
                        conn.execute(
                            "UPDATE accounts SET status = ?2, version = version + 1, updated_at = ?3
                             WHERE id = ?1",
                            rusqlite::params![account, state, utc_now_ms()],
                        )
                    }
                })
                .await;
            // 007 T062(US5):掉线/恢复通知(spawn,不阻塞事件管道)
            notify.on_runtime_changed(&account, &state);
        }
        // 003:验证要求信号(WS/mtop/QR 汇入 AuthorizationChanged)→ 处置服务
        if let PlatformEvent::AuthorizationChanged { state, .. } = &event
            && state == "verification_required"
            && let Some(service) = verification.as_ref()
            && let Some(account) = field.account_id.clone()
        {
            // 007 T062(US5):安全验证通知(spawn)
            notify.on_verification_required(&account);
            service.handle_signal(crate::application::verification::VerificationSignal {
                account_id: account,
                source: crate::application::verification::SignalSource::Ws,
                url: None,
                raw: state.clone(),
            });
        }
        // 007 T062(US5):协议异常 → 系统错误通知(spawn)
        if let PlatformEvent::ProtocolIssue { meta, reason, .. } = &event {
            notify.on_system_error(Some(&meta.account_id), reason);
        }
        // 007 T045:买家聊天消息 → 聊天摄取(幂等去重/未读/触发自动回复分流);
        // 仅买家方向事件会出现在此变体(系统/卡片消息走既有路径)
        if let PlatformEvent::ChatMessageReceived {
            meta,
            chat_id,
            buyer_id,
            message_id,
            msg_kind,
            text,
            image_url,
            item_id,
        } = &event
        {
            if let Some(buyer) = buyer_id.clone().filter(|b| !b.trim().is_empty()) {
                let incoming = IncomingChatMessage {
                    account_id: meta.account_id.clone(),
                    platform_message_id: message_id.clone(),
                    chat_id: chat_id.clone(),
                    buyer_id: buyer,
                    msg_kind: *msg_kind,
                    text: text.clone(),
                    image_url: image_url.clone(),
                    item_id: item_id.clone(),
                };
                if let Err(e) = chat.ingest(incoming).await {
                    tracing::warn!(account = %meta.account_id, error = %e, "聊天消息摄取失败");
                }
            } else {
                tracing::warn!(account = %meta.account_id, "聊天消息缺少买家标识,丢弃");
            }
        }
        // 007 T050:发送回执 → 出站行回填(无匹配行=交付类回执,正常 no-op)。
        // 消费方式注明:不经管道改造,dispatch_loop 直调 ChatService::apply_evidence。
        if let PlatformEvent::OutgoingMessageEvidence {
            meta,
            request_id,
            platform_message_id,
            ..
        } = &event
            && let Some(pmid) = platform_message_id.clone().filter(|v| !v.is_empty())
            && let Err(e) = chat.apply_evidence(request_id, &pmid).await
        {
            tracing::warn!(account = %meta.account_id, error = %e, "聊天发送回执回填失败");
        }
        match pipeline.process(&event).await {
            Ok(EventOutcome::PaymentSignal { .. })
                if field.account_id.is_some() && field.external_order_id.is_some() =>
            {
                let account = field.account_id.clone().unwrap_or_default();
                let order = field.external_order_id.clone().unwrap_or_default();
                // 003 FR-003:验证闸门关闭时延后触发;120s 补偿扫描兜底
                if let Some(service) = verification.as_ref()
                    && !service.gate.allows(&account)
                {
                    tracing::warn!(account, "验证处置中,交付触发延后(闸门)");
                    continue;
                }
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

/// 求评计划定时循环(007 T037):照 recovery_loop 模式;每小时一轮,
/// 启动即扫一轮(补停机期间到期缺口)。停止语义与 recovery_loop 一致。
async fn reminder_loop<A: PlatformAdapter + 'static>(
    reminders: std::sync::Arc<ReviewReminderService<A>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(REVIEW_REMINDER_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
            _ = tick.tick() => {
                if let Err(e) = reminders.run_once().await {
                    tracing::warn!(error = %e, "求评计划扫描失败");
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
