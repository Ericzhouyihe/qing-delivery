//! US6 集成测试(T067):一键同步任务逐账号报告与取消(cancel_requested 兑现)、
//! 单笔同步、人工触发交付幂等、付款事实缺失拒绝、人工确认平台发货前提与独立留痕、
//! 人工动作审计(manual_actions 行存在、reason 记录)。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, items, orders, rules};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::delivery::service::{DeliveryService, HandleOutcome};
use qing_delivery::application::jobs::{JobRow, JobService};
use qing_delivery::application::manual::actions::{
    ManualError, ManualOutcome, ManualRequest, ManualService,
};
use qing_delivery::application::orders_sync::{OrderSyncError, OrderSyncService, SingleSyncOutcome};
use qing_delivery::application::ports::platform::{
    AuthorizationSession, ConfirmOutcome, PlatformAdapter, PlatformError, ProductPage,
    RequestContext, TraceReport,
};
use qing_delivery::domain::crypto;
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{
    FieldStatus, PlatformOrderState, SnapshotBuilder, TradeType,
};

const RULE_TEXT: &str = "资料链接:https://example.com/d/abc\n提取码:ab12";

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

fn complete_snapshot(order: &str) -> SnapshotBuilder {
    SnapshotBuilder {
        platform_order_id: order.into(),
        seller_id: FieldStatus::Verified("seller-1".into()),
        buyer_id: FieldStatus::Verified("buyer-1".into()),
        item_id: FieldStatus::Verified("EXT-ITEM-1".into()),
        trade_type: FieldStatus::Verified(TradeType::Ordinary),
        platform_state: FieldStatus::Verified(PlatformOrderState::PendingShip),
        paid_at_ms: FieldStatus::Verified(1_760_000_000_000),
        amount: FieldStatus::Verified(Money::new(990, "CNY").unwrap()),
        quantity: FieldStatus::Verified(1),
        sku_parts: FieldStatus::Missing,
        sku_single: true,
        conversation_verified: true,
        source: "detail".into(),
        observed_at_ms: 1_760_000_001_000,
        browser_supplemented: false,
    }
}

/// 无规则命中的快照(item 行不存在 → NoRule → Ineligible)。
fn no_rule_snapshot(order: &str) -> SnapshotBuilder {
    let mut s = complete_snapshot(order);
    s.item_id = FieldStatus::Verified("EXT-ITEM-NO-RULE".into());
    s
}

/// 自动确认关闭:确认轴留给人工 confirm_shipment 显式驱动。
async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us6.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    let key_for_seed = key.key;
    let key_id = key.key_id.clone();
    db.call(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO admin(id, username, password_hash, created_at, password_changed_at)
             VALUES ('adm_test', 'admin', 'seed-hash', 0, 0)",
            [],
        )?;
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家一")?;
        accounts::set_control(
            conn,
            "acct-1",
            "online",
            Some(true),
            Some(true),
            Some(false),
            Some(1_600_000_000_000),
        )?;
        items::upsert(conn, "item-1", "acct-1", "EXT-ITEM-1", "资料", "on_sale", "[]", "single")?;
        let rule = rules::insert(conn, "rule-1", "acct-1", "item-1", "single", true)?;
        let envelope = crypto::seal(&key_for_seed, &key_id, &rules::rule_aad(&rule.id, 1), RULE_TEXT.as_bytes());
        rules::insert_content(conn, "rc-1", &rule.id, 1, &envelope, "digest", RULE_TEXT.chars().count() as i64, RULE_TEXT.len() as i64)
    })
    .await
    .unwrap()
    .unwrap();
    Harness {
        db,
        key,
        fake: FakeAdapter::new(),
        _dir: dir,
        _lock: lock,
    }
}

