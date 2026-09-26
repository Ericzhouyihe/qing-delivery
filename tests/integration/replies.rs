//! US3 自动回复分流与求评计划集成测试(T036/T037 转绿):
//! 1) 关键词命中即止:不调 AI、不触默认回复;订单/交付零变化(宪章红线);
//! 2) 关键词图片回复经 ChatSendKind::Image 携带 URL;
//! 3) AI 优先于默认回复;输出截断 2000 字;
//! 4) reply_once 同买家第二次不回;清空记录后恢复;
//! 5) 默认回复 Unknown → Uncertain + 留痕 unknown;
//! 6) 发送失败(Rejected)顺延下一环节(关键词→AI);
//! 7) 求评:未到期不发送;到期发送一次并推进 next_due;
//! 8) 求评:达 max_count 停止(next_due 置空不再排);
//! 9) 求评:账号停用/离线跳过(零发送、零状态行)。

use std::sync::Arc;

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, items, rules_ext};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::catalog::rules::{RuleDraft, RuleService};
use qing_delivery::application::delivery::service::{DeliveryService, HandleOutcome};
use qing_delivery::application::ports::platform::{ChatSendKind, SendOutcome};
use qing_delivery::application::replies::{
    AiReplyProvider, ReplyDispatch, ReplyService, ReplyStage, ReviewReminderService,
};
use qing_delivery::domain::money::Money;
use qing_delivery::domain::orders::snapshot::{FieldStatus, SnapshotBuilder, TradeType};
use qing_delivery::domain::rules_ext::{ReviewConfig, TriggerType};
use qing_delivery::domain::time_util::utc_now_ms;

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: FakeAdapter,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us3-replies.db")).unwrap();
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
    Harness {
        db: db.clone(),
        key,
        fake: FakeAdapter::new(),
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

fn delivery_service(h: &Harness) -> DeliveryService<FakeAdapter> {
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

async fn count(h: &Harness, sql: &str) -> i64 {
    let sql = sql.to_string();
    h.db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(&sql, [], |r| r.get::<_, i64>(0))?)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 记录型 AI 假实现:预设回答只供第一次调用,记录全部入参文本。
struct RecordingAi {
    answer: std::sync::Mutex<Option<String>>,
    calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingAi {
    fn new(answer: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            answer: std::sync::Mutex::new(answer.map(|s| s.to_string())),
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl AiReplyProvider for RecordingAi {
    async fn reply(
        &self,
        _account_id: &str,
        _item_id: Option<&str>,
        _buyer_id: &str,
        text: &str,
    ) -> Result<Option<String>, String> {
        self.calls.lock().unwrap().push(text.to_string());
        Ok(self.answer.lock().unwrap().take())
    }
}

async fn seed_keyword_rule(h: &Harness, id: &str, keyword: &str, kind: &str, text: Option<&str>, image: Option<&str>, item_ids: &[String]) {
    let id = id.to_string();
    let keyword = keyword.to_string();
    let kind = kind.to_string();
    let text = text.map(|s| s.to_string());
    let image = image.map(|s| s.to_string());
    let item_ids = item_ids.to_vec();
    h.db
        .call(move |conn| {
            rules_ext::insert_reply_rule(
                conn,
                &id,
                "acct-1",
                &keyword,
                &kind,
                text.as_deref(),
                image.as_deref(),
                true,
                &item_ids,
            )
        })
        .await
        .unwrap()
        .unwrap();
}

async fn seed_default_reply(h: &Harness) {
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
}

/// 1) 关键词命中即止:不调 AI、不触默认回复;订单/交付零变化。
#[tokio::test]
async fn keyword_reply_hits_and_skips_ai_and_default() {
    let h = harness().await;
    seed_keyword_rule(
        &h,
        "rr-1",
        "发货",
        "text",
        Some("已发货,请查收"),
        None,
        &["item-1".to_string()],
    )
    .await;
    let ai = RecordingAi::new(Some("AI 不应被调用"));
    let rs = ReplyService::with_ai(h.db.clone(), h.fake.clone(), ai.clone());
    let d = rs
        .resolve_and_send("acct-1", Some("item-1"), "buyer-1", Some("chat-1"), "请问什么时候发货呀")
        .await
        .unwrap();
    match &d {
        ReplyDispatch::Sent { stage, .. } => assert_eq!(*stage, ReplyStage::Keyword),
        other => panic!("关键词命中应发送,实际 {other:?}"),
    }
    let sent = h.fake.chat_sent_records();
    assert_eq!(sent.len(), 1, "命中即止,只发一条");
    assert_eq!(sent[0].text, "已发货,请查收");
    assert_eq!(sent[0].buyer_id, "buyer-1");
    assert_eq!(sent[0].chat_id.as_deref(), Some("chat-1"));
    assert!(ai.calls.lock().unwrap().is_empty(), "关键词命中不调 AI");
    // 默认回复未配置也未留痕
    assert_eq!(count(&h, "SELECT COUNT(*) FROM default_reply_log").await, 0);
    // 宪章红线:订单/交付零变化
    assert_eq!(count(&h, "SELECT COUNT(*) FROM orders").await, 0);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM deliveries").await, 0);
}

/// 2) 关键词图片回复:ChatSendKind::Image 携带 URL,正文即 URL(摘要同口径)。
#[tokio::test]
async fn keyword_image_reply_uses_image_kind() {
    let h = harness().await;
    seed_keyword_rule(
        &h,
        "rr-img",
        "清单",
        "image",
        None,
        Some("https://cdn.example.com/list.png"),
        &[],
    )
    .await;
    let rs = ReplyService::new(h.db.clone(), h.fake.clone());
    let d = rs
        .resolve_and_send("acct-1", None, "buyer-2", None, "求清单")
        .await
        .unwrap();
    assert!(matches!(
        &d,
        ReplyDispatch::Sent { stage: ReplyStage::Keyword, .. }
    ));
    let sent = h.fake.chat_sent_records();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].kind,
        ChatSendKind::Image {
            url: "https://cdn.example.com/list.png".into()
        }
    );
    assert_eq!(sent[0].text, "https://cdn.example.com/list.png");
    assert_eq!(sent[0].chat_id, None, "无会话地址按买家号兜底(适配器侧)");
}

/// 3) 未命中关键词 → AI 优先于默认回复;输出截断 2000 字;不写默认留痕。
#[tokio::test]
async fn ai_reply_precedes_default_and_truncates() {
    let h = harness().await;
    seed_default_reply(&h).await;
    let long_answer = "长".repeat(2100);
    let ai = RecordingAi::new(Some(&long_answer));
    let rs = ReplyService::with_ai(h.db.clone(), h.fake.clone(), ai.clone());
    let d = rs
        .resolve_and_send("acct-1", None, "buyer-3", None, "无关文本")
        .await
        .unwrap();
    match &d {
        ReplyDispatch::Sent { stage, .. } => assert_eq!(*stage, ReplyStage::Ai),
        other => panic!("AI 应接管,实际 {other:?}"),
    }
    let sent = h.fake.chat_sent_records();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].text.chars().count(), 2000, "AI 输出截断 2000 字");
    assert!(sent[0].text.chars().all(|c| c == '长'));
    assert_eq!(ai.calls.lock().unwrap().len(), 1);
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM default_reply_log").await,
        0,
        "AI 命中不触默认回复"
    );
}

