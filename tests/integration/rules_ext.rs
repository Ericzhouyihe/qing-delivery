//! US3 规则扩展集成测试(T030,T031—T034 转绿):
//! 1) 同范围同触发两条启用不同 priority → 仅 priority 小的执行;
//! 2) 同范围同触发同 priority 第二条创建 → 422 rule_conflict(应用层显式检查);
//! 3) 账号级规则未确认 → 需确认·暂不发货;确认后执行;
//! 4) 变体:多规格订单命中对应变体卡组,按 units_per_item 取卡;
//! 5) 商品级规则优先于账号级回退(同 priority);
//! 6) 关键词回复 CRUD + 匹配查询(商品级优先、包含匹配忽略大小写;不触交付);
//! 7) RuleDraft 扩展保存/回读 roundtrip(trigger/variants/priority/review_config/来源绑定);
//! 8) 引用缺失 → needs_reconfiguration 置位旁路执行,修复后清除。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, cards, items, rules_ext};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::catalog::rules::{RuleDraft, RuleError, RuleService};
use qing_delivery::application::delivery::service::{DeliveryService, HandleOutcome};
use qing_delivery::domain::crypto::{self, Aad};
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{FieldStatus, SnapshotBuilder, TradeType};
use qing_delivery::domain::rules_ext::{
    ContentSource, ReviewConfig, TriggerType, VariantSource,
};
use qing_delivery::domain::sku::SkuPart;
use sha2::Digest as _;

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

fn card_entry_aad(entry_id: &str) -> Aad {
    Aad {
        purpose: "card_entry".into(),
        entity_id: entry_id.into(),
        content_version: None,
    }
}

