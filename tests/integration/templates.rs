//! US2 发货模板集成测试(T021/T023/T024):
//! 模板 CRUD(创建/整体替换消息/乐观锁)、被引用删除拒绝(rules.template_id
//! 与 rule_variants.template_id 两处引用,引用规则标记需重新配置)、
//! 渲染(系统变量+custom+卡密绑定;掩码预览与真值发送共用同一函数)、
//! 多消息顺序发送(mock:3 条消息→3 行 delivery_proofs 同 attempt_id;
//! 第 2 条 Rejected→已发 1 条留 proof、attempt 落 not_sent、重试仅从第 2 条
//! 续发不重发第 1 条;Unknown→uncertain 处置与既有单条语义一致;
//! 余量不足→stock_insufficient 不部分交付)。

use std::collections::{HashMap, HashSet};

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, cards, items, rules, templates};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::delivery::service::{DeliveryService, HandleOutcome};
use qing_delivery::application::ports::platform::{SendOutcome, SendProof};
use qing_delivery::application::templates::{
    PreviewVars, TemplateDraft, TemplateService, TemplatesError,
};
use qing_delivery::domain::crypto::{self, Aad, Envelope};
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{FieldStatus, SnapshotBuilder, TradeType};
use qing_delivery::domain::templates::{
    RenderCard, RenderContext, TemplateBindings, render_messages,
};
use rusqlite::OptionalExtension as _;
use sha2::Digest as _;

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

const TEMPLATE_ID: &str = "tpl-1";
const TEMPLATE_MESSAGES: [&str; 3] = [
    "感谢{{buyer_nickname}}购买{{card_name}}",
    "您的卡密:{{cards.key1}}",
    "订单{{order_id}}备查,备注:{{custom.note}}",
];

fn bindings_json(units: i64) -> String {
    format!(
        r#"{{"cards":[{{"key":"key1","pool_id":"pool-1","units":{units}}}],"custom":{{"note":"谢谢支持"}}}}"#
    )
}

/// 冻结阶段(快照可信事实 + 预留最早条目)应渲染出的真值消息。
fn expected_rendered(card: &str) -> Vec<String> {
    vec![
        "感谢buyer-1购买考研资料".to_string(),
        format!("您的卡密:{card}"),
        "订单ORD-1备查,备注:谢谢支持".to_string(),
    ]
}