/// 4) reply_once:同买家第二次不回;清空记录后恢复;全程零订单/交付变化。
#[tokio::test]
async fn default_reply_once_then_clear_records_restores() {
    let h = harness().await;
    seed_default_reply(&h).await;
    let rs = ReplyService::new(h.db.clone(), h.fake.clone());
    // 第一次:发送并留痕 accepted
    let d = rs.resolve_and_send("acct-1", None, "buyer-4", None, "在吗").await.unwrap();
    assert!(matches!(
        &d,
        ReplyDispatch::Sent { stage: ReplyStage::Default, .. }
    ));
    assert_eq!(count(&h, "SELECT COUNT(*) FROM default_reply_log WHERE state='accepted'").await, 1);
    assert_eq!(h.fake.chat_sent_records().len(), 1);
    // 第二次(同买家):reply_once 抑制,不再发送
    let d = rs.resolve_and_send("acct-1", None, "buyer-4", None, "还在吗").await.unwrap();
    assert!(matches!(d, ReplyDispatch::None), "reply_once 二次不回:{d:?}");
    assert_eq!(h.fake.chat_sent_records().len(), 1, "没有第二次发送");
    assert_eq!(count(&h, "SELECT COUNT(*) FROM default_reply_log").await, 1);
    // 清空记录后恢复回复
    h.db
        .call(|conn| rules_ext::clear_default_reply_log(conn, "acct-1"))
        .await
        .unwrap()
        .unwrap();
    let d = rs.resolve_and_send("acct-1", None, "buyer-4", None, "再次询问").await.unwrap();
    assert!(matches!(
        &d,
        ReplyDispatch::Sent { stage: ReplyStage::Default, .. }
    ));
    assert_eq!(h.fake.chat_sent_records().len(), 2);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM default_reply_log WHERE state='accepted'").await, 1);
    // 宪章红线:订单/交付零变化
    assert_eq!(count(&h, "SELECT COUNT(*) FROM orders").await, 0);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM deliveries").await, 0);
}

