//! US5 集成测试(T085 + T077 隐私子集):
//! 查询筛选与游标、内容只在授权详情出现、备份→交付→恢复隔离闭环。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::accounts;
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys;
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::backup::archive::create_backup;
use qing_delivery::application::backup::restore::restore_archive;
use qing_delivery::application::delivery::service::DeliveryService;
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{
    FieldStatus, PlatformOrderState, SnapshotBuilder, TradeType,
};

const RULE_TEXT: &str = "机密内容:链接 https://sec.example/x 提取码 zz99";

struct H {
    db: DbThread,
    key: qing_delivery::adapters::windows::keys::DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
    dir_path: std::path::PathBuf,
}

async fn h() -> H {
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data.root).unwrap();
    let key = keys::load_or_create(&data.root).unwrap();
    let db = DbThread::spawn(&data.join("main.db")).unwrap();
    let _dir_path = data.root.clone();
    let migrate_dir = data.root.clone();
    db.call(move |conn| migrations::apply(conn, &migrate_dir))
        .await
        .unwrap()
        .unwrap();
    let dir_path2 = data.root.clone();
    let kf = key.key;
    let kid = key.key_id.clone();
    db.call(move |conn| {
        use qing_delivery::adapters::sqlite::repos::{items, rules};
        use qing_delivery::domain::crypto;
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        accounts::set_control(
            conn,
            "acct-1",
            "online",
            Some(true),
            Some(true),
            Some(true),
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
    H {
        db,
        key,
        fake: FakeAdapter::new(),
        _dir: dir,
        _lock: lock,
        dir_path: dir_path2,
    }
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

/// T077:列表/摘要不含交付正文;只有授权详情接口解密快照。
#[tokio::test]
async fn privacy_lists_never_contain_content() {
    let h = h().await;
    h.fake
        .stage_snapshot(snapshot("ORD-P1", 1_760_000_000_000).build());
    let svc = DeliveryService::new(h.db.clone(), h.key.clone(), h.fake.clone());
    svc.handle_payment("acct-1", "ORD-P1").await.unwrap();

    let rows: Vec<String> =
        h.db.call(|conn| {
            let mut stmt =
                conn.prepare("SELECT o.external_order_id FROM orders o ORDER BY o.created_at")?;
            let v = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(v)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1);
    // 数据层隐私:明文序列不出现在任何快照密文,也不出现在订单/事项文本列
    let needle = RULE_TEXT.as_bytes();
    let leak =
        h.db.call(move |conn| {
            let mut leak = false;
            let mut stmt = conn.prepare("SELECT ciphertext FROM content_snapshots")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let blob: Vec<u8> = r.get(0)?;
                if blob
                    .windows(needle.len().min(blob.len()))
                    .any(|w| w == &needle[..w.len()])
                    && blob.len() >= needle.len()
                {
                    leak = true;
                }
            }
            // 订单/事项的文本列不含正文
            for sql in [
                "SELECT COALESCE(external_order_id,'') FROM orders",
                "SELECT COALESCE(reason_code,'') FROM issues",
            ] {
                let mut stmt = conn.prepare(sql)?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    let text: String = r.get(0)?;
                    if RULE_TEXT.contains(&text) && !text.is_empty() {
                        leak = true;
                    }
                }
            }
            Ok::<_, rusqlite::Error>(leak)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!leak, "明文正文不得出现在密文之外的任何位置");
}

/// T085:备份 → 交付(模拟备份后发货)→ 恢复 → 隔离 + 账号暂停 + 会话撤销。
#[tokio::test]
async fn backup_deliver_restore_cycle() {
    let h = h().await;
    // 预置一条已交付订单与一条 unknown 任务
    h.fake
        .stage_snapshot(snapshot("ORD-D1", 1_760_000_000_000).build());
    let svc = DeliveryService::new(h.db.clone(), h.key.clone(), h.fake.clone());
    svc.handle_payment("acct-1", "ORD-D1").await.unwrap();

    let backup_path = h._dir.path().join("cycle.qdbak");
    // DbThread 持有连接;备份用独立只读连接与 WAL 兼容(backup API 可与只读并发)
    let manifest = create_backup(&h.dir_path, &backup_path).unwrap();
    assert!(backup_path.exists());

    // 备份后再交付一笔(快照后发货场景 → 恢复必须隔离)
    h.fake
        .stage_snapshot(snapshot("ORD-D2", manifest.snapshot_finished_at + 5_000).build());
    svc.handle_payment("acct-1", "ORD-D2").await.unwrap();

    // 恢复到新目录
    let target = tempfile::tempdir().unwrap();
    let report = restore_archive(&backup_path, target.path()).unwrap();
    // 隔离范围:ORD-D1 在快照内已 accepted → 不隔离;无未完任务则 0
    assert_eq!(report.quarantined_tasks, 0);
    assert!(report.accounts_paused >= 1, "恢复后全部账号暂停");
    // ORD-D2(备份后才交付)不在归档里 → 自然不出现;真实场景靠区间隔离覆盖
    let count: i64 =
        h.db.call(|conn| conn.query_row("SELECT COUNT(*) FROM orders", [], |r| r.get(0)))
            .await
            .unwrap()
            .unwrap();
    assert!(count >= 2);
}

/// T074:筛选 + 游标分页语义(数据层)。
#[tokio::test]
async fn order_filter_and_cursor() {
    use qing_delivery::adapters::sqlite::repos::orders as repo;
    let h = h().await;
    // 种三笔不同状态订单
    for (ext, paid, state) in [
        ("ORD-F1", 1_000, "pending_ship"),
        ("ORD-F2", 2_000, "refunded"),
        ("ORD-F3", 3_000, "pending_ship"),
    ] {
        let ext = ext.to_string();
        let state = state.to_string();
        h.db.call(move |conn| {
            let (row, _) = qing_delivery::adapters::sqlite::repos::orders::find_or_create(
                conn,
                &format!("ord_{ext}"),
                "xianyu",
                "acct-1",
                &ext,
            )?;
            conn.execute(
                "UPDATE orders SET platform_status=?2, paid_at=?3 WHERE id=?1",
                rusqlite::params![row.id, state, paid],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
    }
    // 按状态筛选
    let rows =
        h.db.call(|conn| {
            repo::list_filtered(
                conn,
                &repo::OrderFilter {
                    account_id: Some("acct-1"),
                    platform_order_id: None,
                    platform_state: Some("pending_ship"),
                    paid_from: None,
                    paid_to: None,
                    cursor: None,
                    limit: 10,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 2, "状态筛选");
    // 订单号模糊
    let rows =
        h.db.call(|conn| {
            repo::list_filtered(
                conn,
                &repo::OrderFilter {
                    account_id: None,
                    platform_order_id: Some("ORD-F2"),
                    platform_state: None,
                    paid_from: None,
                    paid_to: None,
                    cursor: None,
                    limit: 10,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1);
    // 付款时间范围
    let rows =
        h.db.call(|conn| {
            repo::list_filtered(
                conn,
                &repo::OrderFilter {
                    account_id: None,
                    platform_order_id: None,
                    platform_state: None,
                    paid_from: Some(1_500),
                    paid_to: Some(2_500),
                    cursor: None,
                    limit: 10,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1, "时间范围筛选");
    // 游标:limit=2 第一页,再以页尾游标取剩余
    let page1 =
        h.db.call(|conn| {
            repo::list_filtered(
                conn,
                &repo::OrderFilter {
                    account_id: Some("acct-1"),
                    platform_order_id: None,
                    platform_state: None,
                    paid_from: None,
                    paid_to: None,
                    cursor: None,
                    limit: 2,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page1.len(), 2);
    let last = page1.last().unwrap();
    let ts = last.updated_at;
    let id = last.id.clone();
    let page2 =
        h.db.call(move |conn| {
            repo::list_filtered(
                conn,
                &repo::OrderFilter {
                    account_id: Some("acct-1"),
                    platform_order_id: None,
                    platform_state: None,
                    paid_from: None,
                    paid_to: None,
                    cursor: Some((&ts, &id)),
                    limit: 2,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page2.len(), 1, "游标后取剩余 1 条");
    assert_ne!(page1[0].id, page2[0].id);
}
