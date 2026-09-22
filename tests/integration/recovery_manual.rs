//! US4 集成测试(T065—T073 场景):
//! 启动恢复 unknown 化、120 秒补偿窗口、人工补发/确认/终止/接管、
//! 幂等键重放、自动/人工互斥、退款拒绝、控制竞争。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, deliveries, items, orders, rules};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::delivery::service::{DeliveryService, HandleOutcome};
use qing_delivery::application::manual::actions::{
    ManualError, ManualOutcome, ManualRequest, ManualService,
};
use qing_delivery::application::ports::platform::SendOutcome;
use qing_delivery::application::recovery::boot::BootRecovery;
use qing_delivery::application::recovery::compensate::CompensationService;
use qing_delivery::domain::crypto;
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{
    FieldStatus, PlatformOrderState, SnapshotBuilder, TradeType,
};
use qing_delivery::domain::time_util::utc_now_ms;

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

async fn harness(monitor_since: i64) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us4.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    let key_for_seed = key.key;
    let key_id = key.key_id.clone();
    db.call(move |conn| {
        // manual_actions.admin_id 外键:预置测试管理员
        conn.execute(
            "INSERT OR IGNORE INTO admin(id, username, password_hash, created_at, password_changed_at)
             VALUES ('adm_test', 'admin', 'seed-hash', 0, 0)",
            [],
        )?;
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        accounts::set_control(conn, "acct-1", "online", Some(true), Some(true), Some(true), Some(monitor_since))?;
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

fn service(h: &Harness) -> DeliveryService<FakeAdapter> {
    DeliveryService::new(h.db.clone(), clone_key(&h.key), h.fake.clone())
}

fn manual(h: &Harness, delivery: DeliveryService<FakeAdapter>) -> ManualService<FakeAdapter> {
    ManualService::new(h.db.clone(), clone_key(&h.key), delivery)
}

fn clone_key(k: &DataKey) -> DataKey {
    DataKey {
        key_id: k.key_id.clone(),
        key: k.key,
    }
}

fn req(order: &str, risk: bool) -> ManualRequest {
    ManualRequest {
        admin_id: "adm_test".into(),
        account_id: "acct-1".into(),
        order_db_id: order.into(),
        reason: "买家反馈未收到,人工核对后补发".into(),
        risk_confirmed: risk,
        idempotency_key: format!("idem-{order}-{}", utc_now_ms()),
    }
}

/// 直接种一条 pending_ship 订单(带付款事实与观察时间)并返回内部 ID。
async fn seed_order(h: &Harness, external: &str, observed_at: Option<i64>) -> String {
    let external_owned = external.to_string();
    let local_id = format!("ord_{external}");
    h.db.call(move |conn| {
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
            "UPDATE orders SET platform_status='pending_ship', observed_at=?2 WHERE id=?1",
            rusqlite::params![row.id, observed_at],
        )?;
        Ok::<_, rusqlite::Error>(row.id)
    })
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn boot_recovery_marks_dispatching_unknown() {
    let h = harness(1_600_000_000_000).await;
    let svc = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-B").build());
    h.fake.script_send(SendOutcome::Unknown { hint: None });
    let out = svc.handle_payment("acct-1", "ORD-B").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!()
    };
    let delivery_for_db = delivery_id.clone();
    let delivery_for_lookup = delivery_id.clone();
    let delivery_final = delivery_id.clone();

    // 模拟"结果记录前崩溃":把已分类状态回置为 dispatching 场景——
    // 真实崩溃发生在分类前;此处直接造一条 dispatching attempt 验证启动恢复
    let db = h.db.clone();
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO attempts(id, delivery_id, action_kind, sequence, request_id, state, prepared_at)
             VALUES ('att-boot', ?1, 'initial', 99, 'req-boot', 'dispatching', 1)",
            rusqlite::params![delivery_for_db],
        )?;
        conn.execute(
            "UPDATE deliveries SET content_state='dispatching' WHERE id=?1",
            rusqlite::params![delivery_for_lookup],
        )
    })
    .await
    .unwrap()
    .unwrap();

    let report = BootRecovery::new(h.db.clone()).run().await.unwrap();
    assert_eq!(report.unknown_content, 1);
    let row = db
        .call(move |conn| deliveries::get(conn, &delivery_final))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(row.content_state, "unknown");
    assert_eq!(
        row.review_state, "required",
        "重启不清除未知、不自动重发(FR-020)"
    );
}

