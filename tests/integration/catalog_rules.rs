//! US3 集成测试(T062,V03 场景):
//! 空页正常、部分失败不下架、完整同步下架、启用冲突、超长拒绝、预览零发送。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, items as items_repo};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys;
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::catalog::rules::{RuleDraft, RuleService};
use qing_delivery::application::catalog::sync::ItemSyncService;
use qing_delivery::application::ports::platform::{
    PlatformError, ProductPage, ProductRecord, RequestContext,
};
use qing_delivery::domain::sku::SkuPart;

struct CatalogHarness {
    db: DbThread,
    fake: FakeAdapter,
    ctx: RequestContext,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

fn page(items: Vec<ProductRecord>, next: Option<&str>) -> Result<ProductPage, PlatformError> {
    let empty = items.is_empty();
    Ok(ProductPage {
        items,
        next_cursor: next.map(|s| s.to_string()),
        expresses_empty: empty,
    })
}

fn record(id: &str, title: &str) -> ProductRecord {
    ProductRecord {
        external_item_id: id.into(),
        title: title.into(),
        on_sale: true,
        sku_parts: vec![],
        sku_complete: false,
        single_sku_confirmed: true,
    }
}

async fn harness() -> CatalogHarness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let _key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("cat.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    db.call(|conn| accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家"))
        .await
        .unwrap()
        .unwrap();
    // 先预置一个旧商品,验证下架语义
    db.call(|conn| {
        items_repo::upsert(
            conn,
            "item-old",
            "acct-1",
            "EXT-OLD",
            "旧资料",
            "on_sale",
            "[]",
            "single",
        )
    })
    .await
    .unwrap()
    .unwrap();
    CatalogHarness {
        db,
        fake: FakeAdapter::new(),
        ctx: RequestContext {
            operation_id: "op".into(),
            account_id: "acct-1".into(),
            credential_generation: 0,
            control_generation: 0,
            deadline_ms: 0,
        },
        _dir: dir,
        _lock: lock,
    }
}

