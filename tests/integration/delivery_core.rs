//! US1 交付内核集成测试(T036—T038):
//! 去重/并发/乱序、数量与 SKU、非法事实零发送、结果未知不重发、重试预算。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, deliveries, items, rules};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::delivery::service::{
    DeliveryService, HandleOutcome, MAX_AUTO_RETRIES,
};
use qing_delivery::application::ports::platform::{ConfirmOutcome, SendOutcome};
use qing_delivery::domain::crypto::{self, Aad};
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{FieldStatus, SnapshotBuilder, TradeType};
use qing_delivery::domain::sku::{SkuPart, combo_key};

const RULE_TEXT: &str = "资料链接:https://example.com/d/abc\n提取码:ab12\n打开后请保存,永久有效";

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

async fn harness(auto_confirm: bool) -> Harness {
    harness_with(auto_confirm, |_| {}).await
}

async fn harness_with(
    auto_confirm: bool,
    tweak_snapshot: impl FnOnce(&mut SnapshotBuilder),
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("it.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();

    // 账号:在线 + 运行 + 自动交付;监控起点早于付款
    db.call(move |conn| {
        let acct = accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        accounts::set_control(
            conn,
            &acct.id,
            "online",
            Some(true),
            Some(true),
            Some(auto_confirm),
            Some(1_600_000_000_000),
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();

    // 商品(单规格)+ 启用规则(冻结内容 v1)
    let key_clone = key.key;
    let key_id = key.key_id.clone();
    db.call(move |conn| {
        let item = items::upsert(
            conn,
            "item-1",
            "acct-1",
            "EXT-ITEM-1",
            "考研资料",
            "on_sale",
            "[]",
            "single",
        )?;
        let rule = rules::insert(conn, "rule-1", "acct-1", &item.id, "single", true)?;
        let aad = rules::rule_aad(&rule.id, 1);
        let envelope = crypto::seal(&key_clone, &key_id, &aad, RULE_TEXT.as_bytes());
        let digest = hex::encode(sha2_digest(RULE_TEXT.as_bytes()));
        rules::insert_content(
            conn,
            "rc-1",
            &rule.id,
            1,
            &envelope,
            &digest,
            RULE_TEXT.chars().count() as i64,
            RULE_TEXT.len() as i64,
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();

    // 订单快照(完整事实)
    let mut builder = complete_snapshot("ORD-1", "buyer-1");
    tweak_snapshot(&mut builder);
    let fake = FakeAdapter::new();
    fake.stage_snapshot(builder.build());

    Harness {
        db,
        key,
        fake,
        _dir: dir,
        _lock: lock,
    }
}

fn complete_snapshot(order: &str, buyer: &str) -> SnapshotBuilder {
    SnapshotBuilder {
        platform_order_id: order.into(),
        seller_id: FieldStatus::Verified("seller-1".into()),
        buyer_id: FieldStatus::Verified(buyer.into()),
        item_id: FieldStatus::Verified("EXT-ITEM-1".into()),
        trade_type: FieldStatus::Verified(TradeType::Ordinary),
        platform_state: FieldStatus::Verified(
            qing_delivery::domain::orders::snapshot::PlatformOrderState::PendingShip,
        ),
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

fn sha2_digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut out = [0u8; 32];
    out.copy_from_slice(&sha2::Sha256::digest(bytes));
    out
}

fn service(h: &Harness) -> DeliveryService<FakeAdapter> {
    DeliveryService::new(h.db.clone(), clone_key(&h.key), h.fake.clone())
}

fn clone_key(k: &DataKey) -> DataKey {
    DataKey {
        key_id: k.key_id.clone(),
        key: k.key,
    }
}

#[tokio::test]
async fn happy_path_delivers_once_with_proof_and_confirmation() {
    let h = harness(true).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let delivery_id = match out {
        HandleOutcome::Delivered { delivery_id } => delivery_id,
        other => panic!("应交付成功,实际 {other:?}"),
    };

    // 恰好一次发送,买家与原文精确匹配(含换行与中文)
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].buyer_id, "buyer-1");
    assert_eq!(sent[0].text, RULE_TEXT);

    // 双状态独立:内容 accepted + 平台确认 accepted,均持久化
    let row =
        h.db.call(move |conn| deliveries::get(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.content_state, "accepted");
    assert_eq!(row.confirmation_state, "accepted");
    assert_eq!(row.evidence_origin, "platform");
    assert_eq!(
        h.fake.confirm_calls().len(),
        1,
        "自动确认开启应发起一次确认"
    );
}

#[tokio::test]
async fn confirmation_off_leaves_content_delivered_independently() {
    let h = harness(false).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Delivered { delivery_id } = out else {
        panic!("自动确认关闭不阻止内容交付,实际 {out:?}")
    };
    let row =
        h.db.call(move |conn| deliveries::get(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.content_state, "accepted");
    assert_eq!(row.confirmation_state, "disabled", "确认轴保持 disabled");
    assert!(h.fake.confirm_calls().is_empty());
}

#[tokio::test]
async fn twenty_duplicates_and_concurrency_send_at_most_once() {
    let h = harness(false).await;
    let svc = service(&h);
    for _ in 0..20 {
        let _ = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    }
    assert_eq!(
        h.fake.sent_records().len(),
        1,
        "SC-003:重复通知每单最多一次"
    );

    // 并发:第二笔订单两路同时处理
    h.fake
        .stage_snapshot(complete_snapshot("ORD-2", "buyer-2").build());
    let (a, b) = tokio::join!(
        svc.handle_payment("acct-1", "ORD-2"),
        svc.handle_payment("acct-1", "ORD-2")
    );
    let _ = a.unwrap();
    let _ = b.unwrap();
    let ord2_sends = h
        .fake
        .sent_records()
        .iter()
        .filter(|r| r.order_id.contains("ORD-2") || r.buyer_id == "buyer-2")
        .count();
    assert_eq!(ord2_sends, 1, "并发竞争最多一次执行");
}

#[tokio::test]
async fn unknown_result_never_resends_even_after_rehandle() {
    let h = harness(false).await;
    h.fake.script_send(SendOutcome::Unknown {
        hint: Some("timeout".into()),
    });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!("应返回 Unknown,实际 {out:?}")
    };
    // 重放/再触发不得自动重发
    for _ in 0..3 {
        let again = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
        assert_eq!(again, HandleOutcome::AlreadyHandled, "未知结果禁止自动重发");
    }
    assert_eq!(h.fake.sent_records().len(), 1);
    let row =
        h.db.call(move |conn| deliveries::get(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.content_state, "unknown");
    assert_eq!(row.review_state, "required", "未知产生持久待处理(FR-020)");
}

#[tokio::test]
async fn retry_budget_exhausts_after_three_retries() {
    let h = harness(false).await;
    for _ in 0..(MAX_AUTO_RETRIES + 2) {
        h.fake
            .script_send(SendOutcome::NotSubmitted { retryable: true });
    }
    let svc = service(&h);
    // 初次失败:排定第 1 次重试
    let delivery_id = any_delivery_id(svc.handle_payment("acct-1", "ORD-1").await.unwrap());

    // 第 1、2 次重试失败:各自排定下一次(预算 3 次内)
    let mut scheduled = 1;
    for i in 0..(MAX_AUTO_RETRIES - 1) {
        let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
        if let HandleOutcome::NotSent {
            retry_scheduled, ..
        } = out
        {
            assert!(retry_scheduled, "预算内第 {} 次重试应排定", i + 2);
            scheduled += 1;
        }
    }
    assert_eq!(scheduled, MAX_AUTO_RETRIES, "共排定 3 次重试");
    // 第 3 次(最后一次)重试失败:预算耗尽,required,不再排定
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::NotSent {
        retry_scheduled, ..
    } = out
    else {
        panic!("耗尽后应 NotSent,实际 {out:?}")
    };
    assert!(!retry_scheduled);
    let total = h.fake.sent_records().len() as i64;
    assert_eq!(total, MAX_AUTO_RETRIES + 1, "初次 + 最多 3 次重试");
    let row =
        h.db.call(move |conn| deliveries::get(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.retry_count, MAX_AUTO_RETRIES + 1);
    assert_eq!(row.review_state, "required");
}

#[tokio::test]
async fn quantity_over_one_sends_single_text() {
    let h = harness_with(false, |b| b.quantity = FieldStatus::Verified(3)).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::Delivered { .. }),
        "多件每单一次,实际 {out:?}"
    );
    assert_eq!(h.fake.sent_records().len(), 1);
}

#[tokio::test]
async fn complete_sku_combo_matches_only_exact_rule() {
    // 规则绑定完整组合 p1=v1;p2=v2
    let h = harness_with(false, |b| {
        b.sku_parts = FieldStatus::Verified(vec![
            SkuPart {
                property_id: "p2".into(),
                value_id: "v2".into(),
                property_label: "版本".into(),
                value_label: "含解析".into(),
            },
            SkuPart {
                property_id: "p1".into(),
                value_id: "v1".into(),
                property_label: "格式".into(),
                value_label: "PDF".into(),
            },
        ]);
        b.sku_single = false;
    })
    .await;
    // 原规则(single)不匹配;同快照应 Ineligible(NoRule)而不是任选
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Ineligible { reason, .. } = out else {
        panic!("无组合规则不得发送,实际 {out:?}")
    };
    assert!(
        reason.contains("无启用规则"),
        "原因应指明无规则,实际 {reason}"
    );
    assert_eq!(h.fake.sent_records().len(), 0);

    // 补建完全一致组合的规则后可交付
    let combo = combo_key(&[
        SkuPart {
            property_id: "p1".into(),
            value_id: "v1".into(),
            property_label: String::new(),
            value_label: String::new(),
        },
        SkuPart {
            property_id: "p2".into(),
            value_id: "v2".into(),
            property_label: String::new(),
            value_label: String::new(),
        },
    ])
    .unwrap();
    let key_clone = h.key.key;
    let key_id = h.key.key_id.clone();
    h.db.call(move |conn| {
        let item = items::find_by_external(conn, "acct-1", "EXT-ITEM-1")?.unwrap();
        let rule = rules::insert(conn, "rule-2", "acct-1", &item.id, &combo, true)?;
        let aad = rules::rule_aad(&rule.id, 1);
        let envelope = crypto::seal(&key_clone, &key_id, &aad, RULE_TEXT.as_bytes());
        let digest = hex::encode(sha2_digest(RULE_TEXT.as_bytes()));
        rules::insert_content(
            conn,
            "rc-2",
            &rule.id,
            1,
            &envelope,
            &digest,
            RULE_TEXT.chars().count() as i64,
            RULE_TEXT.len() as i64,
        )
    })
    .await
    .unwrap()
    .unwrap();
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::Delivered { .. }),
        "精确组合应交付,实际 {out:?}"
    );
}

#[tokio::test]
async fn invalid_facts_never_send() {
    // 未付款
    let h = harness_with(false, |b| {
        b.platform_state = FieldStatus::Verified(
            qing_delivery::domain::orders::snapshot::PlatformOrderState::Unpaid,
        );
    })
    .await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::Ineligible { .. }),
        "未付款不得发送:{out:?}"
    );
    assert_eq!(h.fake.sent_records().len(), 0);

    // 不支持交易类型(小刀)
    let h2 = harness_with(false, |b| {
        b.trade_type = FieldStatus::Verified(TradeType::Bargain);
    })
    .await;
    let svc2 = service(&h2);
    let out = svc2.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::Ineligible { .. }),
        "小刀不得当普通单:{out:?}"
    );
    assert_eq!(h2.fake.sent_records().len(), 0);

    // 缺数量(缺失不是默认 1)
    let h3 = harness_with(false, |b| b.quantity = FieldStatus::Missing).await;
    let svc3 = service(&h3);
    let out = svc3.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::Ineligible { .. }),
        "缺数量不得发送:{out:?}"
    );
    assert_eq!(h3.fake.sent_records().len(), 0);
}