fn digest_of(text: &str) -> String {
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

/// 交付流测试环境:在线账号 + 单规格商品 + 启用规则(template_id=tpl-1
/// + template_bindings:key1 绑 data 池 units 份、custom.note 赋值)+
/// 三消息模板 + data 池(quantity 件 × units 份)。
async fn template_harness(texts: &[&str], units: i64, pool_enabled: bool) -> Harness {
    let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("templates-it.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();

    let key_clone = key.key;
    let key_id = key.key_id.clone();
    let messages: Vec<String> = TEMPLATE_MESSAGES.iter().map(|s| s.to_string()).collect();
    let bindings = bindings_json(units);
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
        // 商品 + 启用规则绑定模板
        let item = items::upsert(
            conn, "item-1", "acct-1", "EXT-ITEM-1", "考研资料", "on_sale", "[]", "single",
        )?;
        rules::insert(conn, "rule-1", "acct-1", &item.id, "single", true)?;
        conn.execute(
            "UPDATE rules SET template_id = ?2, template_bindings = ?3 WHERE id = 'rule-1'",
            rusqlite::params![TEMPLATE_ID, TEMPLATE_ID, bindings],
        )?;
        // 三消息模板(明文占位符,不含机密)
        templates::insert_template(conn, TEMPLATE_ID, "标准发货模板", true, &messages)?;
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
            let aad = Aad {
                purpose: "card_entry".into(),
                entity_id: entry_id.clone(),
                content_version: None,
            };
            let envelope = crypto::seal(&key_clone, &key_id, &aad, text.as_bytes());
            cards::append_entries(
                conn,
                "pool-1",
                &[cards::NewEntry {
                    id: &entry_id,
                    envelope: &envelope,
                    content_digest: &digest_of(text),
                }],
            )?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();

    let builder = complete_snapshot("ORD-1", "buyer-1", 1);
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

async fn content_state(h: &Harness, delivery_id: &str) -> String {
    let id = delivery_id.to_string();
    h.db
        .call(move |conn| {
            conn.query_row(
                "SELECT content_state FROM deliveries WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap()
}

/// 该交付全部 proof(content_digest, attempt_id),按落库顺序。
async fn proofs_of(h: &Harness, delivery_id: &str) -> Vec<(String, String)> {
    let id = delivery_id.to_string();
    h.db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT dp.content_digest, dp.attempt_id FROM delivery_proofs dp
                 JOIN attempts a ON a.id = dp.attempt_id
                 WHERE a.delivery_id = ?1 ORDER BY dp.rowid",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 该交付全部 attempt(state, result_code),按序号。
async fn attempts_of(h: &Harness, delivery_id: &str) -> Vec<(String, Option<String>)> {
    let id = delivery_id.to_string();
    h.db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT state, result_code FROM attempts WHERE delivery_id = ?1 ORDER BY sequence",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 快照 source_content_id 与解密后的消息 JSON 数组(验证与实际发送逐字一致)。
async fn snapshot_of(h: &Harness, delivery_id: &str) -> (String, Vec<String>) {
    let id = delivery_id.to_string();
    let row = h
        .db
        .call(move |conn| {
            conn.query_row(
                "SELECT cs.id, cs.source_content_id, cs.ciphertext, cs.nonce, cs.key_id,
                        cs.format_version
                 FROM content_snapshots cs JOIN deliveries d ON d.content_snapshot_id = cs.id
                 WHERE d.id = ?1",
                rusqlite::params![id],
                |r| {
                    let nonce: Vec<u8> = r.get(3)?;
                    let mut fixed = [0u8; crypto::NONCE_LEN];
                    if nonce.len() == fixed.len() {
                        fixed.copy_from_slice(&nonce);
                    }
                    Ok::<_, rusqlite::Error>((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        Envelope {
                            ciphertext: r.get(2)?,
                            nonce: fixed,
                            key_id: r.get(4)?,
                            format_version: r.get::<_, i64>(5)? as u16,
                        },
                    ))
                },
            )
            .optional()
        })
        .await
        .unwrap()
        .unwrap()
        .expect("应有内容快照");
    let aad = Aad {
        purpose: "content_snapshot".into(),
        entity_id: row.0.clone(),
        content_version: None,
    };
    let plain = crypto::open(&h.key.key, &aad, &row.2).unwrap();
    let messages = serde_json::from_slice(&plain).unwrap();
    (row.1, messages)
}

/// (kind, reason_code) of open issue for order;None if absent.
async fn open_issue(h: &Harness, external_order: &str) -> Option<(String, String)> {
    let ext = external_order.to_string();
    h.db
        .call(move |conn| {
            conn.query_row(
                "SELECT i.kind, i.reason_code FROM issues i
                 JOIN orders o ON o.id = i.order_id
                 WHERE i.state = 'open' AND o.external_order_id = ?1 LIMIT 1",
                rusqlite::params![ext],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .await
        .unwrap()
        .unwrap()
}

fn accepted_stub() -> SendOutcome {
    // FakeAdapter 会重写 request_id/digest/buyer 保证严格关联
    SendOutcome::Accepted(SendProof {
        request_id: "stub".into(),
        platform_message_id: None,
        buyer_id: String::new(),
        chat_id: None,
        content_digest: String::new(),
    })
}

// ---- T021-1:模板 CRUD(创建/整体替换/乐观锁) ----

#[tokio::test]
async fn 模板服务创建整体替换与乐观锁() {
    let h = template_harness(&[], 1, true).await;
    let svc = TemplateService::new(h.db.clone());

    let created = svc
        .create(TemplateDraft {
            name: "晚发货模板".into(),
            enabled: true,
            messages: vec![
                "第一句 {{buyer_nickname}}".into(),
                "第二句 {{cards.k1}} {{custom.c1}}".into(),
            ],
        })
        .await
        .unwrap();
    assert_eq!(created.version, 1);
    assert_eq!(created.messages.len(), 2);
    assert_eq!(created.keys.cards, vec!["k1".to_string()]);
    assert_eq!(created.keys.custom, vec!["c1".to_string()]);
    assert_eq!(created.used_by_rules, 0);

    // 更新:消息整体替换(旧消息不复存在),name/enabled 同步
    let updated = svc
        .update(
            &created.id,
            created.version,
            Some("改名".into()),
            Some(false),
            Some(vec!["整体替换后唯一一条 {{custom.note}}".into()]),
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert!(!updated.enabled);
    assert_eq!(
        updated.messages,
        vec!["整体替换后唯一一条 {{custom.note}}".to_string()]
    );
    assert_eq!(updated.keys.custom, vec!["note".to_string()]);
    let db_messages = h
        .db
        .call({
            let id = created.id.clone();
            move |conn| templates::list_messages(conn, &id)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db_messages.len(), 1, "整体替换后旧消息必须清除");

    // 乐观锁:过期版本拒绝
    let err = svc
        .update(
            &created.id,
            created.version,
            None,
            None,
            Some(vec!["x {{order_id}}".into()]),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TemplatesError::VersionConflict), "{err:?}");

    // 校验:占位符非法 / 0 条消息 / 名字为空 → InvalidRequest(422 语义)
    for draft in [
        TemplateDraft {
            name: "坏".into(),
            enabled: true,
            messages: vec!["{{cards.非 法}}".into()],
        },
        TemplateDraft {
            name: "空".into(),
            enabled: true,
            messages: vec![],
        },
        TemplateDraft {
            name: "  ".into(),
            enabled: true,
            messages: vec!["合法 {{order_id}}".into()],
        },
    ] {
        let err = svc.create(draft).await.unwrap_err();
        assert!(matches!(err, TemplatesError::InvalidRequest(_)), "{err:?}");
    }

    // 详情与列表:列表含 harness 模板且 used_by_rules=1(rule-1 引用)
    let got = svc.get(&created.id).await.unwrap();
    assert_eq!(got.name, "改名");
    let list = svc.list(None).await.unwrap();
    assert!(list.iter().any(|t| t.id == created.id));
    let tpl = list
        .iter()
        .find(|t| t.id == TEMPLATE_ID)
        .expect("列表应含 harness 模板");
    assert_eq!(tpl.used_by_rules, 1);
    assert_eq!(tpl.messages.len(), 3);

    // 删除未引用模板;消息级联清理
    let deleted_id = created.id.clone();
    svc.delete(&created.id).await.unwrap();
    assert!(matches!(
        svc.get(&created.id).await,
        Err(TemplatesError::NotFound)
    ));
    let left: i64 = h
        .db
        .call(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM delivery_template_messages WHERE template_id = ?1",
                rusqlite::params![deleted_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(left, 0);
}

// ---- T021-2:被引用删除(规则级 + 变体级) ----

#[tokio::test]
async fn 被规则或变体引用的模板删除被拒并标记需重新配置() {
    let h = template_harness(&["CARD-A"], 1, true).await;
    let svc = TemplateService::new(h.db.clone());

    // 规则级引用(rules.template_id)
    let err = svc.delete(TEMPLATE_ID).await.unwrap_err();
    let TemplatesError::Referenced(rule_ids) = &err else {
        panic!("应返回引用冲突,实际 {err:?}")
    };
    assert!(rule_ids.contains(&"rule-1".to_string()), "{rule_ids:?}");
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
    assert_eq!(flagged, 1, "拒绝删除时引用规则应标记需重新配置");
    // 模板与消息原样保留(未删除)
    assert!(svc.get(TEMPLATE_ID).await.is_ok());

    // 解除规则级引用 → 变体级引用(rule_variants.template_id)同样拒绝
    h.db
        .call(|conn| {
            conn.execute("UPDATE rules SET template_id = NULL WHERE id='rule-1'", [])?;
            conn.execute(
                "INSERT INTO rule_variants(id, rule_id, spec_name, spec_value, source,
                     template_id, template_bindings, units_per_item, position)
                 VALUES ('rv-1', 'rule-1', '规格', 'A', 'template', ?1, NULL, 2, 1)",
                rusqlite::params![TEMPLATE_ID],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
    let err = svc.delete(TEMPLATE_ID).await.unwrap_err();
    let TemplatesError::Referenced(rule_ids) = &err else {
        panic!("变体引用同样应拒绝,实际 {err:?}")
    };
    assert!(rule_ids.contains(&"rule-1".to_string()), "{rule_ids:?}");

    // 全部解除后可删;消息级联清理
    h.db
        .call(|conn| conn.execute("DELETE FROM rule_variants WHERE id='rv-1'", []))
        .await
        .unwrap()
        .unwrap();
    svc.delete(TEMPLATE_ID).await.unwrap();
    let left: i64 = h
        .db
        .call(|conn| {
            conn.query_row("SELECT COUNT(*) FROM delivery_template_messages", [], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(left, 0, "删除模板应级联清理消息");
}

// ---- T021-3:渲染(掩码预览与真值共用同一函数) ----

#[tokio::test]
async fn 掩码预览与真值渲染共用同一函数() {
    let h = template_harness(&["CARD-A"], 1, true).await;
    let svc = TemplateService::new(h.db.clone());
    let bindings: TemplateBindings = serde_json::from_str(&bindings_json(1)).unwrap();
    let vars = PreviewVars {
        buyer_nickname: "买家小王".into(),
        order_id: "ORD-9".into(),
        buyer_id: "buyer-9".into(),
        card_name: "考研资料".into(),
    };
    let masked = svc
        .render_preview(TEMPLATE_ID, &bindings, vars.clone())
        .await
        .unwrap();
    assert_eq!(
        masked,
        vec![
            "感谢买家小王购买考研资料".to_string(),
            "您的卡密:[卡密内容 ×1]".to_string(),
            "订单ORD-9备查,备注:谢谢支持".to_string(),
        ]
    );

    // 同一渲染函数真值模式:除卡密值外逐字一致(预览=发送)
    let messages: Vec<String> = TEMPLATE_MESSAGES.iter().map(|s| s.to_string()).collect();
    let real = render_messages(
        &messages,
        &RenderContext {
            buyer_nickname: "买家小王".into(),
            order_id: "ORD-9".into(),
            buyer_id: "buyer-9".into(),
            card_name: "考研资料".into(),
            cards: HashMap::from([(
                "key1".to_string(),
                RenderCard {
                    text: "CARD-A".into(),
                    count: 1,
                },
            )]),
            custom: HashMap::from([("note".to_string(), "谢谢支持".into())]),
        },
        false,
    )
    .unwrap();
    assert_eq!(masked[0], real[0]);
    assert_eq!(masked[2], real[2]);
    assert_ne!(masked[1], real[1], "卡密值掩码替换");

    // custom 未赋值 → 拒绝(变量缺失)
    let err = svc
        .render_preview(TEMPLATE_ID, &TemplateBindings::default(), vars.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, TemplatesError::InvalidRequest(_)), "{err:?}");
    // 模板不存在
    let err = svc
        .render_preview("tpl-404", &bindings, vars)
        .await
        .unwrap_err();
    assert!(matches!(err, TemplatesError::NotFound), "{err:?}");
}

// ---- T021-4:多消息顺序发送,逐条 proof 同 attempt ----

#[tokio::test]
async fn 三条消息顺序发送逐条proof同attempt() {
    let h = template_harness(&["CARD-A", "CARD-B"], 1, true).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Delivered { delivery_id } = out else {
        panic!("应交付成功,实际 {out:?}")
    };
    // 三条消息逐条顺序发送(预留最早可用条 CARD-A)
    let expected = expected_rendered("CARD-A");
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 3, "三条消息逐条发送");
    let texts: Vec<&str> = sent.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(
        texts,
        expected.iter().map(String::as_str).collect::<Vec<_>>(),
        "发送内容应与渲染结果逐字一致"
    );
    // 每条 text_digest = 该条消息摘要
    for (r, m) in sent.iter().zip(expected.iter()) {
        assert_eq!(r.text_digest, digest_of(m));
    }
    // 每条 Accepted 各一行 proof,同 attempt_id,content_digest=该条摘要
    let proofs = proofs_of(&h, &delivery_id).await;
    assert_eq!(proofs.len(), 3, "3 条消息 → 3 行 proof");
    let attempt_ids: HashSet<&String> = proofs.iter().map(|(_, a)| a).collect();
    assert_eq!(attempt_ids.len(), 1, "同一 attempt 多行 proof");
    let expected_digests: HashSet<String> = expected.iter().map(|m| digest_of(m)).collect();
    for (digest, _) in &proofs {
        assert!(expected_digests.contains(digest), "digest={digest}");
    }
    // attempt accepted;交付 accepted
    let attempts = attempts_of(&h, &delivery_id).await;
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].0, "accepted");
    assert_eq!(content_state(&h, &delivery_id).await, "accepted");
    // 快照:JSON 消息数组与实际发送逐字一致;source_content_id=template:{id}
    let (source, messages) = snapshot_of(&h, &delivery_id).await;
    assert_eq!(source, format!("template:{TEMPLATE_ID}"));
    assert_eq!(messages, expected);
    // 卡密扣减:used 1、余量 1
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (1, 0, 1));
}

// ---- T021-5:第 2 条 Rejected → 已发 1 条留 proof,重试仅续发未确认条 ----

#[tokio::test]
async fn 第二条Rejected保留proof重试仅从未确认条继续() {
    let h = template_harness(&["CARD-A"], 1, true).await;
    h.fake.script_send(accepted_stub());
    h.fake
        .script_send(SendOutcome::Rejected { safe_code: "content_blocked".into() });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::NotSent {
        delivery_id,
        reason,
        ..
    } = out
    else {
        panic!("应 not_sent,实际 {out:?}")
    };
    assert_eq!(reason.as_deref(), Some("content_blocked"));
    let expected = expected_rendered("CARD-A");

    // 第 1 条已发并留 proof;第 2 条尝试发送被拒(无 proof);attempt 落 not_sent
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 2, "第 2 条尝试发送但被拒(共 2 次发送尝试)");
    assert_eq!(sent[0].text, expected[0]);
    assert_eq!(sent[1].text, expected[1], "被拒条目是第 2 条");
    let proofs = proofs_of(&h, &delivery_id).await;
    assert_eq!(proofs.len(), 1, "仅已确认条保留 proof");
    assert_eq!(proofs[0].0, digest_of(&expected[0]));
    let attempts = attempts_of(&h, &delivery_id).await;
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].0, "not_sent");
    assert_eq!(attempts[0].1.as_deref(), Some("content_blocked"));
    assert_eq!(content_state(&h, &delivery_id).await, "not_sent");
    // 平台拒绝:预留不释放(同单条语义,人工补发复用同一快照同一张卡)
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (0, 1, 0));

    // 重试:全部 Accepted → 不重发第 1 条,仅从第 2 条续发
    let out2 = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out2, HandleOutcome::Delivered { .. }),
        "实际 {out2:?}"
    );
    let sent = h.fake.sent_records();
    assert_eq!(sent.len(), 4, "重试只补发未确认 2 条(总尝试 4 次)");
    assert_eq!(sent[0].text, expected[0], "第 1 条只发送一次");
    assert!(
        sent[2..].iter().all(|r| r.text != expected[0]),
        "重试不得重发已确认的第 1 条"
    );
    assert_eq!(sent[2].text, expected[1]);
    assert_eq!(sent[3].text, expected[2]);
    // proofs:两条 attempt 合计 3 行;重试 attempt 独立且 accepted
    let proofs = proofs_of(&h, &delivery_id).await;
    assert_eq!(proofs.len(), 3);
    let attempt_ids: HashSet<&String> = proofs.iter().map(|(_, a)| a).collect();
    assert_eq!(attempt_ids.len(), 2, "重试是独立 attempt");
    let attempts = attempts_of(&h, &delivery_id).await;
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].0, "accepted");
    assert_eq!(content_state(&h, &delivery_id).await, "accepted");
    // 冻结幂等:重试不重复取卡
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (0, 0, 1));
}

// ---- T021-6:中途 Unknown → uncertain,与既有单条语义一致 ----

#[tokio::test]
async fn 中途Unknown转人工不自动重发() {
    let h = template_harness(&["CARD-A"], 1, true).await;
    h.fake.script_send(accepted_stub());
    h.fake.script_send(SendOutcome::Unknown {
        hint: Some("timeout".into()),
    });
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    let HandleOutcome::Unknown { delivery_id } = out else {
        panic!("应 unknown,实际 {out:?}")
    };
    // 第 1 条 Accepted 留 proof;第 2 条结果未知(已尝试发送,无 proof)
    let proofs = proofs_of(&h, &delivery_id).await;
    assert_eq!(proofs.len(), 1);
    let attempts = attempts_of(&h, &delivery_id).await;
    assert_eq!(attempts[0].0, "unknown");
    assert_eq!(content_state(&h, &delivery_id).await, "unknown");
    let issue = open_issue(&h, "ORD-1").await.expect("应开 delivery_unknown");
    assert_eq!(issue.0, "delivery_unknown");
    // 宪章 I:结果未知不回库、不换卡、不自动重发
    let stock = stock_of(&h).await;
    assert_eq!((stock.available, stock.reserved, stock.used), (0, 1, 0));
    let again = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert_eq!(again, HandleOutcome::AlreadyHandled);
    let stock = stock_of(&h).await;
    assert_eq!(stock.reserved, 1, "未知结果禁止释放/重配");
}

// ---- T021-7:余量不足 → stock_insufficient,不部分交付 ----

#[tokio::test]
async fn 卡密余量不足整体回滚转待处理() {
    // units=2、库存仅 1 条 → 预留不足整体回滚
    let h = template_harness(&["CARD-A"], 2, true).await;
    let svc = service(&h);
    let out = svc.handle_payment("acct-1", "ORD-1").await.unwrap();
    assert!(
        matches!(out, HandleOutcome::AlreadyHandled),
        "实际 {out:?}"
    );
    let issue = open_issue(&h, "ORD-1").await.expect("应开 stock_insufficient");
    assert_eq!(issue.0, "stock_insufficient");
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
}