#[tokio::test]
async fn compensation_respects_120s_window() {
    let h = harness(1_600_000_000_000).await;
    let now = utc_now_ms();
    // 3 分钟前观察:可补偿
    let old = seed_order(&h, "ORD-C1", Some(now - 180_000)).await;
    // 刚观察:等待窗口内
    seed_order(&h, "ORD-C2", Some(now)).await;
    h.fake.stage_snapshot(complete_snapshot("ORD-C1").build());
    h.fake.stage_snapshot(complete_snapshot("ORD-C2").build());

    let delivery = service(&h);
    let comp = CompensationService::new(h.db.clone(), delivery);
    let report = comp.run_once().await.unwrap();
    assert_eq!(report.candidates_total, 2);
    assert_eq!(report.waiting, 1, "120 秒窗口内不补偿");
    assert_eq!(report.handled, 1);
    // 补偿交付成功
    let delivered =
        h.db.call(move |conn| deliveries::find_initial(conn, &old))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(delivered.content_state, "accepted");
    assert_eq!(h.fake.sent_records().len(), 1);
    // 已有任务不再是候选(绝不建第二初始任务)
    let again = comp.run_once().await.unwrap();
    assert_eq!(again.candidates_total, 1, "只剩等待窗口内那笔");
}

#[tokio::test]
async fn manual_resend_uses_frozen_snapshot_and_idempotent() {
    let h = harness(1_600_000_000_000).await;
    let delivery = service(&h);
    // 首次发送结果未知 → 人工补发
    h.fake.stage_snapshot(complete_snapshot("ORD-M").build());
    h.fake.script_send(SendOutcome::Unknown { hint: None });
    let out = delivery.handle_payment("acct-1", "ORD-M").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!()
    };
    let order_db = format!("ord_{delivery_id}");
    // 取真实订单内部 ID
    let order_id =
        h.db.call(move |conn| orders::get_order_external_reverse(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();

    let manual = manual(&h, service(&h));
    // 风险未确认 → 拒绝
    assert!(matches!(
        manual.resend(req(&order_id, false)).await,
        Err(ManualError::RiskConfirmationRequired)
    ));
    // 确认后补发成功:同一原文
    let key = format!("fixed-key-{}", utc_now_ms());
    let mut r = req(&order_id, true);
    r.idempotency_key = key.clone();
    let out = manual.resend(r).await.unwrap();
    assert!(matches!(out, ManualOutcome::ResentAccepted));
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 2, "初次(未知)+ 补发");
    assert_eq!(sent[1].text, RULE_TEXT, "补发必须用冻结快照原文");

    // 同幂等键重放:不再发送
    let mut r2 = req(&order_id, true);
    r2.idempotency_key = key;
    let replay = manual.resend(r2).await.unwrap();
    assert!(matches!(replay, ManualOutcome::IdempotentReplay));
    assert_eq!(h.fake.sent_records().len(), 2, "同键重放不产生新动作");
    let _ = order_db;
}

#[tokio::test]
async fn mark_received_records_manual_evidence_only() {
    let h = harness(1_600_000_000_000).await;
    let delivery = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-R").build());
    h.fake.script_send(SendOutcome::Unknown { hint: None });
    let out = delivery.handle_payment("acct-1", "ORD-R").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!()
    };
    let delivery_for_lookup = delivery_id.clone();
    let order_id =
        h.db.call(move |conn| orders::get_order_external_reverse(conn, &delivery_for_lookup))
            .await
            .unwrap()
            .unwrap()
            .unwrap();

    let manual = manual(&h, service(&h));
    let out = manual.mark_received(req(&order_id, false)).await.unwrap();
    assert!(matches!(out, ManualOutcome::MarkedReceived));
    let row_probe = delivery_id.clone();
    let row =
        h.db.call(move |conn| deliveries::get(conn, &row_probe))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.content_state, "accepted");
    assert_eq!(row.evidence_origin, "manual", "人工证据不伪装平台标识");
    // 平台确认轴保持 unknown(人工确认不自动触发平台确认)
    assert_eq!(
        row.confirmation_state, "disabled",
        "内容未知时确认轴从未发起,保持 disabled"
    );
    assert_eq!(h.fake.confirm_calls().len(), 0);
    assert_eq!(h.fake.sent_records().len(), 1, "确认已收到不发消息");
}

#[tokio::test]
async fn refunded_order_rejects_resend_and_terminate_guards() {
    let h = harness(1_600_000_000_000).await;
    let delivery = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-X").build());
    // 正常交付后订单转退款 → 补发被拒
    delivery.handle_payment("acct-1", "ORD-X").await.unwrap();
    let order_id =
        h.db.call(|conn| orders::find_by_external(conn, "xianyu", "acct-1", "ORD-X"))
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .id;
    let order_for_update = order_id.clone();
    h.db.call(move |conn| {
        conn.execute(
            "UPDATE orders SET platform_status='refunded' WHERE id=?1",
            rusqlite::params![order_for_update],
        )
    })
    .await
    .unwrap()
    .unwrap();
    let manual = manual(&h, service(&h));
    let order_for_resend = order_id.clone();
    assert!(matches!(
        manual.resend(req(&order_for_resend, true)).await,
        Err(ManualError::RefundedOrCompleted)
    ));

    // 终止:已有提交动作(accepted attempt)→ 拒绝,结果必须保留
    assert!(matches!(
        manual.terminate(req(&order_id, true)).await,
        Err(ManualError::NotAllowed(_))
    ));
}