/// 增补第二个账号(默认停用外置;测试内自行 set_control)。
async fn seed_account(h: &Harness, id: &str, status: &str, runtime: bool) {
    let owned = id.to_string();
    let status = status.to_string();
    h.db
        .call(move |conn| {
            accounts::upsert_identity(conn, &owned, "xianyu", &format!("seller-{owned}"), "卖家")?;
            accounts::set_control(conn, &owned, &status, Some(runtime), None, None, None)?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
}

fn clone_key(k: &DataKey) -> DataKey {
    DataKey {
        key_id: k.key_id.clone(),
        key: k.key,
    }
}

/// 种一条已存在订单(带付款事实与买家,尚无交付);返回内部 ID。
async fn seed_order(h: &Harness, external: &str) -> String {
    let external_owned = external.to_string();
    let local_id = format!("ord_{external}");
    h.db
        .call(move |conn| {
            let (row, _) =
                orders::find_or_create(conn, &local_id, "xianyu", "acct-1", &external_owned)?;
            orders::merge_order_facts(
                conn,
                &row.id,
                &orders::OrderFactMerge {
                    buyer_id: Some("buyer-1"),
                    paid_at: Some(1_760_000_000_000),
                    quantity: Some(1),
                    amount_minor: Some(990),
                    currency: Some("CNY"),
                    sku_pairs: None,
                    sku_complete: None,
                    trade_type: Some("ordinary"),
                },
            )?;
            conn.execute(
                "UPDATE orders SET platform_status='pending_ship' WHERE id=?1",
                rusqlite::params![row.id],
            )?;
            Ok::<_, rusqlite::Error>(row.id)
        })
        .await
        .unwrap()
        .unwrap()
}

fn sync_service(h: &Harness) -> OrderSyncService<FakeAdapter> {
    OrderSyncService::new(h.db.clone(), clone_key(&h.key), h.fake.clone())
}

fn manual_service(h: &Harness) -> ManualService<FakeAdapter> {
    ManualService::new(
        h.db.clone(),
        clone_key(&h.key),
        DeliveryService::new(h.db.clone(), clone_key(&h.key), h.fake.clone()),
    )
}

fn manual_req(order: &str, key: &str) -> ManualRequest {
    ManualRequest {
        admin_id: "adm_test".into(),
        account_id: "acct-1".into(),
        order_db_id: order.into(),
        reason: "人工核对后操作".into(),
        risk_confirmed: false,
        idempotency_key: key.into(),
    }
}

/// 轮询任务直至终态(执行器在后台 spawn;窗口 5s 防死等)。
async fn wait_terminal(db: &DbThread, job_id: &str) -> JobRow {
    let jobs = JobService::new(db.clone());
    for _ in 0..500 {
        if let Ok(row) = jobs.get(job_id).await {
            if matches!(
                row.state.as_str(),
                "succeeded" | "failed" | "cancelled" | "needs_review"
            ) {
                return row;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("任务未在 5 秒内到达终态");
}

fn parse_result(job: &JobRow) -> serde_json::Value {
    serde_json::from_str(job.result_ref.as_deref().unwrap_or("{}")).unwrap()
}

// ---- T067-1:同步任务逐账号报告字段 ----

/// 一键同步产出逐账号报告:orders_seen/created/restored/reassigned/ineligible/
/// failed[]/coverage 全字段;离线账号跳过并标 offline。
#[tokio::test]
async fn sync_job_reports_per_account_fields() {
    let h = harness().await;
    seed_account(&h, "acct-2", "offline", true).await;
    // acct-1:两笔新建可交付 + 一笔新建不合格(无规则)+ 一笔拉取失败(无快照)+ 一笔已存在订单恢复
    h.fake.stage_trace(
        vec![
            "ORD-S1".into(),
            "ORD-S2".into(),
            "ORD-S3".into(),
            "ORD-S4".into(),
            "ORD-S5".into(),
        ],
        true,
    );
    h.fake.stage_snapshot(complete_snapshot("ORD-S1").build());
    h.fake.stage_snapshot(complete_snapshot("ORD-S2").build());
    h.fake.stage_snapshot(no_rule_snapshot("ORD-S3").build());
    // ORD-S4 不预置快照 → fetch 失败 → failed[]
    h.fake.stage_snapshot(complete_snapshot("ORD-S5").build());
    let existed = seed_order(&h, "ORD-S5").await;

    let svc = sync_service(&h);
    let job = svc.start(vec![], 7).await.unwrap();
    let final_row = wait_terminal(&h.db, &job.id).await;
    assert_eq!(final_row.state, "succeeded", "全部账号完成即成功");

    let v = parse_result(&final_row);
    let accounts = v["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2, "默认覆盖全部启用账号");
    let a1 = accounts
        .iter()
        .find(|a| a["account_id"] == "acct-1")
        .expect("acct-1 报告存在");
    assert_eq!(a1["orders_seen"], 5);
    assert_eq!(a1["created"], 3, "S1/S2/S3 新建");
    assert_eq!(a1["restored"], 1, "S5 已存在订单补交付成功");
    assert_eq!(a1["reassigned"], 0);
    assert_eq!(a1["ineligible"], 1, "S3 无规则不合格");
    let failed = a1["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["order_id"], "ORD-S4");
    assert!(
        failed[0]["reason"].as_str().unwrap().contains("ORD-S4"),
        "失败原因列明订单:{failed:?}"
    );
    assert_eq!(a1["coverage"]["complete"], true, "覆盖完整");
    assert!(a1["coverage"]["from"].is_i64() && a1["coverage"]["to"].is_i64());
    assert!(a1.get("offline").is_none(), "在线账号不标 offline");

    let a2 = accounts
        .iter()
        .find(|a| a["account_id"] == "acct-2")
        .expect("acct-2 报告存在");
    assert_eq!(a2["offline"], true, "离线账号跳过并标注");
    assert_eq!(a2["orders_seen"], 0);
    assert!(a2["coverage"].is_null(), "离线账号无覆盖宣称");
    let _ = existed;

    // 交付事实:S1/S2/S5 三笔真实发送
    assert_eq!(h.fake.sent_records().len(), 3);
}

/// created/restored/reassigned 口径(已处理订单重复同步零增量)。
#[tokio::test]
async fn sync_job_rerun_counts_reassigned_not_duplicating() {
    let h = harness().await;
    h.fake
        .stage_trace(vec!["ORD-R1".into(), "ORD-R2".into()], true);
    h.fake.stage_snapshot(no_rule_snapshot("ORD-R2").build());
    h.fake.stage_snapshot(complete_snapshot("ORD-R1").build());
    let svc = sync_service(&h);
    let job = svc.start(vec!["acct-1".into()], 7).await.unwrap();
    let first = wait_terminal(&h.db, &job.id).await;
    let v1 = parse_result(&first);
    let a1 = &v1["accounts"][0];
    assert_eq!(a1["created"], 2);
    assert_eq!(a1["ineligible"], 1);

    // 第二轮:R1 已处理(AlreadyHandled 零变化);R2 仍不合格(pending_verification 重建
    // 资格裁决 → Ineligible,已存在订单经事实推进修正 → reassigned 口径)
    h.fake
        .stage_trace(vec!["ORD-R1".into(), "ORD-R2".into()], true);
    h.fake.stage_snapshot(no_rule_snapshot("ORD-R2").build());
    let job2 = svc.start(vec!["acct-1".into()], 7).await.unwrap();
    let second = wait_terminal(&h.db, &job2.id).await;
    let v2 = parse_result(&second);
    let a2 = &v2["accounts"][0];
    assert_eq!(a2["orders_seen"], 2);
    assert_eq!(a2["created"], 0, "无新建");
    assert_eq!(a2["restored"], 0);
    assert_eq!(a2["reassigned"], 1, "已存在订单状态修正(不合格重开)");
    assert_eq!(a2["ineligible"], 1);
    assert_eq!(h.fake.sent_records().len(), 1, "SC-005:重复同步不重发");
}

// ---- T067-2:取消语义(cancel_requested 在账号边界兑现) ----

/// 门闸适配器:acct-1 的追溯调用挂起直至测试放行(保证取消窗口确定)。
#[derive(Clone)]
struct GatedAdapter {
    inner: FakeAdapter,
    gate: Arc<tokio::sync::Notify>,
    trace_calls: Arc<Mutex<HashMap<String, usize>>>,
}

impl GatedAdapter {
    fn new(inner: FakeAdapter) -> Self {
        Self {
            inner,
            gate: Arc::new(tokio::sync::Notify::new()),
            trace_calls: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl PlatformAdapter for GatedAdapter {
    async fn start_authorization(
        &self,
        ctx: &RequestContext,
    ) -> Result<AuthorizationSession, PlatformError> {
        self.inner.start_authorization(ctx).await
    }
    async fn stop_account(&self, ctx: &RequestContext) -> Result<(), PlatformError> {
        self.inner.stop_account(ctx).await
    }
    async fn list_products(
        &self,
        ctx: &RequestContext,
        cursor: Option<&str>,
    ) -> Result<ProductPage, PlatformError> {
        self.inner.list_products(ctx, cursor).await
    }
    async fn fetch_order_snapshot(
        &self,
        ctx: &RequestContext,
        external_order_id: &str,
    ) -> Result<qing_delivery::domain::orders::snapshot::OrderSnapshot, PlatformError> {
        self.inner.fetch_order_snapshot(ctx, external_order_id).await
    }
    async fn trace_sold_orders(
        &self,
        ctx: &RequestContext,
        from_ms: i64,
        to_ms: i64,
        max_pages: u32,
    ) -> Result<(Vec<String>, TraceReport), PlatformError> {
        *self
            .trace_calls
            .lock()
            .unwrap()
            .entry(ctx.account_id.clone())
            .or_default() += 1;
        if ctx.account_id == "acct-1" {
            self.gate.notified().await;
        }
        self.inner.trace_sold_orders(ctx, from_ms, to_ms, max_pages).await
    }
    async fn send_text(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        buyer_id: &str,
        content: &qing_delivery::application::ports::platform::ContentForSend,
    ) -> Result<qing_delivery::application::ports::platform::SendOutcome, PlatformError> {
        self.inner.send_text(ctx, order_id, buyer_id, content).await
    }
    async fn confirm_shipment(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        prior_proof: &qing_delivery::application::ports::platform::SendProof,
    ) -> Result<ConfirmOutcome, PlatformError> {
        self.inner.confirm_shipment(ctx, order_id, prior_proof).await
    }
    async fn verify_shipment_state(
        &self,
        ctx: &RequestContext,
        order_id: &str,
    ) -> Result<bool, PlatformError> {
        self.inner.verify_shipment_state(ctx, order_id).await
    }
}

/// 取消请求在账号 1 处理中到达:执行器在账号边界停下,job 终态 cancelled,
/// 已处理部分保留在 result_ref,账号 2 未被追溯。
#[tokio::test]
async fn sync_cancel_stops_at_account_boundary() {
    let mut h = harness().await;
    seed_account(&h, "acct-2", "online", true).await;
    let gated = GatedAdapter::new(h.fake.clone());
    // acct-1 一笔可交付(保留"已处理部分");acct-2 一笔(不应被处理)
    h.fake.stage_trace(vec!["ORD-K1".into()], true);
    h.fake.stage_snapshot(complete_snapshot("ORD-K1").build());
    h.fake.stage_trace(vec!["ORD-K2".into()], true);
    h.fake.stage_snapshot(complete_snapshot("ORD-K2").build());

    let svc = OrderSyncService::new(h.db.clone(), clone_key(&h.key), gated.clone());
    let job = svc.start(vec!["acct-1".into(), "acct-2".into()], 7).await.unwrap();

    // 等执行器进入 acct-1 的追溯(挂起在门闸上)
    for _ in 0..500 {
        if *gated.trace_calls.lock().unwrap().get("acct-1").unwrap_or(&0) >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // 取消(版本竞争重试)
    let jobs = JobService::new(h.db.clone());
    loop {
        let row = jobs.get(&job.id).await.unwrap();
        if matches!(
            row.state.as_str(),
            "succeeded" | "failed" | "cancelled" | "needs_review"
        ) {
            panic!("任务提前终态:{row:?}")
        }
        match jobs.request_cancel(&job.id, row.version).await {
            Ok(_) => break,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
    }
    gated.gate.notify_one(); // 放行 acct-1 追溯

    let final_row = wait_terminal(&h.db, &job.id).await;
    assert_eq!(final_row.state, "cancelled", "取消语义:账号边界停下");
    let v = parse_result(&final_row);
    let accounts = v["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 1, "仅保留已处理账号");
    assert_eq!(accounts[0]["account_id"], "acct-1");
    assert_eq!(accounts[0]["orders_seen"], 1);
    assert_eq!(accounts[0]["created"], 1, "已处理部分保留");
    assert!(
        !gated.trace_calls.lock().unwrap().contains_key("acct-2"),
        "acct-2 未被追溯"
    );
    assert!(h.fake.sent_records().iter().any(|r| r.text == RULE_TEXT));
}

// ---- T067-3:人工触发交付(成功 + 幂等拒绝)+ 审计留痕 ----

#[tokio::test]
async fn trigger_delivery_full_pipeline_and_idempotent_reject() {
    let h = harness().await;
    let order_id = seed_order(&h, "ORD-TD").await;
    h.fake.stage_snapshot(complete_snapshot("ORD-TD").build());
    let manual = manual_service(&h);

    // 首次触发:走完整管线(核验→建任务→冻结→发送→分类)
    let out = manual
        .trigger_delivery(manual_req(&order_id, "key-td-1"))
        .await
        .unwrap();
    match out {
        ManualOutcome::Triggered(o) => {
            assert!(
                matches!(*o, HandleOutcome::Delivered { .. }),
                "完整管线交付:{o:?}"
            );
        }
        other => panic!("首次触发应成功交付:{other:?}"),
    }
    assert_eq!(h.fake.sent_records().len(), 1);

    // 重复触发(新幂等键):AlreadyHandled → TriggerNoOp(幂等拒绝,不再发送)
    let dup = manual
        .trigger_delivery(manual_req(&order_id, "key-td-2"))
        .await
        .unwrap();
    assert!(
        matches!(dup, ManualOutcome::TriggerNoOp(ref o) if matches!(**o, HandleOutcome::AlreadyHandled)),
        "重复触发幂等拒绝:{dup:?}"
    );
    assert_eq!(h.fake.sent_records().len(), 1, "重复触发不重发");

    // 同键重放:幂等重放语义
    let replay = manual
        .trigger_delivery(manual_req(&order_id, "key-td-1"))
        .await
        .unwrap();
    assert!(matches!(replay, ManualOutcome::IdempotentReplay));
    assert_eq!(h.fake.sent_records().len(), 1);

    // 审计留痕:trigger_delivery 行存在且记录 reason
    let probe = order_id.clone();
    let audit = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                conn.query_row(
                    "SELECT reason FROM manual_actions WHERE order_id = ?1 AND action = 'trigger_delivery'",
                    rusqlite::params![probe],
                    |r| r.get::<_, String>(0),
                )
                .optional()?,
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.unwrap(), "人工核对后操作");
}

// ---- T067-4:付款事实缺失 → 拒绝并列明缺失要素 ----

#[tokio::test]
async fn trigger_delivery_missing_payment_facts_rejected_with_fields() {
    let h = harness().await;
    // 不合并任何付款事实:paid_at/金额/买家全部缺失
    let external = "ORD-MF";
    let local_id = "ord_ORD-MF";
    let order_id = h
        .db
        .call(move |conn| {
            let (row, _) = orders::find_or_create(conn, local_id, "xianyu", "acct-1", external)?;
            Ok::<_, rusqlite::Error>(row.id)
        })
        .await
        .unwrap()
        .unwrap();

    let manual = manual_service(&h);
    let err = manual
        .trigger_delivery(manual_req(&order_id, "key-mf-1"))
        .await
        .unwrap_err();
    match &err {
        ManualError::MissingFacts(fields) => {
            let joined = fields.join(",");
            assert!(joined.contains("paid_at"), "列明付款时间缺失:{fields:?}");
            assert!(joined.contains("amount"), "列明金额缺失:{fields:?}");
            assert!(joined.contains("buyer"), "列明买家定位缺失:{fields:?}");
        }
        other => panic!("应拒绝缺失事实订单:{other:?}"),
    }
    assert!(
        err.to_string().contains("paid_at"),
        "错误信息列明缺失要素:{}",
        err
    );
    assert_eq!(h.fake.sent_records().len(), 0, "缺核验绝不发送");
}

// ---- T067-5:人工确认平台发货(前提 accepted;确认轴独立) ----

#[tokio::test]
async fn confirm_shipment_precondition_and_independent_axis() {
    let h = harness().await;
    let manual = manual_service(&h);
    let delivery = DeliveryService::new(h.db.clone(), clone_key(&h.key), h.fake.clone());

    // 5a 交付未 accepted(结果未知)→ 422 语义(NotAllowed)
    h.fake.stage_snapshot(complete_snapshot("ORD-CF1").build());
    h.fake
        .script_send(qing_delivery::application::ports::platform::SendOutcome::Unknown {
            hint: None,
        });
    let out = delivery.handle_payment("acct-1", "ORD-CF1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Unknown { .. }));
    let order1 = order_id_of(&h, "ORD-CF1").await;
    let err = manual
        .confirm_shipment(manual_req(&order1, "key-cf1"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ManualError::NotAllowed(ref m) if m.contains("accepted")),
        "交付未 accepted 必须拒绝:{err:?}"
    );
    assert!(h.fake.confirm_calls().is_empty(), "前置不满足不触平台");

    // 5b accepted → 确认独立记录:confirmation 轴推进,内容轴不变
    h.fake.stage_snapshot(complete_snapshot("ORD-CF2").build());
    delivery.handle_payment("acct-1", "ORD-CF2").await.unwrap();
    let order2 = order_id_of(&h, "ORD-CF2").await;
    let out = manual
        .confirm_shipment(manual_req(&order2, "key-cf2"))
        .await
        .unwrap();
    match &out {
        ManualOutcome::ConfirmShipment {
            platform_confirmed, ..
        } => assert!(*platform_confirmed),
        other => panic!("确认成功:{other:?}"),
    }
    assert_eq!(h.fake.confirm_calls(), vec!["ORD-CF2".to_string()]);
    let row = delivery_row(&h, "ORD-CF2").await;
    assert_eq!(row.content_state, "accepted", "消息交付状态不变");
    assert_eq!(row.confirmation_state, "accepted", "确认轴独立确认");
    // attempts 留痕 manual_confirm
    let attempt_kinds = attempt_kinds_of(&h, &row.id).await;
    assert!(
        attempt_kinds.iter().any(|k| k == "manual_confirm"),
        "attempts 记录 manual_confirm:{attempt_kinds:?}"
    );
    // 人工动作审计
    let audited = manual_audit_count(&h, &order2, "confirm_shipment").await;
    assert!(audited >= 1, "confirm_shipment 审计留痕");

    // 5c 平台拒绝 → 人工断言留痕,不谎报平台确认
    h.fake.stage_snapshot(complete_snapshot("ORD-CF3").build());
    delivery.handle_payment("acct-1", "ORD-CF3").await.unwrap();
    h.fake
        .script_confirm(ConfirmOutcome::Rejected { safe_code: "BIZ_REJECT".into() });
    let order3 = order_id_of(&h, "ORD-CF3").await;
    let out = manual
        .confirm_shipment(manual_req(&order3, "key-cf3"))
        .await
        .unwrap();
    match &out {
        ManualOutcome::ConfirmShipment {
            platform_confirmed, ..
        } => assert!(!*platform_confirmed, "平台失败不谎报确认"),
        other => panic!("人工断言记录:{other:?}"),
    }
    let row3 = delivery_row(&h, "ORD-CF3").await;
    assert_eq!(row3.content_state, "accepted", "内容轴不受确认失败影响");
    assert_eq!(row3.confirmation_state, "rejected", "确认轴如实记录平台结果");
    let audited3 = manual_audit_count(&h, &order3, "confirm_shipment").await;
    assert!(audited3 >= 1, "平台失败仍留人工断言审计");
}

// ---- T067-6:单笔同步 ----

#[tokio::test]
async fn single_order_sync_returns_handle_outcome() {
    let h = harness().await;
    let order_id = seed_order(&h, "ORD-SS").await;
    h.fake.stage_snapshot(complete_snapshot("ORD-SS").build());
    let svc = sync_service(&h);

    let out = svc.sync_single("acct-1", &order_id).await.unwrap();
    assert!(matches!(out, SingleSyncOutcome::Delivered { .. }));
    assert_eq!(h.fake.sent_records().len(), 1);

    // 再次同步:已处理 → AlreadyHandled
    let again = svc.sync_single("acct-1", &order_id).await.unwrap();
    assert!(matches!(again, SingleSyncOutcome::AlreadyHandled));
    assert_eq!(h.fake.sent_records().len(), 1);

    // 账号不可用 → 403 语义
    seed_account(&h, "acct-2", "offline", true).await;
    let err = svc.sync_single("acct-2", &order_id).await.unwrap_err();
    assert!(matches!(err, OrderSyncError::AccountUnavailable));

    // 订单不存在 → 404 语义
    let missing = svc.sync_single("acct-1", "ord_none").await.unwrap_err();
    assert!(matches!(missing, OrderSyncError::NotFound));
}

// ---- 辅助 ----

use rusqlite::OptionalExtension;

#[allow(dead_code)]
fn unused_keeps_imports() {}

async fn order_id_of(h: &Harness, external: &str) -> String {
    let ext = external.to_string();
    h.db
        .call(move |conn| orders::find_by_external(conn, "xianyu", "acct-1", &ext))
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .id
}

async fn delivery_row(h: &Harness, external: &str) -> qing_delivery::adapters::sqlite::repos::deliveries::DeliveryRow {
    h.db
        .call({
            let external = external.to_string();
            move |conn| {
                qing_delivery::adapters::sqlite::repos::deliveries::find_initial_by_external(
                    conn, &external,
                )
            }
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn attempt_kinds_of(h: &Harness, delivery_id: &str) -> Vec<String> {
    let id = delivery_id.to_string();
    h.db
        .call(move |conn| -> rusqlite::Result<Vec<String>> {
            let mut stmt =
                conn.prepare("SELECT action_kind FROM attempts WHERE delivery_id = ?1")?;
            let rows = stmt
                .query_map(rusqlite::params![id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
        .unwrap()
}

async fn manual_audit_count(h: &Harness, order_id: &str, action: &str) -> i64 {
    let order = order_id.to_string();
    let act = action.to_string();
    h.db
        .call(move |conn| -> rusqlite::Result<i64> {
            conn.query_row(
                "SELECT COUNT(*) FROM manual_actions WHERE order_id = ?1 AND action = ?2",
                rusqlite::params![order, act],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap()
}

const _: fn() = || {
    // 引用保持导入(编译期断言用)
    let _ = qing_delivery::application::orders_sync::MAX_TRACE_PAGES;
};