/// 建批量卡组并导入 texts(信封 AAD purpose card_entry)。
async fn seed_pool(h: &Harness, pool_id: &str, texts: &[&str]) {
    let key = h.key.key;
    let key_id = h.key.key_id.clone();
    let pool = pool_id.to_string();
    let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    h.db
        .call(move |conn| {
            cards::insert_pool(
                conn,
                &cards::NewPool {
                    id: &pool,
                    name: &format!("组-{pool}"),
                    kind: "data",
                    enabled: true,
                    delay_seconds: 0,
                    description: "",
                    content_envelope: None,
                    api_config_envelope: None,
                },
            )?;
            for (i, text) in texts.iter().enumerate() {
                let entry_id = format!("{pool}-e{i}");
                let envelope = crypto::seal(&key, &key_id, &card_entry_aad(&entry_id), text.as_bytes());
                cards::append_entries(
                    conn,
                    &pool,
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
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us3.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    db.call(|conn| {
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
        items::upsert(
            conn, "item-1", "acct-1", "EXT-ITEM-1", "考研资料", "on_sale", "[]", "single",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let h = Harness {
        db: db.clone(),
        key,
        fake: FakeAdapter::new(),
        _dir: dir,
        _lock: lock,
    };
    h
}

fn snapshot_with_parts(order: &str, buyer: &str, parts: Vec<SkuPart>, quantity: u32) -> SnapshotBuilder {
    let sku = if parts.is_empty() {
        None
    } else {
        Some(qing_delivery::domain::sku::combo_key(&parts).unwrap())
    };
    let mut b = complete_snapshot(order, buyer, quantity);
    if let Some(parts) = sku.map(|_| parts) {
        b.sku_parts = FieldStatus::Verified(parts);
        b.sku_single = false;
    }
    b
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

fn rule_service(h: &Harness) -> RuleService {
    RuleService::new(
        h.db.clone(),
        DataKey {
            key_id: h.key.key_id.clone(),
            key: h.key.key,
        },
    )
}

fn fixed_draft(item: &str, sku: &str, content: &str, priority: i64, enabled: bool) -> RuleDraft {
    RuleDraft {
        item_id: item.into(),
        sku_key: sku.into(),
        content: content.into(),
        enabled,
        priority,
        ..Default::default()
    }
}

/// 1) 同范围同触发两条启用不同 priority → 仅 priority 小的执行(另一条不发货)。
#[tokio::test]
async fn only_smallest_priority_rule_delivers() {
    let h = harness().await;
    let rs = rule_service(&h);
    rs.create("acct-1", fixed_draft("item-1", "single", "高优先内容", 10, true))
        .await
        .unwrap();
    rs.create("acct-1", fixed_draft("item-1", "single", "低优先内容", 200, true))
        .await
        .unwrap();
    h.fake
        .stage_snapshot(complete_snapshot("ORD-1", "buyer-1", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "应交付,{out:?}");
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 1, "仅最高优先级一条执行,不得重复发货");
    assert_eq!(sent[0].text, "高优先内容");
}

/// 2) 同范围同触发同 priority 第二条创建 → EnabledConflict(422 rule_conflict;
///    不同 priority 允许并存)。禁用规则不占优先级位。
#[tokio::test]
async fn same_priority_second_create_rejected() {
    let h = harness().await;
    let rs = rule_service(&h);
    rs.create("acct-1", fixed_draft("item-1", "single", "第一条", 100, true))
        .await
        .unwrap();
    let err = rs
        .create("acct-1", fixed_draft("item-1", "single", "第二条", 100, true))
        .await
        .unwrap_err();
    assert!(matches!(err, RuleError::EnabledConflict), "同优先级冲突:{err}");
    // 不同优先级允许并存
    rs.create("acct-1", fixed_draft("item-1", "single", "另一档", 200, true))
        .await
        .unwrap();
    // 禁用不占位
    rs.create("acct-1", fixed_draft("item-1", "single", "禁用档", 100, false))
        .await
        .unwrap();
}

/// 3) 账号级规则未确认 → 匹配返回需确认不执行;确认后执行(FR-032)。
#[tokio::test]
async fn account_rule_needs_confirmation_then_executes() {
    let h = harness().await;
    let rs = rule_service(&h);
    let draft = RuleDraft {
        item_id: String::new(),
        sku_key: "single".into(),
        content: "账号级兜底内容".into(),
        enabled: true,
        all_items_confirmed: false,
        ..Default::default()
    };
    let saved = rs.create("acct-1", draft).await.unwrap();
    // 落库标记:未确认由 all_items_confirmed=false 表达(FR-032 需确认·暂不发货);
    // needs_reconfiguration 仅承载引用缺失,两态在执行侧分别裁决
    let saved_id_probe = saved.id.clone();
    let flag = h
        .db
        .call(move |conn| {
            let row = rules_ext::get_rule(conn, &saved_id_probe)?.unwrap();
            Ok::<_, rusqlite::Error>((row.all_items_confirmed, row.needs_reconfiguration))
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!flag.0, "all_items_confirmed 保存为 false");
    assert!(!flag.1, "未确认不与引用缺失标记混用");

    h.fake
        .stage_snapshot(complete_snapshot("ORD-1", "buyer-1", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    match out {
        HandleOutcome::Ineligible { reason, .. } => {
            assert!(reason.contains("需确认"), "应提示需确认·暂不发货:{reason}");
        }
        other => panic!("未确认账号级规则不得执行,实际 {other:?}"),
    }
    assert!(
        h.fake.sent_records().is_empty(),
        "需确认规则零发送,订单/交付状态不因规则推进"
    );

    // 确认「适用于全部商品」后执行(修复后清除标记)
    rs.update_full(
        "acct-1",
        &saved.id,
        saved.version,
        RuleDraft {
            item_id: String::new(),
            sku_key: "single".into(),
            content: "账号级兜底内容".into(),
            enabled: true,
            all_items_confirmed: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "确认后应执行,{out:?}");
    assert_eq!(h.fake.sent_records().len(), 1);
}

/// 4) 变体:多规格订单命中对应变体卡组,按 units_per_item × 件数取卡。
#[tokio::test]
async fn variant_rules_hit_matching_pool_and_units() {
    let h = harness().await;
    seed_pool(&h, "pool-red", &["红卡1", "红卡2", "红卡3", "红卡4", "红卡5"]).await;
    seed_pool(&h, "pool-blue", &["蓝卡1", "蓝卡2", "蓝卡3", "蓝卡4", "蓝卡5"]).await;
    let rs = rule_service(&h);
    let draft = RuleDraft {
        item_id: "item-1".into(),
        sku_key: String::new(),
        content: String::new(),
        enabled: true,
        content_source: ContentSource::CardPool,
        variants: vec![
            qing_delivery::application::catalog::rules::VariantDraft {
                spec_name: "颜色".into(),
                spec_values: vec!["红色".into()],
                source: VariantSource::CardPool,
                card_pool_id: Some("pool-red".into()),
                template_id: None,
                template_bindings: None,
                units_per_item: 2,
                delay_override_seconds: None,
            },
            qing_delivery::application::catalog::rules::VariantDraft {
                spec_name: "颜色".into(),
                spec_values: vec!["蓝色".into()],
                source: VariantSource::CardPool,
                card_pool_id: Some("pool-blue".into()),
                template_id: None,
                template_bindings: None,
                units_per_item: 3,
                delay_override_seconds: None,
            },
        ],
        ..Default::default()
    };
    rs.create("acct-1", draft).await.unwrap();

    // 红色规格 × 2 件 → pool-red 取 2×2=4 张
    let parts = vec![SkuPart {
        property_id: "p1".into(),
        value_id: "v1".into(),
        property_label: "颜色".into(),
        value_label: "红色".into(),
    }];
    h.fake
        .stage_snapshot(snapshot_with_parts("ORD-RED", "buyer-1", parts, 2).build());
    let out = service(&h).handle_payment("acct-1", "ORD-RED").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "{out:?}");
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].text, "红卡1\n红卡2\n红卡3\n红卡4", "按 units_per_item×件数取卡拼接");

    // 蓝色规格 × 1 件 → pool-blue 取 3 张,红色池不动
    let parts = vec![SkuPart {
        property_id: "p1".into(),
        value_id: "v2".into(),
        property_label: "颜色".into(),
        value_label: "蓝色".into(),
    }];
    h.fake
        .stage_snapshot(snapshot_with_parts("ORD-BLUE", "buyer-2", parts, 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-BLUE").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "{out:?}");
    let blue_texts: Vec<String> = h
        .fake
        .sent_records()
        .iter()
        .filter(|r| r.buyer_id == "buyer-2")
        .map(|r| r.text.clone())
        .collect();
    assert_eq!(blue_texts.len(), 1);
    assert_eq!(blue_texts[0], "蓝卡1\n蓝卡2\n蓝卡3");

    let counts = h
        .db
        .call(|conn| {
            Ok::<_, rusqlite::Error>((
                cards::state_counts(conn, "pool-red")?,
                cards::state_counts(conn, "pool-blue")?,
            ))
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counts.0.available, 1, "红池 5-4");
    assert_eq!(counts.0.used, 4);
    assert_eq!(counts.1.available, 2, "蓝池 5-3");
    assert_eq!(counts.1.used, 3);
}

/// 5) 商品级规则优先于账号级回退(同 priority)。
#[tokio::test]
async fn item_rule_beats_account_fallback_same_priority() {
    let h = harness().await;
    let rs = rule_service(&h);
    rs.create(
        "acct-1",
        RuleDraft {
            item_id: String::new(),
            sku_key: "single".into(),
            content: "账号级内容".into(),
            enabled: true,
            all_items_confirmed: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    rs.create("acct-1", fixed_draft("item-1", "single", "商品级内容", 100, true))
        .await
        .unwrap();
    h.fake
        .stage_snapshot(complete_snapshot("ORD-1", "buyer-1", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "{out:?}");
    assert_eq!(h.fake.sent_records().len(), 1);
    assert_eq!(h.fake.sent_records()[0].text, "商品级内容");
}

/// 6) 关键词回复 CRUD 与匹配查询:包含匹配忽略大小写、商品级优先账号级、
///    不触发任何交付(订单/交付零变化)。
#[tokio::test]
async fn reply_rules_crud_and_match_query() {
    let h = harness().await;
    let item_ids = vec!["item-1".to_string()];
    h.db
        .call(move |conn| {
            rules_ext::insert_reply_rule(
                conn,
                "rr-acct",
                "acct-1",
                "TAOCAN",
                "text",
                Some("已收到,套餐内容见链接"),
                None,
                true,
                &[],
            )?;
            rules_ext::insert_reply_rule(
                conn,
                "rr-item",
                "acct-1",
                "发货",
                "text",
                Some("发货中,请稍候"),
                None,
                true,
                &item_ids,
            )
        })
        .await
        .unwrap()
        .unwrap();

    // 匹配查询:两条都包含命中,商品级(rr-item)排在账号级之前
    let matches = h
        .db
        .call(|conn| rules_ext::match_reply_rules(conn, "acct-1", Some("item-1"), "请问taocan什么时候发货"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matches.len(), 2, "包含匹配忽略大小写;两条关键词均命中");
    assert_eq!(matches[0].id, "rr-item", "商品级优先于账号级");
    assert_eq!(matches[1].id, "rr-acct");
    // 大小写不敏感:关键词 TAOCAN 命中小写文本
    let matches = h
        .db
        .call(|conn| rules_ext::match_reply_rules(conn, "acct-1", Some("item-1"), "我的taocan呢"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matches.len(), 1);
    // 其他商品不命中商品级关键词,账号级仍命中
    let matches = h
        .db
        .call(|conn| {
            rules_ext::match_reply_rules(conn, "acct-1", Some("item-other"), "taocan发货了吗")
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "rr-acct");
    // 未命中返回空(分流编排 T036 接手;此处零订单/交付变化)
    let before: i64 = h
        .db
        .call(|conn| {
            Ok::<i64, rusqlite::Error>(conn.query_row(
                "SELECT COUNT(*) FROM deliveries",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before, 0, "关键词匹配不触发交付");

    // CRUD:更新(整体替换,含关联商品)、列表、删除
    let updated = h
        .db
        .call(|conn| {
            rules_ext::update_reply_rule(
                conn,
                "rr-item",
                "已发货",
                "text",
                Some("已发货,请查收"),
                None,
                false,
                &[],
            )
        })
        .await
        .unwrap()
        .unwrap()
        .expect("更新已存在的关键词规则");
    assert_eq!(updated.keyword, "已发货");
    assert!(!updated.enabled);
    assert!(updated.item_ids.is_empty(), "改为账号级(无关联行)");
    let list = h
        .db
        .call(|conn| rules_ext::list_reply_rules(conn, "acct-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter().any(|r| r.id == "rr-acct" && r.item_ids.is_empty()),
        "无关联行 = 账号级"
    );
    let removed = h
        .db
        .call(|conn| rules_ext::delete_reply_rule(conn, "rr-item"))
        .await
        .unwrap()
        .unwrap();
    assert!(removed);
    let after = h
        .db
        .call(|conn| rules_ext::list_reply_rules(conn, "acct-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.len(), 1);
}

/// 默认回复 upsert/一次性判定/清空记录(T032 仓储语义)。
#[tokio::test]
async fn default_reply_upsert_once_and_clear() {
    let h = harness().await;
    h.db
        .call(|conn| {
            rules_ext::upsert_default_reply(
                conn,
                "acct-1",
                true,
                Some("亲,稍后回复您"),
                None,
                true,
            )
        })
        .await
        .unwrap()
        .unwrap();
    let got = h
        .db
        .call(|conn| rules_ext::get_default_reply(conn, "acct-1"))
        .await
        .unwrap()
        .unwrap()
        .expect("默认回复已保存");
    assert!(got.enabled);
    assert!(got.reply_once);

    // 一次性判定:无 accepted 记录 → 未回复过;追加 accepted 后 → 已回复过
    let replied = h
        .db
        .call(|conn| rules_ext::has_accepted_default_reply(conn, "acct-1", "buyer-9"))
        .await
        .unwrap()
        .unwrap();
    assert!(!replied);
    h.db
        .call(|conn| {
            rules_ext::append_default_reply_log(conn, "drl-1", "acct-1", "buyer-9", "accepted")
        })
        .await
        .unwrap()
        .unwrap();
    let replied = h
        .db
        .call(|conn| rules_ext::has_accepted_default_reply(conn, "acct-1", "buyer-9"))
        .await
        .unwrap()
        .unwrap();
    assert!(replied, "reply_once 判定 = 同 (account,buyer) 存在 accepted 行");
    let log = h
        .db
        .call(|conn| rules_ext::list_default_reply_log(conn, "acct-1", 10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].buyer_id, "buyer-9");
    // 清空记录
    let cleared = h
        .db
        .call(|conn| rules_ext::clear_default_reply_log(conn, "acct-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleared, 1);
    let replied = h
        .db
        .call(|conn| rules_ext::has_accepted_default_reply(conn, "acct-1", "buyer-9"))
        .await
        .unwrap()
        .unwrap();
    assert!(!replied, "清空后可再次回复");
}

/// 7) RuleDraft 扩展保存/回读 roundtrip:trigger/priority/来源绑定/变体/求评配置。
#[tokio::test]
async fn rule_draft_extended_roundtrip() {
    let h = harness().await;
    seed_pool(&h, "pool-a", &["A1", "A2"]).await;
    // 模板 + 变体模板绑定
    h.db
        .call(|conn| {
            qing_delivery::adapters::sqlite::repos::templates::insert_template(
                conn,
                "tpl-1",
                "双规格模板",
                true,
                &["感谢购买,您的卡密:{{cards.k1}}".to_string()],
            )
        })
        .await
        .unwrap()
        .unwrap();

    let mut bindings = qing_delivery::domain::templates::TemplateBindings::default();
    bindings.cards.push(
        qing_delivery::domain::templates::CardBinding {
            key: "k1".into(),
            pool_id: "pool-a".into(),
            units: 1,
        },
    );
    bindings.custom.insert("note".into(), "谢谢".into());

    let rs = rule_service(&h);
    let draft = RuleDraft {
        item_id: "item-1".into(),
        sku_key: String::new(),
        content: String::new(),
        enabled: true,
        trigger_type: TriggerType::ReviewMissingTimeout,
        priority: 55,
        content_source: ContentSource::CardPool,
        card_pool_id: Some("pool-a".into()),
        template_id: None,
        template_bindings: None,
        variants: vec![qing_delivery::application::catalog::rules::VariantDraft {
            spec_name: "颜色".into(),
            spec_values: vec!["红色".into(), "蓝色".into()],
            source: VariantSource::Template,
            card_pool_id: None,
            template_id: Some("tpl-1".into()),
            template_bindings: Some(bindings.clone()),
            units_per_item: 2,
            delay_override_seconds: Some(120),
        }],
        review_config: Some(ReviewConfig {
            wait_hours: 24,
            interval_hours: 12,
            max_count: 3,
            text: "亲,期待您的评价".into(),
        }),
        all_items_confirmed: false,
    };
    let saved = rs.create("acct-1", draft).await.unwrap();

    // 回读:规则行 + 变体
    let read = h
        .db
        .call(move |conn| {
            let rule = rules_ext::get_rule(conn, &saved.id)?.unwrap();
            let variants = rules_ext::list_variants(conn, &saved.id)?;
            Ok::<_, rusqlite::Error>((rule, variants))
        })
        .await
        .unwrap()
        .unwrap();
    let (rule, variants) = read;
    assert_eq!(rule.trigger_type, "review_missing_timeout");
    assert_eq!(rule.priority, 55);
    assert_eq!(rule.card_pool_id.as_deref(), Some("pool-a"));
    assert!(!rule.needs_reconfiguration, "引用齐全,标记清除");
    let review: ReviewConfig =
        serde_json::from_str(rule.review_config.as_deref().unwrap()).unwrap();
    assert_eq!(review.max_count, 3);
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].spec_name, "颜色");
    assert_eq!(variants[0].spec_values, vec!["红色".to_string(), "蓝色".to_string()]);
    assert_eq!(variants[0].source, VariantSource::Template);
    assert_eq!(variants[0].template_id.as_deref(), Some("tpl-1"));
    assert_eq!(variants[0].units_per_item, 2);
    assert_eq!(variants[0].delay_override_seconds, Some(120));
    let rb: qing_delivery::domain::templates::TemplateBindings =
        serde_json::from_str(variants[0].template_bindings.as_deref().unwrap()).unwrap();
    assert_eq!(rb, bindings);

    // buyer_reviewed 触发:创建时 422 unsupported_capability(FR-036 能力门禁)
    let err = rs
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: "赠品".into(),
                enabled: false,
                trigger_type: TriggerType::BuyerReviewed,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RuleError::UnsupportedTrigger), "{err}");
}

/// 8) 引用缺失 → needs_reconfiguration 置位并旁路执行;修复后清除再执行。
#[tokio::test]
async fn missing_reference_marks_and_bypasses_until_fixed() {
    let h = harness().await;
    let rs = rule_service(&h);
    let saved = rs
        .create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: String::new(),
                enabled: true,
                content_source: ContentSource::CardPool,
                card_pool_id: Some("pool-ghost".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ghost_probe = saved.id.clone();
    let flagged = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                rules_ext::get_rule(conn, &ghost_probe)?.unwrap().needs_reconfiguration,
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert!(flagged, "引用缺失的规则保存后标记需重新配置");

    h.fake
        .stage_snapshot(complete_snapshot("ORD-1", "buyer-1", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    match out {
        HandleOutcome::Ineligible { reason, .. } => {
            assert!(reason.contains("重新配置"), "应旁路执行并提示:{reason}");
        }
        other => panic!("需重新配置规则不得执行,实际 {other:?}"),
    }
    assert!(h.fake.sent_records().is_empty());

    // 修复:绑定到真实卡组 → 标记清除 → 可执行
    seed_pool(&h, "pool-real", &["真卡1"]).await;
    let fresh = h
        .db
        .call({
            let id = saved.id.clone();
            move |conn| Ok::<_, rusqlite::Error>(rules_ext::get_rule(conn, &id)?.unwrap().version)
        })
        .await
        .unwrap()
        .unwrap();
    rs.update_full(
        "acct-1",
        &saved.id,
        fresh,
        RuleDraft {
            item_id: "item-1".into(),
            sku_key: "single".into(),
            content: String::new(),
            enabled: true,
            content_source: ContentSource::CardPool,
            card_pool_id: Some("pool-real".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let out = service(&h).handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "修复后应执行,{out:?}");
    assert_eq!(h.fake.sent_records().len(), 1);
    assert_eq!(h.fake.sent_records()[0].text, "真卡1");
}

/// 9) T038:需配置类 Ineligible → issues(kind='rule_needs_config',
///    reason_code 区分 needs_confirmation / needs_reconfiguration,
///    allowed_actions 仅 terminate);同单重复触发去重(既有索引)。
#[tokio::test]
async fn rule_needs_config_opens_dedup_issue() {
    let h = harness().await;
    let rs = rule_service(&h);
    // (a) 账号级未确认 → needs_confirmation
    rs.create(
        "acct-1",
        RuleDraft {
            item_id: String::new(),
            sku_key: "single".into(),
            content: "账号级兜底内容".into(),
            enabled: true,
            all_items_confirmed: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    h.fake
        .stage_snapshot(complete_snapshot("ORD-C1", "buyer-1", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-C1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Ineligible { .. }), "{out:?}");
    // 重复触发同单:走既有 pending 分支,仍开同类事项 → 去重为一行
    let out = service(&h).handle_payment("acct-1", "ORD-C1").await.unwrap();
    assert!(matches!(out, HandleOutcome::Ineligible { .. }), "{out:?}");
    let rows = h
        .db
        .call(|conn| {
            Ok::<_, rusqlite::Error>(conn
                .prepare(
                    "SELECT kind, reason_code, allowed_actions FROM issues
                     WHERE order_id = (SELECT id FROM orders WHERE external_order_id = 'ORD-C1')",
                )?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1, "同单同 kind 同 reason 去重:{rows:?}");
    assert_eq!(rows[0].0, "rule_needs_config");
    assert_eq!(rows[0].1, "needs_confirmation");
    assert_eq!(rows[0].2, "[\"terminate\"]");

    // (b) 引用缺失卡组 → needs_reconfiguration
    rs.create(
        "acct-1",
        RuleDraft {
            item_id: "item-1".into(),
            sku_key: "single".into(),
            content: String::new(),
            enabled: true,
            content_source: ContentSource::CardPool,
            card_pool_id: Some("pool-ghost2".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    h.fake
        .stage_snapshot(complete_snapshot("ORD-C2", "buyer-2", 1).build());
    let out = service(&h).handle_payment("acct-1", "ORD-C2").await.unwrap();
    assert!(matches!(out, HandleOutcome::Ineligible { .. }), "{out:?}");
    let reason_code: String = h
        .db
        .call(|conn| {
            conn.query_row(
                "SELECT reason_code FROM issues
                 WHERE order_id = (SELECT id FROM orders WHERE external_order_id = 'ORD-C2')
                   AND kind = 'rule_needs_config'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reason_code, "needs_reconfiguration");

    // (c) 其余不合格原因沿用 delivery_ineligible(不误标):
    //     禁用全部规则后无规则可命中 → NoRule(rule_content_versions 有 FK,禁用不删除)
    h.db
        .call(|conn| {
            conn.execute("UPDATE rules SET enabled = 0 WHERE account_id = 'acct-1'", [])
        })
        .await
        .unwrap()
        .unwrap();
    let out = service(&h).handle_payment("acct-1", "ORD-C2").await.unwrap();
    assert!(matches!(out, HandleOutcome::Ineligible { .. }), "{out:?}");
    let kinds = h
        .db
        .call(|conn| {
            Ok::<Vec<String>, rusqlite::Error>(conn
                .prepare(
                    "SELECT kind FROM issues
                     WHERE order_id = (SELECT id FROM orders WHERE external_order_id = 'ORD-C2')",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(kinds.contains(&"delivery_ineligible".to_string()), "{kinds:?}");
}

/// 007 US3 补齐:规则删除端点用例——乐观锁/NotFound/成功删除。
#[tokio::test]
async fn rule_delete_guards() {
    let h = harness().await;
    let svc = RuleService::new(h.db.clone(), h.key.clone());
    let draft = RuleDraft {
        item_id: "item-1".into(),
        sku_key: String::new(),
        content: "待删除内容".into(),
        enabled: true,
        ..Default::default()
    };
    let saved = svc.create("acct-1", draft).await.unwrap();
    // 错误版本 → 冲突
    assert!(matches!(
        svc.delete("acct-1", &saved.id, saved.version + 1).await,
        Err(RuleError::VersionConflict)
    ));
    // 跨账号 → NotFound(不泄漏存在性)
    assert!(matches!(
        svc.delete("acct-other", &saved.id, saved.version).await,
        Err(RuleError::NotFound)
    ));
    // 正确版本 → 删除成功;再删 → NotFound
    svc.delete("acct-1", &saved.id, saved.version).await.unwrap();
    assert!(matches!(
        svc.delete("acct-1", &saved.id, saved.version).await,
        Err(RuleError::NotFound)
    ));
}
