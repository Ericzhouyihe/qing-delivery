//! T089 规模与性能验证(确定性部分):
//! SC-002(100 单交付正确性)、SC-006(10k 订单查询分位数)。
//! 运行:`cargo test --test e2e -- --nocapture --test-threads=1`

use std::time::Instant;

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::accounts;
use qing_delivery::adapters::sqlite::repos::{items, rules};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys;
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::delivery::service::DeliveryService;
use qing_delivery::domain::crypto;
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{
    FieldStatus, PlatformOrderState, SnapshotBuilder, TradeType,
};

const RULE_TEXT: &str = "链接 https://example.com/scale\n提取码 sc01";

async fn setup() -> (
    tempfile::TempDir,
    DbThread,
    FakeAdapter,
    qing_delivery::adapters::windows::keys::DataKey,
) {
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::resolve(Some(dir.path())).unwrap();
    let _lock = DirLock::acquire(&data.root).unwrap();
    let key = keys::load_or_create(&data.root).unwrap();
    let db = DbThread::spawn(&data.join("main.db")).unwrap();
    let mdir = data.root.clone();
    db.call(move |conn| migrations::apply(conn, &mdir))
        .await
        .unwrap()
        .unwrap();
    let kf = key.key;
    let kid = key.key_id.clone();
    db.call(move |conn| {
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        accounts::set_control(
            conn,
            "acct-1",
            "online",
            Some(true),
            Some(true),
            Some(false),
            Some(1),
        )?;
        items::upsert(
            conn, "item-1", "acct-1", "EXT-1", "资料", "on_sale", "[]", "single",
        )?;
        let rule = rules::insert(conn, "rule-1", "acct-1", "item-1", "single", true)?;
        let aad = rules::rule_aad(&rule.id, 1);
        let env = crypto::seal(&kf, &kid, &aad, RULE_TEXT.as_bytes());
        rules::insert_content(
            conn,
            "rc-1",
            &rule.id,
            1,
            &env,
            "d",
            RULE_TEXT.len() as i64,
            RULE_TEXT.len() as i64,
        )
    })
    .await
    .unwrap()
    .unwrap();
    (dir, db, FakeAdapter::new(), key)
}

fn snapshot(order: &str, paid: i64) -> SnapshotBuilder {
    SnapshotBuilder {
        platform_order_id: order.into(),
        seller_id: FieldStatus::Verified("seller-1".into()),
        buyer_id: FieldStatus::Verified("buyer-1".into()),
        item_id: FieldStatus::Verified("EXT-1".into()),
        trade_type: FieldStatus::Verified(TradeType::Ordinary),
        platform_state: FieldStatus::Verified(PlatformOrderState::PendingShip),
        paid_at_ms: FieldStatus::Verified(paid),
        amount: FieldStatus::Verified(Money::new(990, "CNY").unwrap()),
        quantity: FieldStatus::Verified(1),
        sku_parts: FieldStatus::Missing,
        sku_single: true,
        conversation_verified: true,
        source: "detail".into(),
        observed_at_ms: paid + 1000,
        browser_supplemented: false,
    }
}