/// 5) 默认回复 Unknown:返回 Uncertain + 留痕 unknown;不自动重发。
#[tokio::test]
async fn unknown_default_reply_records_uncertain() {
    let h = harness().await;
    seed_default_reply(&h).await;
    h.fake
        .script_chat_send(SendOutcome::Unknown { hint: Some("超时".into()) });
    let rs = ReplyService::new(h.db.clone(), h.fake.clone());
    let d = rs.resolve_and_send("acct-1", None, "buyer-5", None, "在吗").await.unwrap();
    match &d {
        ReplyDispatch::Uncertain { stage, hint, .. } => {
            assert_eq!(*stage, ReplyStage::Default);
            assert_eq!(hint.as_deref(), Some("超时"));
        }
        other => panic!("Unknown 应返回 uncertain,实际 {other:?}"),
    }
    // 留痕 state=unknown;该买家 reply_once 判定不受影响(accepted 才算已回)
    assert_eq!(count(&h, "SELECT COUNT(*) FROM default_reply_log WHERE state='unknown'").await, 1);
    // unknown 不自动重发:同买家再次询问会重新走默认环节(非自动重试同一消息)
    let d = rs.resolve_and_send("acct-1", None, "buyer-5", None, "再说一次").await.unwrap();
    assert!(matches!(
        &d,
        ReplyDispatch::Sent { stage: ReplyStage::Default, .. }
    ));
}

/// 6) 发送失败顺延:关键词 Rejected → AI 环节接管。
#[tokio::test]
async fn keyword_send_failure_falls_through_to_ai() {
    let h = harness().await;
    seed_keyword_rule(
        &h,
        "rr-6",
        "发货",
        "text",
        Some("关键词内容"),
        None,
        &[],
    )
    .await;
    let ai = RecordingAi::new(Some("AI 兜底回复"));
    h.fake.script_chat_send(SendOutcome::Rejected {
        safe_code: "HTTP_403".into(),
    });
    let rs = ReplyService::with_ai(h.db.clone(), h.fake.clone(), ai.clone());
    let d = rs
        .resolve_and_send("acct-1", None, "buyer-6", None, "发货了吗")
        .await
        .unwrap();
    match &d {
        ReplyDispatch::Sent { stage, .. } => assert_eq!(*stage, ReplyStage::Ai),
        other => panic!("关键词失败应顺延 AI,实际 {other:?}"),
    }
    let sent = h.fake.chat_sent_records();
    assert_eq!(sent.len(), 2, "关键词一次(被拒)+ AI 一次(接纳)");
    assert_eq!(sent[1].text, "AI 兜底回复");
}