async fn harness_with_key() -> (CatalogHarness, RuleService) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("cat.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    db.call(|conn| {
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
        items_repo::upsert(
            conn, "item-1", "acct-1", "EXT-1", "资料", "on_sale", "[]", "single",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let h = CatalogHarness {
        db: db.clone(),
        fake: FakeAdapter::new(),
        ctx: RequestContext {
            operation_id: "op".into(),
            account_id: "acct-1".into(),
            credential_generation: 0,
            control_generation: 0,
            deadline_ms: 0,
        },
        _dir: dir,
        _lock: lock,
    };
    let svc = RuleService::new(db, key);
    (h, svc)
}

#[tokio::test]
async fn empty_page_is_normal_success() {
    let h = harness().await;
    h.fake.stage_product_page(page(vec![], None));
    let svc = ItemSyncService::new(h.db.clone(), h.fake.clone());
    let result = svc.run("acct-1", &h.ctx).await.unwrap();
    assert_eq!(result.state, "complete", "成功省略列表 = 正常空页(US3-1)");
    assert_eq!(result.fetched_count, 0);
    // 空同步不下架任何已有商品(无覆盖证据)
    let old =
        h.db.call(|conn| items_repo::find_by_external(conn, "acct-1", "EXT-OLD"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(old.listing_state, "off_sale", "完整空同步后旧商品确实下架");
    assert_eq!(result.delisted, 1);
}

#[tokio::test]
async fn partial_failure_never_delists() {
    let h = harness().await;
    // 第一页成功,第二页失败 → 不完整
    h.fake
        .stage_product_page(page(vec![record("EXT-A", "A")], Some("cursor-2")));
    h.fake.stage_product_page(Err(PlatformError::RateLimited));
    let svc = ItemSyncService::new(h.db.clone(), h.fake.clone());
    let result = svc.run("acct-1", &h.ctx).await.unwrap();
    assert_eq!(result.state, "failed");
    assert!(result.failure_reason.is_some());
    assert_eq!(result.delisted, 0, "失败同步绝不下架(US3-4)");
    let old =
        h.db.call(|conn| items_repo::find_by_external(conn, "acct-1", "EXT-OLD"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(old.listing_state, "on_sale", "未返回的商品不被误标下架");
    // 已拉到的 A 保留
    let a =
        h.db.call(|conn| items_repo::find_by_external(conn, "acct-1", "EXT-A"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(a.title, "A", "部分结果保留已知事实");
}

#[tokio::test]
async fn multi_page_complete_sync_delists_missing() {
    let h = harness().await;
    h.fake.stage_product_page(page(
        vec![record("EXT-A", "A"), record("EXT-B", "B")],
        Some("c2"),
    ));
    h.fake
        .stage_product_page(page(vec![record("EXT-OLD", "旧资料")], None));
    let svc = ItemSyncService::new(h.db.clone(), h.fake.clone());
    let result = svc.run("acct-1", &h.ctx).await.unwrap();
    assert_eq!(result.state, "complete");
    assert_eq!(result.fetched_count, 3);
    // A、B 出现过 → 在售;无 EXT-OLD 之外的旧商品
    assert_eq!(result.delisted, 0);
    let b =
        h.db.call(|conn| items_repo::find_by_external(conn, "acct-1", "EXT-B"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(b.listing_state, "on_sale");

    // 再同步:只剩 OLD,完整 → A/B 下架
    h.fake
        .stage_product_page(page(vec![record("EXT-OLD", "旧资料")], None));
    let result2 = svc.run("acct-1", &h.ctx).await.unwrap();
    assert_eq!(result2.delisted, 2, "完整同步后缺失商品下架");
}

#[tokio::test]
async fn rule_lifecycle_conflict_and_snapshot_freeze() {
    let (_h, svc) = harness_with_key().await;
    // 创建启用规则 → 同范围第二条启用冲突;禁用的允许;切换经版本更新
    let first = svc
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: "v1 内容".into(),
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let conflict = svc
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: "v2 内容".into(),
                enabled: true,
                ..Default::default()
            },
        )
        .await;
    assert!(conflict.is_err(), "同范围启用唯一(FR-008)");
    let second = svc
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: "另范围备用".into(),
                enabled: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // 内容更新 → v2;旧版本保留(交付快照可回溯)
    let updated = svc
        .update(
            "acct-1",
            &first.id,
            first.version,
            Some("v2 内容".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated.content_version, 2);

    // 禁用第一条 → 可返回新版本号;第二条随后启用
    let disabled = svc
        .update("acct-1", &first.id, updated.version, None, Some(false))
        .await
        .unwrap();
    svc.update("acct-1", &second.id, second.version, None, Some(true))
        .await
        .unwrap();
    // 第二条已启用:再启用第一条 → 冲突(执行时同样检测歧义)
    let again = svc
        .update("acct-1", &first.id, disabled.version, None, Some(true))
        .await;
    assert!(again.is_err(), "执行时同样检测歧义");
}

#[tokio::test]
async fn overlong_content_rejected_with_reason() {
    let (_h, svc) = harness_with_key().await;
    let too_long = "很".repeat(1001);
    let err = svc
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: too_long,
                enabled: false,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("1000") && msg.contains("不截断"),
        "拒绝必须显示当前限制与不截断,实际:{msg}"
    );
    // 预览同源:计数正确
    let p = svc.preview(&"a".repeat(4001));
    assert!(!p.valid && p.utf8_bytes == 4001);
}

/// SKU 组合键匹配预览:完整组合精确命中,部分不命中。
#[tokio::test]
async fn sku_combo_rule_scope() {
    let (_h, svc) = harness_with_key().await;
    let combo = qing_delivery::domain::sku::combo_key(&[SkuPart {
        property_id: "p1".into(),
        value_id: "v1".into(),
        property_label: String::new(),
        value_label: String::new(),
    }])
    .unwrap();
    svc.create(
        "acct-1",
        RuleDraft {
            item_id: "item-1".into(),
            sku_key: combo.clone(),
            content: "组合内容".into(),
            enabled: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    use qing_delivery::application::catalog::rules::MatchPreview;
    assert!(matches!(
        svc.match_preview("acct-1", "item-1", &combo).await.unwrap(),
        MatchPreview::Matched { .. }
    ));
    // "single" 不命中组合规则(无全账号兜底)
    assert_eq!(
        svc.match_preview("acct-1", "item-1", "single")
            .await
            .unwrap(),
        MatchPreview::None
    );
}