fn pct(sorted: &mut [u128], p: f64) -> u128 {
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// SC-002(确定性子集):100 笔普通订单,买家/内容匹配 100%,每单恰好一次。
#[tokio::test]
async fn sc002_hundred_orders_correctness() {
    let (_dir, db, fake, key) = setup().await;
    for i in 0..100 {
        fake.stage_snapshot(
            snapshot(&format!("SC2-{i:03}"), 1_760_000_000_000 + i as i64 * 1000).build(),
        );
    }
    let svc = DeliveryService::new(db.clone(), key, fake.clone());
    let start = Instant::now();
    let mut delivered = 0;
    for i in 0..100 {
        let out = svc
            .handle_payment("acct-1", &format!("SC2-{i:03}"))
            .await
            .unwrap();
        if matches!(
            out,
            qing_delivery::application::delivery::service::HandleOutcome::Delivered { .. }
        ) {
            delivered += 1;
        }
    }
    let elapsed = start.elapsed();
    assert_eq!(delivered, 100, "100/100 交付成功");
    let sent = fake.sent_records();
    assert_eq!(sent.len(), 100, "每单恰好一次");
    assert!(sent.iter().all(|r| r.text == RULE_TEXT), "内容匹配 100%");
    assert!(
        sent.iter().all(|r| r.buyer_id == "buyer-1"),
        "买家匹配 100%"
    );
    println!(
        "SC-002: 100 单交付耗时 {:?}(均值 {:.1}ms/单,本地模拟;P95≤10s 目标以真实平台计时为准)",
        elapsed,
        elapsed.as_millis() as f64 / 100.0
    );
}

/// SC-006:10,000 条历史订单,查询 P95 ≤ 2 秒(本地 SQLite 实测分位数)。
#[tokio::test]
async fn sc006_ten_thousand_query_p95() {
    let (_dir, db, _fake, _key) = setup().await;
    // 批量种子 10k 订单(直接 SQL,纯查询基准)
    db.call(|conn| {
        conn.execute("BEGIN", [])?;
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO orders(id, platform, account_id, external_order_id, buyer_id,
                 paid_at, quantity, amount_minor, currency, trade_type, platform_status,
                 created_at, updated_at, fact_version)
             VALUES (?1,'xianyu','acct-1',?2,'buyer-1',?3,1,990,'CNY','ordinary',
                     'pending_ship',?3,?3,1)",
        )?;
        for i in 0..10_000 {
            stmt.execute(rusqlite::params![
                format!("ord_seed_{i:05}"),
                format!("EXT-SEED-{i:05}"),
                1_760_000_000_000 + i as i64 * 60_000
            ])?;
        }
        drop(stmt);
        conn.execute("COMMIT", [])?;
        Ok::<(), rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();

    // 混合查询样本:按账号、按订单号模糊、按状态+时间范围、游标翻页
    let mut timings: Vec<u128> = Vec::new();
    for i in 0..40 {
        let t = Instant::now();
        let q = i % 4;
        db.call(
            move |conn| -> rusqlite::Result<
                Vec<qing_delivery::adapters::sqlite::repos::orders::OrderSummaryRow>,
            > {
                use qing_delivery::adapters::sqlite::repos::orders as repo;
                Ok(match q {
                    0 => repo::list_filtered(
                        conn,
                        &repo::OrderFilter {
                            account_id: Some("acct-1"),
                            platform_order_id: None,
                            platform_state: None,
                            paid_from: None,
                            paid_to: None,
                            cursor: None,
                            limit: 50,
                        },
                    )?,
                    1 => repo::list_filtered(
                        conn,
                        &repo::OrderFilter {
                            account_id: None,
                            platform_order_id: Some(&format!("EXT-SEED-{i:05}")),
                            platform_state: None,
                            paid_from: None,
                            paid_to: None,
                            cursor: None,
                            limit: 50,
                        },
                    )?,
                    2 => repo::list_filtered(
                        conn,
                        &repo::OrderFilter {
                            account_id: Some("acct-1"),
                            platform_order_id: None,
                            platform_state: Some("pending_ship"),
                            paid_from: Some(1_760_000_100_000),
                            paid_to: None,
                            cursor: None,
                            limit: 50,
                        },
                    )?,
                    _ => {
                        let first = repo::list_filtered(
                            conn,
                            &repo::OrderFilter {
                                account_id: Some("acct-1"),
                                platform_order_id: None,
                                platform_state: None,
                                paid_from: None,
                                paid_to: None,
                                cursor: None,
                                limit: 50,
                            },
                        )?;
                        if let Some(last) = first.last() {
                            let (ts, id) = (last.updated_at, last.id.clone());
                            repo::list_filtered(
                                conn,
                                &repo::OrderFilter {
                                    account_id: Some("acct-1"),
                                    platform_order_id: None,
                                    platform_state: None,
                                    paid_from: None,
                                    paid_to: None,
                                    cursor: Some((&ts, &id)),
                                    limit: 50,
                                },
                            )?
                        } else {
                            first
                        }
                    }
                })
            },
        )
        .await
        .unwrap()
        .unwrap();
        timings.push(t.elapsed().as_micros());
    }
    let p50 = pct(&mut timings.clone(), 0.50);
    let p95 = pct(&mut timings.clone(), 0.95);
    println!("SC-006: 40 次查询 @10k 记录:P50={p50}µs P95={p95}µs(目标 P95≤2s)");
    assert!(p95 < 2_000_000, "P95 {p95}µs 超出 2 秒目标");
}

/// SC-001(结构子集):预置账号+商品下创建规则并预览的步骤计时(服务层,不含浏览器操作)。
#[tokio::test]
async fn sc001_rule_config_path_timing() {
    use qing_delivery::application::catalog::rules::{RuleDraft, RuleService};
    let (_dir, db, _fake, key) = setup().await;
    let svc = RuleService::new(db, key);
    let t = Instant::now();
    let check = svc.preview("https://example.com/final\n提取码 ok12");
    assert!(check.valid);
    // setup 已占用 single 范围;本用例用独立规格组合(体现精确匹配作用域)
    let sku_key = qing_delivery::domain::sku::combo_key(&[qing_delivery::domain::sku::SkuPart {
        property_id: "sc001".into(),
        value_id: "p1".into(),
        property_label: String::new(),
        value_label: String::new(),
    }])
    .unwrap();
    let saved = svc
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key,
                content: "https://example.com/final\n提取码 ok12".into(),
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let elapsed = t.elapsed();
    assert!(!saved.id.is_empty());
    println!(
        "SC-001(服务层):预览+保存规则 {elapsed:?}(5 分钟目标含浏览器内完整操作,人工计时另行记录)"
    );
}