#[tokio::test]
async fn historical_takeover_via_explicit_path() {
    // 监控起点晚于付款:自动拒绝,显式接管允许(其余核验不放宽)
    let h = harness(1_900_000_000_000).await;
    let delivery = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-H").build());
    let out = delivery.handle_payment("acct-1", "ORD-H").await.unwrap();
    let HandleOutcome::Ineligible { reason, .. } = out else {
        panic!("{out:?}")
    };
    assert!(reason.contains("历史"), "自动路径必须拒绝:{reason}");
    assert_eq!(h.fake.sent_records().len(), 0);

    let order_id =
        h.db.call(|conn| orders::find_by_external(conn, "xianyu", "acct-1", "ORD-H"))
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .id;
    let manual = manual(&h, service(&h));
    let mut r = req(&order_id, true);
    r.reason = "接入前订单,已核对买家付款事实,显式接管".into();
    let out = manual.takeover(r).await.unwrap();
    assert!(
        matches!(out, ManualOutcome::Takeover(ref t) if matches!(**t, HandleOutcome::Delivered { .. }))
    );
    assert_eq!(h.fake.sent_records().len(), 1);
}

#[tokio::test]
async fn auto_delivery_off_blocks_manual_resend() {
    let h = harness(1_600_000_000_000).await;
    let delivery = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-S").build());
    h.fake.script_send(SendOutcome::Unknown { hint: None });
    let out = delivery.handle_payment("acct-1", "ORD-S").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!()
    };
    let order_id =
        h.db.call(move |conn| orders::get_order_external_reverse(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();

    // 关闭自动交付 → 人工补发同样被阻止(spec R7)
    let db = h.db.clone();
    db.call(|conn| accounts::set_control(conn, "acct-1", "online", None, Some(false), None, None))
        .await
        .unwrap()
        .unwrap();
    let manual = manual(&h, service(&h));
    assert!(matches!(
        manual.resend(req(&order_id, true)).await,
        Err(ManualError::AutoDeliveryOff)
    ));
}

/// V07 子集:暂停与自动交付并发竞争——无论交错,至多一次发送且状态一致。
#[tokio::test]
async fn pause_race_with_delivery_sends_at_most_once() {
    let h = harness(1_600_000_000_000).await;
    let svc = service(&h);
    h.fake.stage_snapshot(complete_snapshot("ORD-P").build());
    let db = h.db.clone();
    let (a, b) = tokio::join!(
        async { svc.handle_payment("acct-1", "ORD-P").await },
        async {
            // 竞争暂停(忽略结果:赢或输都合法)
            let db = db.clone();
            db.call(move |conn| -> rusqlite::Result<i64> {
                let row =
                    accounts::get(conn, "acct-1")?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                accounts::set_control(conn, "acct-1", "paused", Some(false), None, None, None)?;
                Ok(row.version)
            })
            .await
        }
    );
    let _ = b.unwrap().unwrap();
    let out = a.unwrap();
    let sends = h
        .fake
        .sent_records()
        .iter()
        .filter(|r| r.text == RULE_TEXT)
        .count();
    assert!(
        sends <= 1,
        "暂停竞争至多一次发送,实际 {sends}(outcome={out:?})"
    );
}

/// T063:追溯扫描发现漏单 → 逐笔核验补建;覆盖缺口持久化。
#[tokio::test]
async fn trace_scan_discovers_missed_order() {
    let h = harness(1_600_000_000_000).await;
    h.fake
        .stage_trace(vec!["ORD-T1".into(), "ORD-T2".into()], false);
    h.fake.stage_snapshot(complete_snapshot("ORD-T1").build());
    h.fake.stage_snapshot(complete_snapshot("ORD-T2").build());
    let delivery = service(&h);
    let scan = qing_delivery::application::recovery::trace::TraceScanService::new(
        h.db.clone(),
        DeliveryService::new(h.db.clone(), clone_key(&h.key), h.fake.clone()),
    );
    let _ = delivery;
    let report = scan.run_once("acct-1", &req_ctx()).await.unwrap();
    assert_eq!(report.candidates, 2);
    assert_eq!(report.handled, 2, "两笔候选逐笔核验交付");
    assert!(!report.coverage.coverage_complete, "覆盖缺口如实上报");
    assert_eq!(h.fake.sent_records().len(), 2);
    // 重复扫描:已处理订单不再发送
    h.fake.stage_trace(vec!["ORD-T1".into()], true);
    let again = scan.run_once("acct-1", &req_ctx()).await.unwrap();
    assert_eq!(again.handled, 0, "已交付订单返回 AlreadyHandled,不重复");
    assert_eq!(h.fake.sent_records().len(), 2, "SC-005:漏单恢复不重复发送");
}

fn req_ctx() -> qing_delivery::application::ports::platform::RequestContext {
    qing_delivery::application::ports::platform::RequestContext {
        operation_id: "op".into(),
        account_id: "acct-1".into(),
        credential_generation: 0,
        control_generation: 0,
        deadline_ms: 0,
    }
}