// ---------- T037 求评计划 ----------

/// 建一笔已接纳交付 + 账号级求评规则(wait 1h/interval 2h/max 3),
/// 返回 (订单库内 ID, 求评规则 ID)。
async fn seed_accepted_order_with_review_rule(
    h: &Harness,
    order: &str,
    buyer: &str,
) -> (String, String) {
    let rs = rule_service(h);
    rs.create(
        "acct-1",
        RuleDraft {
            item_id: "item-1".into(),
            sku_key: "single".into(),
            content: "感谢购买".into(),
            enabled: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    h.fake
        .stage_snapshot(complete_snapshot(order, buyer, 1).build());
    let out = delivery_service(h).handle_payment("acct-1", order).await.unwrap();
    assert!(matches!(out, HandleOutcome::Delivered { .. }), "{out:?}");
    // 账号级求评规则(须确认「适用于全部商品」)
    let review_rule = rs
        .create(
            "acct-1",
            RuleDraft {
                item_id: String::new(),
                sku_key: "single".into(),
                // 固定内容来源要求非空正文;求评触发不消费该列,仅占位过校验
                content: "(求评规则占位正文)".into(),
                enabled: true,
                trigger_type: TriggerType::ReviewMissingTimeout,
                review_config: Some(ReviewConfig {
                    wait_hours: 1,
                    interval_hours: 2,
                    max_count: 3,
                    text: "亲,期待您的评价".into(),
                }),
                all_items_confirmed: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let order_db_id: String = h
        .db
        .call({
            let order = order.to_string();
            move |conn| {
                Ok::<_, rusqlite::Error>(conn.query_row(
                    "SELECT id FROM orders WHERE external_order_id = ?1",
                    rusqlite::params![order],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap()
        .unwrap();
    (order_db_id, review_rule.id)
}

async fn backdate_delivery(h: &Harness, hours: i64) {
    h.db
        .call(move |conn| {
            conn.execute(
                "UPDATE deliveries SET updated_at = ?1",
                rusqlite::params![utc_now_ms() - hours * 3_600_000],
            )
        })
        .await
        .unwrap()
        .unwrap();
}

/// 7) 未到期不发送;到期发送一次并推进 next_due;立即再跑不重复。
#[tokio::test]
async fn review_reminder_due_sends_once_and_advances_next_due() {
    let h = harness().await;
    let (order_db_id, _) = seed_accepted_order_with_review_rule(&h, "ORD-R1", "buyer-r1").await;
    let rem = ReviewReminderService::new(h.db.clone(), h.fake.clone());

    // 未到期(交付刚发生,wait_hours=1):不发送、不建状态行
    assert_eq!(rem.run_once().await.unwrap(), 0);
    assert!(h.fake.chat_sent_records().is_empty());
    assert_eq!(count(&h, "SELECT COUNT(*) FROM review_reminder_state").await, 0);

    // 回拨交付时间 2 小时 → 到期:发送一次并推进
    backdate_delivery(&h, 2).await;
    assert_eq!(rem.run_once().await.unwrap(), 1);
    let chat = h.fake.chat_sent_records();
    assert_eq!(chat.len(), 1);
    assert_eq!(chat[0].text, "亲,期待您的评价");
    assert_eq!(chat[0].buyer_id, "buyer-r1");
    let state = h
        .db
        .call({
            let order_db_id = order_db_id.clone();
            move |conn| {
                Ok::<_, rusqlite::Error>(
                    conn.query_row(
                        "SELECT reminded_count, next_due_at FROM review_reminder_state
                         WHERE order_db_id = ?1",
                        rusqlite::params![order_db_id],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
                    )?,
                )
            }
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.0, 1, "成功一次计数 +1");
    let next = state.1.expect("next_due_at = now + interval_hours(2)");
    assert!(next > qing_delivery::domain::time_util::format_rfc3339(utc_now_ms()));

    // 立即再跑:next_due 未到 → 不重复
    assert_eq!(rem.run_once().await.unwrap(), 0);
    assert_eq!(h.fake.chat_sent_records().len(), 1);
}

/// 8) 达 max_count 停止:推进到上限后 next_due 置空,不再排期/发送。
#[tokio::test]
async fn review_reminder_stops_at_max_count() {
    let h = harness().await;
    let (order_db_id, rule_id) =
        seed_accepted_order_with_review_rule(&h, "ORD-R2", "buyer-r2").await;
    // 直接把状态行排到"已提醒 2 次(max=3)且已到期"
    h.db
        .call({
            let order_db_id = order_db_id.clone();
            let rule_id = rule_id.clone();
            move |conn| {
                rules_ext::upsert_reminder(
                    conn,
                    &order_db_id,
                    &rule_id,
                    2,
                    None,
                    Some("2000-01-01T00:00:00.000Z"),
                )
            }
        })
        .await
        .unwrap()
        .unwrap();
    let rem = ReviewReminderService::new(h.db.clone(), h.fake.clone());
    assert_eq!(rem.run_once().await.unwrap(), 1, "第 3 次到期仍发送");
    let state = h
        .db
        .call({
            let order_db_id = order_db_id.clone();
            move |conn| {
                Ok::<_, rusqlite::Error>(conn.query_row(
                    "SELECT reminded_count, next_due_at FROM review_reminder_state
                     WHERE order_db_id = ?1",
                    rusqlite::params![order_db_id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
                )?)
            }
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.0, 3, "计数到 max_count");
    assert!(state.1.is_none(), "达上限不再排期");
    // 再跑(即使人为再排到期,防御分支也拦):零发送
    h.db
        .call({
            let order_db_id = order_db_id.clone();
            move |conn| {
                conn.execute(
                    "UPDATE review_reminder_state SET next_due_at = '2000-01-01T00:00:00.000Z'
                     WHERE order_db_id = ?1",
                    rusqlite::params![order_db_id],
                )
            }
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rem.run_once().await.unwrap(), 0, "达上限后不再发送");
    assert_eq!(h.fake.chat_sent_records().len(), 1);
}

/// 9) 账号停用(runtime_enabled=0)或离线(status != online):跳过留痕,
///    零发送、零状态行。
#[tokio::test]
async fn review_reminder_skips_disabled_or_offline_account() {
    let h = harness().await;
    seed_accepted_order_with_review_rule(&h, "ORD-R3", "buyer-r3").await;
    backdate_delivery(&h, 2).await;
    h.db
        .call(|conn| {
            conn.execute(
                "UPDATE accounts SET runtime_enabled = 0 WHERE id = 'acct-1'",
                [],
            )
        })
        .await
        .unwrap()
        .unwrap();
    let rem = ReviewReminderService::new(h.db.clone(), h.fake.clone());
    assert_eq!(rem.run_once().await.unwrap(), 0);
    assert!(h.fake.chat_sent_records().is_empty(), "停用账号不发送");
    assert_eq!(count(&h, "SELECT COUNT(*) FROM review_reminder_state").await, 0);

    // 恢复启用但状态非 online(离线):同样跳过
    h.db
        .call(|conn| {
            conn.execute(
                "UPDATE accounts SET runtime_enabled = 1, status = 'offline' WHERE id = 'acct-1'",
                [],
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rem.run_once().await.unwrap(), 0);
    assert!(h.fake.chat_sent_records().is_empty());
    assert_eq!(count(&h, "SELECT COUNT(*) FROM review_reminder_state").await, 0);
}