#[tokio::test]
async fn rejected_is_permanent_not_retried() {
    let h = harness(false).await;
    h.fake.script_send(SendOutcome::Rejected {
        safe_code: "FORBIDDEN".into(),
    });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::NotSent {
        retry_scheduled,
        reason,
        ..
    } = out
    else {
        panic!("拒绝应 NotSent,实际 {out:?}")
    };
    assert!(!retry_scheduled, "永久拒绝不进入自动预算");
    assert_eq!(reason.as_deref(), Some("FORBIDDEN"));
    assert_eq!(h.fake.sent_records().len(), 1);
}

#[tokio::test]
async fn confirmation_failure_never_resends_content() {
    let h = harness(true).await;
    h.fake.script_confirm(ConfirmOutcome::Rejected {
        safe_code: "NOT_ALLOWED".into(),
    });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Delivered { delivery_id } = out else {
        panic!("确认失败不影响内容结果:{out:?}")
    };
    let row =
        h.db.call(move |conn| deliveries::get(conn, &delivery_id))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(row.content_state, "accepted", "内容保持已交付");
    assert_eq!(row.confirmation_state, "rejected", "确认失败只影响确认轴");
    assert_eq!(h.fake.sent_records().len(), 1, "确认失败不得触发再次发送");
}

/// 从任一产生任务的结果中取交付 ID。
fn any_delivery_id(out: HandleOutcome) -> String {
    match out {
        HandleOutcome::Delivered { delivery_id }
        | HandleOutcome::NotSent { delivery_id, .. }
        | HandleOutcome::Unknown { delivery_id }
        | HandleOutcome::Ineligible { delivery_id, .. } => delivery_id,
        other => panic!("无交付 ID,实际 {other:?}"),
    }
}

/// AAD 绑定验证:快照密文不能用规则 AAD 打开(用途隔离)。
#[tokio::test]
async fn snapshot_envelope_is_aad_bound() {
    let h = harness(false).await;
    let svc = service(&h);
    svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let row =
        h.db.call(|conn| deliveries::find_initial_by_external(conn, "ORD-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    let snap =
        h.db.call(move |conn| deliveries::get_snapshot(conn, &row.content_snapshot_id.unwrap()))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    let wrong_aad = Aad {
        purpose: "rule_content".into(),
        entity_id: "rule-1".into(),
        content_version: Some(1),
    };
    assert!(
        crypto::open(&h.key.key, &wrong_aad, &snap.envelope).is_err(),
        "快照密文必须绑定快照 AAD"
    );
    let right_aad = Aad {
        purpose: "content_snapshot".into(),
        entity_id: snap.id.clone(),
        content_version: None,
    };
    assert!(crypto::open(&h.key.key, &right_aad, &snap.envelope).is_ok());
}
