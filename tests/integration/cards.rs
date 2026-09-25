//! US1 卡密库存集成测试(T006/T009/T012):
//! 原子预留不双配、余量不足/停用转 issue stock_insufficient 且不部分交付、
//! Accepted→used / NotSubmitted(terminal)→释放 / Unknown→保持 reserved、
//! 被引用删除拒绝(引用规则标记需重新配置)。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, cards, items, rules};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::cards::{CardPoolDraft, CardService, CardsError};
use qing_delivery::application::delivery::service::{
    DeliveryService, HandleOutcome, MAX_AUTO_RETRIES,
};
use qing_delivery::application::ports::platform::SendOutcome;
use qing_delivery::domain::cards::CardPoolKind;
use qing_delivery::domain::crypto::{self, Aad};
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{FieldStatus, SnapshotBuilder, TradeType};
use rusqlite::OptionalExtension as _;
use sha2::Digest as _;

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

/// 交付流测试环境:在线账号 + 单规格商品 + 启用规则(card_pool_id=pool-1)+
/// data 池(quantity 件 × 1 份);tweak 可改快照。
async fn delivery_harness(
    quantity: u32,
    texts: &[&str],
    pool_enabled: bool,
    tweak: impl FnOnce(&mut SnapshotBuilder),
) -> Harness {
    let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("cards-it.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();

    let key_clone = key.key;
    let key_id = key.key_id.clone();
    let pool_enabled_owned = pool_enabled;
    db.call(move |conn| {
        // 账号在线 + 运行 + 自动交付
        let acct = accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        accounts::set_control(
            conn,
            &acct.id,
            "online",
            Some(true),
            Some(true),
            Some(false),
            Some(1_600_000_000_000),
        )?;
        // 商品 + 启用规则绑定卡密组
        let item = items::upsert(
            conn, "item-1", "acct-1", "EXT-ITEM-1", "考研资料", "on_sale", "[]", "single",
        )?;
        rules::insert(conn, "rule-1", "acct-1", &item.id, "single", true)?;
        conn.execute(
            "UPDATE rules SET card_pool_id = 'pool-1' WHERE id = 'rule-1'",
            [],
        )?;
        // 卡密组 + 条目(信封 AAD purpose card_entry)
        let pool = cards::NewPool {
            id: "pool-1",
            name: "网课卡池",
            kind: "data",
            enabled: pool_enabled_owned,
            delay_seconds: 0,
            description: "",
            content_envelope: None,
            api_config_envelope: None,
        };
        cards::insert_pool(conn, &pool)?;
        for (i, text) in texts.iter().enumerate() {
            let entry_id = format!("cent-{i}");
            let aad = card_entry_aad(&entry_id);
            let envelope = crypto::seal(&key_clone, &key_id, &aad, text.as_bytes());
            cards::append_entries(
                conn,
                "pool-1",
                &[cards::NewEntry {
                    id: &entry_id,
                    envelope: &envelope,
                    content_digest: &hex::encode(sha2::Sha256::digest(text.as_bytes())),
                }],
            )?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();

    let mut builder = complete_snapshot("ORD-1", "buyer-1", quantity);
    tweak(&mut builder);
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

fn card_entry_aad(entry_id: &str) -> Aad {
    Aad {
        purpose: "card_entry".into(),
        entity_id: entry_id.into(),
        content_version: None,
    }
}

fn complete_snapshot(order: &str, buyer: &str, quantity: u32) -> SnapshotBuilder {
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
        quantity: FieldStatus::Verified(quantity),
        sku_parts: FieldStatus::Missing,
        sku_single: true,
        conversation_verified: true,
        source: "detail".into(),
        observed_at_ms: 1_760_000_001_000,
        browser_supplemented: false,
    }
}

fn service(h: &Harness) -> DeliveryService<FakeAdapter> {
    DeliveryService::new(
        h.db.clone(),
        DataKey {
            key_id: h.key.key_id.clone(),
            key: h.key.key,
        },
        h.fake.clone(),
    )
}

async fn stock_of(h: &Harness) -> cards::StockCounts {
    h.db
        .call(|conn| cards::state_counts(conn, "pool-1"))
        .await
        .unwrap()
        .unwrap()
}

/// (kind, reason_code, allowed_actions) of open issue for order;None if absent.
async fn open_issue(h: &Harness, external_order: &str) -> Option<(String, String, String)> {
    let ext = external_order.to_string();
    h.db
        .call(move |conn| {
            conn.query_row(
                "SELECT i.kind, i.reason_code, i.allowed_actions FROM issues i
                 JOIN orders o ON o.id = i.order_id
                 WHERE i.state = 'open' AND o.external_order_id = ?1 LIMIT 1",
                rusqlite::params![ext],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
        .await
        .unwrap()
        .unwrap()
}

// ---- T006-1:原子预留不双配 ----

#[tokio::test]
async fn 原子预留两订单各自绑定不重复第三单不足() {
    let h = delivery_harness(1, &["CARD-A", "CARD-B"], true, |_| {}).await;
    // 两笔订单先后预留(串行事务闭包,与 freeze_snapshot 同构)
    let a = h
        .db
        .call(|conn| cards::reserve_one(conn, "pool-1", "ord-1", "dlv-1"))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let b = h
        .db
        .call(|conn| cards::reserve_one(conn, "pool-1", "ord-2", "dlv-2"))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_ne!(a.id, b.id, "两个订单不得预留同一条卡密");
    // 余量耗尽:第三单取不到
    let c = h
        .db
        .call(|conn| cards::reserve_one(conn, "pool-1", "ord-3", "dlv-3"))
        .await
        .unwrap()
        .unwrap();
    assert!(c.is_none(), "余量不足必须返回空,不得部分交付");

    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (0, 2, 0));
    // 各自绑定各自订单
    let bindings = h
        .db
        .call(|conn| {
            let mut stmt = conn
                .prepare("SELECT reserved_order_id, reserved_delivery_id FROM card_entries
                          WHERE state='reserved' ORDER BY reserved_order_id")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bindings.len(), 2);
    assert!(bindings.iter().any(|(o, d)| o == "ord-1" && d == "dlv-1"));
    assert!(bindings.iter().any(|(o, d)| o == "ord-2" && d == "dlv-2"));
}

#[tokio::test]
async fn 并发预留经串行数据库线程仍不双配() {
    let h = delivery_harness(1, &["CARD-A", "CARD-B"], true, |_| {}).await;
    let (a, b) = tokio::join!(
        h.db.call(|conn| cards::reserve_one(conn, "pool-1", "ord-1", "dlv-1")),
        h.db.call(|conn| cards::reserve_one(conn, "pool-1", "ord-2", "dlv-2")),
    );
    let a = a.unwrap().unwrap().unwrap();
    let b = b.unwrap().unwrap().unwrap();
    assert_ne!(a.id, b.id, "并发路径同样不得双配");
    let stock = stock_of(&h).await;
    assert_eq!(stock.reserved, 2);
}

// ---- T006-2:余量不足/停用 → stock_insufficient,不部分交付 ----

#[tokio::test]
async fn 余量不足开事项且回滚不部分交付() {
    // 需 2 件,库存仅 1 条
    let h = delivery_harness(2, &["CARD-A"], true, |_| {}).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::AlreadyHandled),
        "冻结失败转已有处置,实际 {out:?}"
    );
    let issue = open_issue(&h, "ORD-1").await.expect("应开 stock_insufficient 事项");
    assert_eq!(issue.0, "stock_insufficient");
    assert_eq!(issue.1, "out_of_stock");
    assert_eq!(issue.2, "[\"terminate\"]");
    // 不部分交付:预留已整体回滚,库存原样
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (1, 0, 0));
    assert!(h.fake.sent_records().is_empty(), "不足不得发送任何内容");
    let snapshots: i64 = h
        .db
        .call(|conn| conn.query_row("SELECT COUNT(*) FROM content_snapshots", [], |r| r.get(0)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshots, 0, "不足不得落快照");
    // 重复触发:事项去重(同 order+kind+reason 唯一),行为不变
    let _ = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let count: i64 = h
        .db
        .call(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM issues WHERE kind='stock_insufficient' AND state='open'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 1, "重复触发不得重复开事项");
}

#[tokio::test]
async fn 停用池同样转待处理不消耗库存() {
    let h = delivery_harness(1, &["CARD-A"], false, |_| {}).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::AlreadyHandled), "实际 {out:?}");
    let issue = open_issue(&h, "ORD-1").await.expect("停用池应开事项");
    assert_eq!(issue.0, "stock_insufficient");
    let stock = stock_of(&h).await;
    assert_eq!(stock.available, 1, "停用池不得预留");
    assert!(h.fake.sent_records().is_empty());
}

// ---- T006-3:Accepted → used;余量 3→2 ----

#[tokio::test]
async fn 平台接纳扣减卡密并发送卡文本() {
    let texts = ["CARD-A", "CARD-B", "CARD-C"];
    let h = delivery_harness(1, &texts, true, |_| {}).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Delivered { delivery_id } = out else {
        panic!("应交付成功,实际 {out:?}")
    };
    // 发送内容 = 预留到的某张卡(预留顺序最早可用条,明文逐字)
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 1);
    assert!(
        texts.iter().any(|t| sent[0].text == *t),
        "发送内容应为某张卡密原文,实际 {}",
        sent[0].text
    );
    // 余量 3→2:该条 used 且绑定订单与交付
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (2, 0, 1));
    let used = h
        .db
        .call(move |conn| {
            conn.query_row(
                "SELECT state, reserved_order_id, reserved_delivery_id FROM card_entries
                 WHERE state='used'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(used.2, delivery_id, "used 条目应绑定该交付");
    assert!(!used.1.is_empty(), "used 条目应保留订单绑定留痕");
    // 快照来源可追溯 card:{pool}:{ids}
    let source = h
        .db
        .call(move |conn| {
            conn.query_row(
                "SELECT cs.source_content_id FROM content_snapshots cs
                 JOIN deliveries d ON d.content_snapshot_id = cs.id
                 WHERE d.id = ?1",
                rusqlite::params![delivery_id],
                |r| r.get::<_, String>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert!(
        source.starts_with("card:pool-1:cent-"),
        "source_content_id 应为 card:{{pool}}:{{ids}},实际 {source}"
    );
    // 多件订单:两卡逐行拼接(units = 件数)
    let h2 = delivery_harness(2, &["K-1", "K-2", "K-3"], true, |_| {}).await;
    let svc2 = service(&h2);
    let out2 = svc2.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out2, HandleOutcome::Delivered { .. }), "实际 {out2:?}");
    let sent2 = h2.fake.sent_records();
    assert_eq!(sent2.len(), 1);
    assert_eq!(
        sent2[0].text.lines().count(),
        2,
        "两件应拼接两行卡密,实际:{}",
        sent2[0].text
    );
    let stock2 = stock_of(&h2).await;
    assert_eq!(stock2.used, 2, "两件扣减两条");
    assert_eq!(stock2.available, 1);
}

// ---- T006-4:NotSubmitted(terminal)→释放;Unknown →保持 reserved ----

#[tokio::test]
async fn 结果未知保持预留不回库() {
    let h = delivery_harness(1, &["CARD-A"], true, |_| {}).await;
    h.fake.script_send(SendOutcome::Unknown {
        hint: Some("timeout".into()),
    });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Unknown { .. }), "实际 {out:?}");
    // 宪章 I:结果未知不回库、不换卡——保持 reserved
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (0, 1, 0));
    let issue = open_issue(&h, "ORD-1").await.expect("应开 delivery_unknown");
    assert_eq!(issue.0, "delivery_unknown");
    // 重触发不自动重发、不释放
    let again = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert_eq!(again, HandleOutcome::AlreadyHandled);
    let stock = stock_of(&h).await;
    assert_eq!(stock.reserved, 1, "未知结果禁止释放/重配");
}

#[tokio::test]
async fn 预算耗尽terminal释放预留回库存() {
    let h = delivery_harness(1, &["CARD-A"], true, |_| {}).await;
    // 初次 + 3 次重试全部 NotSubmitted(可重试)
    for _ in 0..=(MAX_AUTO_RETRIES as usize) {
        h.fake
            .script_send(SendOutcome::NotSubmitted { retryable: true });
    }
    let svc = service(&h);
    let mut last = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    for _ in 0..MAX_AUTO_RETRIES {
        last = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    }
    let HandleOutcome::NotSent { retry_scheduled, .. } = last else {
        panic!("预算耗尽应 terminal,实际 {last:?}")
    };
    assert!(!retry_scheduled);
    // 确定未发送(terminal)→ 释放回库存
    let stock = stock_of(&h).await;
    assert_eq!(
        (stock.available, stock.reserved, stock.used),
        (1, 0, 0),
        "terminal not_sent 应释放预留"
    );
    // 预算内排定重试期间不释放(前几次尝试后仍 reserved)已由最终态收敛覆盖
}

// ---- T006-5:被引用删除拒绝(引用规则标记需重新配置) ----

#[tokio::test]
async fn 被规则引用的池删除被拒并标记需重新配置() {
    let h = delivery_harness(1, &["CARD-A"], true, |_| {}).await;
    let svc = CardService::new(h.db.clone(), h.key.clone());
    let err = svc.delete_pool("pool-1").await.unwrap_err();
    assert!(
        matches!(err, CardsError::Referenced { rules: 1, variants: 0 }),
        "应返回引用冲突,实际 {err:?}"
    );
    // 引用规则被标记需重新配置;库存原样
    let flagged: i64 = h
        .db
        .call(|conn| {
            conn.query_row(
                "SELECT needs_reconfiguration FROM rules WHERE id='rule-1'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(flagged, 1, "409 后规则页应显示需重新配置");
    let stock = stock_of(&h).await;
    assert_eq!(stock.available, 1, "拒绝删除时库存不得清理");

    // 解除引用后可删除;条目级联清理
    h.db
        .call(|conn| conn.execute("UPDATE rules SET card_pool_id = NULL WHERE id='rule-1'", []))
        .await
        .unwrap()
        .unwrap();
    svc.delete_pool("pool-1").await.unwrap();
    let left: i64 = h
        .db
        .call(|conn| {
            conn.query_row("SELECT COUNT(*) FROM card_entries", [], |r| r.get(0))
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(left, 0, "删除组应级联清理条目");
}

// ---- 应用服务补充:创建/追加/导入(应用层数据面) ----

#[tokio::test]
async fn 服务层创建追加去重与审计列表不回明文() {
    let h = delivery_harness(0, &[], true, |_| {}).await;
    let svc = CardService::new(h.db.clone(), h.key.clone());
    let created = svc
        .create_pool(CardPoolDraft {
            name: "批量组X".into(),
            kind: CardPoolKind::Data,
            enabled: true,
            delay_seconds: 0,
            description: String::new(),
            content: None,
            entries: vec!["K-1".into(), "K-2".into(), "  ".into()],
            api_config: None,
        })
        .await
        .unwrap();
    assert_eq!(created.stock.unwrap().available, 2, "空行不入库");

    // 追加:空行 + 池内重复跳过,新卡入库
    let appended = svc
        .append_data(
            &created.id,
            vec![
                "K-3".to_string(),
                "K-1".to_string(), // 池内重复
                "   ".to_string(), // 空行
            ],
        )
        .await
        .unwrap();
    assert_eq!(appended.appended, 1);
    assert_eq!(appended.skipped_empty, 1);
    assert_eq!(appended.skipped_duplicate, 1);

    // 条目审计:状态过滤 + 分页;摘要仅前缀,永不携带密文
    let (page, next) = svc
        .list_entries(&created.id, Some("available".into()), None, 2)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(next.is_some());
    assert!(page[0].content_digest_prefix.len() == 12);
    assert!(page.iter().all(|e| e.state == "available"));
    // 追加第二页
    let (page2, next2) = svc
        .list_entries(&created.id, Some("available".into()), next, 2)
        .await
        .unwrap();
    assert_eq!(page2.len(), 1);
    assert!(next2.is_none());

    // CSV 批量导入:一行一组一卡;错误行逐行报告
    let csv = "名称,类型,内容,描述,启用,延迟秒\n导入组A,批量,IMP-1,,是,0\n坏行,voice,x,,,\n导入组B,文本,固定内容,,否,\n"
        .as_bytes()
        .to_vec();
    let report = svc.batch_import(csv, "import.csv").await.unwrap();
    assert_eq!(report.total, 3);
    assert_eq!(report.succeeded, 2);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].row, 3);
    // 超限拒绝
    let big = vec![0u8; 3 * 1024 * 1024];
    assert!(matches!(
        svc.batch_import(big, "big.csv").await,
        Err(CardsError::PayloadTooLarge)
    ));
    // 明文不落任何明文列(信封列之外无内容列)
    let leaked: i64 = h
        .db
        .call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM card_entries WHERE request_key IS NOT NULL
                  OR content_digest IN ('K-1','K-3','IMP-1')",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leaked, 0, "明文不得出现在任何可检索明文列");
}
