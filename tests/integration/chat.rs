//! US4 在线聊天集成测试(T044/T050/T051 转绿):
//! 1) 同一 WS 消息经事件管道与 ChatService::ingest 双路径不重复落库(FR-046);
//! 2) 入站文本触发 replies 分流(US3-B 通电):出站行 sent,零订单/交付写入;
//! 3) 发送四状态流转:sending→sent;NotSubmitted→failed;Unknown→uncertain;
//! 4) unknown(uncertain)消息 retry 被拒(unsafe_retry);failed 可人工重试(新行);
//! 5) OutgoingMessageEvidence 回填:uncertain→sent + platform_message_id 回填;
//! 6) 未读:入站聚合、系统消息不加红点、打开会话清零、全局/按账号汇总;
//! 7) 删除会话:本机隐藏、消息物理清空、未读不再计入;
//! 8) 快捷回复 50 上限:第 51 条 reply_limit_reached;
//! 9) 图片发送(mock 完整链路):文件落 uploads/chat/、白名单、10MB 上限、
//!    能力门禁 false(live)拒绝;
//! 10) 离线账号发送 → account_unavailable 语义。

use qing_delivery::adapters::mock::fake::FakeAdapter;
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::{accounts, items, rules_ext};
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::chat::{
    ChatError, ChatService, IngestOutcome, IncomingChatMessage,
};
use qing_delivery::application::events::EventPipeline;
use qing_delivery::application::ports::platform::{CapabilitySet, PlatformEvent, SendOutcome};
use qing_delivery::domain::chat::ChatMsgKind;
use qing_delivery::domain::time_util::utc_now_ms;

struct Harness {
    db: DbThread,
    fake: FakeAdapter,
    uploads_root: std::path::PathBuf,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us4-chat.db")).unwrap();
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
            conn,
            "item-1",
            "acct-1",
            "EXT-ITEM-1",
            "考研资料",
            "on_sale",
            "[]",
            "single",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let uploads_root = data_dir.root.join("uploads");
    Harness {
        db: db.clone(),
        fake: FakeAdapter::new(),
        uploads_root: uploads_root.clone(),
        _dir: dir,
        _lock: lock,
    }
}

/// mock 能力档(chat_send_image=true)的聊天服务。
fn chat_service(h: &Harness) -> ChatService<FakeAdapter> {
    ChatService::new(h.db.clone(), h.fake.clone())
        .with_storage(h.uploads_root.clone(), CapabilitySet::MOCK)
}

/// live 能力档(图片发送未验证=false)。
fn live_caps_service(h: &Harness) -> ChatService<FakeAdapter> {
    ChatService::new(h.db.clone(), h.fake.clone())
        .with_storage(h.uploads_root.clone(), CapabilitySet::LIVE)
}

fn incoming(id: &str, text: &str) -> IncomingChatMessage {
    IncomingChatMessage {
        account_id: "acct-1".into(),
        platform_message_id: id.into(),
        chat_id: Some("chat-1".into()),
        buyer_id: "buyer-1".into(),
        msg_kind: ChatMsgKind::Text,
        text: Some(text.into()),
        image_url: None,
        item_id: Some("item-1".into()),
    }
}

async fn count(h: &Harness, sql: &str) -> i64 {
    let sql = sql.to_string();
    h.db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(sql.as_str(), [], |r| r.get::<_, i64>(0))?)
        })
        .await
        .unwrap()
        .unwrap()
}

async fn column(h: &Harness, sql: &str) -> String {
    let sql = sql.to_string();
    h.db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(sql.as_str(), [], |r| r.get::<_, String>(0))?)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 1) 双路径不重复落库:事件管道(Observed 记录)+ ChatService::ingest;
///    同 platform_message_id 重复摄取只有一行,未读只 +1。
#[tokio::test]
async fn dual_path_pipeline_and_ingest_no_duplicate_rows() {
    let h = harness().await;
    let chat = chat_service(&h);
    // mock 适配器产出口径:staged buyer chat → ChatMessageReceived 事件
    h.fake.stage_buyer_chat(qing_delivery::adapters::mock::fake::StagedBuyerChat {
        platform_message_id: "pm-1".into(),
        chat_id: Some("chat-1".into()),
        buyer_id: "buyer-1".into(),
        kind: ChatMsgKind::Text,
        text: Some("你好".into()),
        image_url: None,
        item_id: None,
    });
    let events = h.fake.chat_message_events("acct-1", 1);
    assert_eq!(events.len(), 1);
    let PlatformEvent::ChatMessageReceived { .. } = &events[0] else {
        panic!("应为 ChatMessageReceived 事件");
    };
    // 路径 A:事件管道(持久化去重记录;聊天摄取由 dispatch 分支承担)
    let pipeline = EventPipeline::new(h.db.clone());
    let out = pipeline.process(&events[0]).await.unwrap();
    assert!(
        matches!(out, qing_delivery::application::events::EventOutcome::Observed),
        "管道对聊天事件为 Observed:{out:?}"
    );
    // 路径 B:ChatService::ingest(同一条消息)
    assert_eq!(
        chat.ingest(incoming("pm-1", "你好")).await.unwrap(),
        IngestOutcome::Stored
    );
    // 重复投递(WS 重发/轮询再来一次):Duplicate,零新行
    assert_eq!(
        chat.ingest(incoming("pm-1", "你好")).await.unwrap(),
        IngestOutcome::Duplicate
    );
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM chat_messages WHERE platform_message_id='pm-1'").await,
        1,
        "双路径+重复投递只有一行"
    );
    // 未读只 +1(重复不重算)
    assert_eq!(
        count(&h, "SELECT unread_count FROM conversations WHERE peer_buyer_id='buyer-1'").await,
        1
    );
}

/// 2) 入站文本触发自动回复(US3-B 通电):出站行 status=sent,
///    发送结果只影响 chat_messages,零订单/交付写入(宪章红线)。
#[tokio::test]
async fn ingest_text_triggers_reply_and_writes_no_orders() {
    let h = harness().await;
    h.db
        .call(|conn| {
            rules_ext::insert_reply_rule(
                conn,
                "rr-chat",
                "acct-1",
                "发货",
                "text",
                Some("已发货,请查收"),
                None,
                true,
                &["item-1".to_string()],
            )
        })
        .await
        .unwrap()
        .unwrap();
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-2", "请问什么时候发货呀")).await.unwrap();
    // 自动回复经 send_chat_message 发出
    let sent = h.fake.chat_sent_records();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].text, "已发货,请查收");
    assert_eq!(sent[0].buyer_id, "buyer-1");
    // 出站留痕:status=sent + 平台消息 ID 回填
    assert_eq!(
        column(
            &h,
            "SELECT status FROM chat_messages WHERE direction='out' ORDER BY created_at DESC LIMIT 1"
        )
        .await,
        "sent"
    );
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM chat_messages WHERE direction='out' AND platform_message_id IS NOT NULL").await,
        1
    );
    // 宪章红线:零订单/交付写入
    assert_eq!(count(&h, "SELECT COUNT(*) FROM orders").await, 0);
    assert_eq!(count(&h, "SELECT COUNT(*) FROM deliveries").await, 0);
}

/// 消息游标分页(新→旧,"加载更早"):limit 截断 + before_id 续页不重不漏。
#[tokio::test]
async fn message_cursor_pagination_new_to_old() {
    let h = harness().await;
    let chat = chat_service(&h);
    for i in 1..=5 {
        chat.ingest(incoming(&format!("pm-p{i}"), &format!("第{i}条"))).await.unwrap();
    }
    let conv = conv_of(&h).await;
    // 第一页:最新 limit 条 + next_cursor
    let (page1, next) = chat.list_messages(&conv, None, 2).await.unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = next.expect("还有更早消息");
    // 续页两页取完
    let (page2, next2) = chat.list_messages(&conv, Some(&cursor), 2).await.unwrap();
    assert_eq!(page2.len(), 2);
    let cursor2 = next2.expect("仍有更早消息");
    let (page3, next3) = chat.list_messages(&conv, Some(&cursor2), 2).await.unwrap();
    assert_eq!(page3.len(), 1);
    assert!(next3.is_none(), "取完无下一页");
    // 新→旧且不重不漏
    let combined = [page1.as_slice(), page2.as_slice(), page3.as_slice()].concat();
    let all: std::collections::HashSet<&str> =
        combined.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(all.len(), 5, "无重复");
    assert!(
        page1[0].id > page1[1].id && page2[0].id > page2[1].id,
        "页内新→旧(id 时间有序)"
    );
}

/// 3) 发送四状态:默认 Accepted→sent;NotSubmitted→failed;Unknown→uncertain;
///    出站行先落 sending(发送前持久化)。
#[tokio::test]
async fn send_text_maps_send_outcomes_to_status() {
    let h = harness().await;
    let chat = chat_service(&h);
    // 先造会话(入站一条)
    chat.ingest(incoming("pm-3", "在吗")).await.unwrap();

    // Accepted → sent
    let row = chat.send_text(&conv_of(&h).await, "您好").await.unwrap();
    assert_eq!(row.status.as_deref(), Some("sent"));
    assert!(row.platform_message_id.is_some(), "接纳回执回填平台消息 ID");

    // NotSubmitted → failed
    h.fake
        .script_chat_send(SendOutcome::NotSubmitted { retryable: true });
    let row = chat.send_text(&conv_of(&h).await, "第二条").await.unwrap();
    assert_eq!(row.status.as_deref(), Some("failed"));

    // Unknown → uncertain(不可自动重试)
    h.fake
        .script_chat_send(SendOutcome::Unknown { hint: Some("超时".into()) });
    let row = chat.send_text(&conv_of(&h).await, "第三条").await.unwrap();
    assert_eq!(row.status.as_deref(), Some("uncertain"));
    assert_eq!(row.error_hint.as_deref(), Some("超时"));

    // 空/超长文本
    let r = chat.send_text(&conv_of(&h).await, "").await.unwrap_err();
    assert!(matches!(r, ChatError::Invalid(_)));
    let r = chat
        .send_text(&conv_of(&h).await, &"字".repeat(2001))
        .await
        .unwrap_err();
    assert!(matches!(r, ChatError::ContentTooLong), "{r:?}");
}

async fn conv_of(h: &Harness) -> String {
    column(h, "SELECT id FROM conversations WHERE peer_buyer_id='buyer-1'").await
}

/// 4) 重试守卫:failed 可人工重试(新行);uncertain 拒绝 unsafe_retry。
#[tokio::test]
async fn retry_only_failed_and_uncertain_rejected() {
    let h = harness().await;
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-4", "在吗")).await.unwrap();
    let conv = conv_of(&h).await;

    // failed 行
    h.fake
        .script_chat_send(SendOutcome::NotSubmitted { retryable: true });
    let failed = chat.send_text(&conv, "会失败的一条").await.unwrap();
    assert_eq!(failed.status.as_deref(), Some("failed"));
    // 人工重试成功(默认 Accepted):新行 sent,旧行保留审计
    let retried = chat.retry(&failed.id).await.unwrap();
    assert_eq!(retried.status.as_deref(), Some("sent"));
    assert_ne!(retried.id, failed.id, "新 attempt=新行(幂等键承载)");
    assert_eq!(retried.body_text.as_deref(), Some("会失败的一条"));

    // uncertain 行:重试被拒
    h.fake
        .script_chat_send(SendOutcome::Unknown { hint: Some("结果未知".into()) });
    let uncertain = chat.send_text(&conv, "结果未知的一条").await.unwrap();
    assert_eq!(uncertain.status.as_deref(), Some("uncertain"));
    let err = chat.retry(&uncertain.id).await.unwrap_err();
    assert!(matches!(err, ChatError::UnsafeRetry), "{err:?}");
    // sent 行同样不可重试
    let err = chat.retry(&retried.id).await.unwrap_err();
    assert!(matches!(err, ChatError::UnsafeRetry));
}

/// 5) OutgoingMessageEvidence 回填:按 request_key 匹配出站行,
///    uncertain→sent + platform_message_id 回填;无匹配(交付回执)为 no-op。
#[tokio::test]
async fn outgoing_evidence_backfills_uncertain_row() {
    let h = harness().await;
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-5", "在吗")).await.unwrap();
    let conv = conv_of(&h).await;
    h.fake
        .script_chat_send(SendOutcome::Unknown { hint: Some("等待回执".into()) });
    let row = chat.send_text(&conv, "等回执的一条").await.unwrap();
    assert_eq!(row.status.as_deref(), Some("uncertain"));
    // 出站行携带 request_key(发送幂等键)
    let request_key = column(
        &h,
        &format!("SELECT request_key FROM chat_messages WHERE id='{}'", row.id),
    )
    .await;
    assert!(!request_key.is_empty());
    // 回执到达(dispatch_loop 分支同法直调)
    let hit = chat
        .apply_evidence(&request_key, "platform-mid-9")
        .await
        .unwrap();
    assert_eq!(hit, Some((row.id.clone(), true)), "uncertain 补记 sent");
    assert_eq!(
        column(
            &h,
            &format!("SELECT status FROM chat_messages WHERE id='{}'", row.id)
        )
        .await,
        "sent"
    );
    assert_eq!(
        column(
            &h,
            &format!(
                "SELECT platform_message_id FROM chat_messages WHERE id='{}'",
                row.id
            )
        )
        .await,
        "platform-mid-9"
    );
    // 无匹配(交付类回执):None,不报错
    assert_eq!(chat.apply_evidence("no-such-key", "pm-x").await.unwrap(), None);
}

/// 6) 未读:入站聚合 +1、系统/卡片不加红点、读后清零、全局/按账号汇总。
#[tokio::test]
async fn unread_aggregates_and_clears_on_read() {
    let h = harness().await;
    let chat = chat_service(&h);
    for i in 1..=2 {
        chat.ingest(incoming(&format!("pm-u{i}"), "用户消息")).await.unwrap();
    }
    // 系统消息不加红点
    let mut sys = incoming("pm-sys", "交易通知");
    sys.msg_kind = ChatMsgKind::System;
    chat.ingest(sys).await.unwrap();
    let conv = conv_of(&h).await;
    assert_eq!(
        count(&h, "SELECT unread_count FROM conversations WHERE id != '' AND peer_buyer_id='buyer-1'").await,
        2,
        "两条文本 +1×2,系统消息不加"
    );
    // 汇总(全局/按账号)
    let (total, by_account) = chat.unread_summary().await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(by_account, vec![("acct-1".to_string(), 2)]);
    // 打开会话清零 + 入站消息补 read_at
    chat.mark_read(&conv).await.unwrap();
    assert_eq!(count(&h, "SELECT unread_count FROM conversations WHERE peer_buyer_id='buyer-1'").await, 0);
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM chat_messages WHERE direction='in' AND read_at IS NOT NULL").await,
        3
    );
    let (total, _) = chat.unread_summary().await.unwrap();
    assert_eq!(total, 0);
    // 不存在的会话 404 语义
    assert!(matches!(
        chat.mark_read("conv-none").await.unwrap_err(),
        ChatError::ConversationNotFound
    ));
}

/// 7) 删除会话:本机隐藏(hidden_at)、消息物理清空、列表不可见、未读不再计入。
#[tokio::test]
async fn delete_conversation_hides_and_clears_messages() {
    let h = harness().await;
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-d1", "会删掉的消息")).await.unwrap();
    let conv = conv_of(&h).await;
    let (items, _) = chat
        .list_conversations("acct-1", None, false, None, 20)
        .await
        .unwrap();
    assert_eq!(items.len(), 1, "删除前列表可见");
    chat.delete_conversation(&conv).await.unwrap();
    let (items, _) = chat
        .list_conversations("acct-1", None, false, None, 20)
        .await
        .unwrap();
    assert!(items.is_empty(), "删除后本机隐藏不可见");
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM chat_messages WHERE conversation_id != '' AND body_text='会删掉的消息'").await,
        0,
        "展示消息物理清空(data-model:删除会话=清空展示消息)"
    );
    assert_eq!(
        count(&h, &format!("SELECT COUNT(*) FROM conversations WHERE id='{conv}' AND hidden_at IS NOT NULL")).await,
        1,
        "删除后本机隐藏(hidden_at 置位)"
    );
    let (total, _) = chat.unread_summary().await.unwrap();
    assert_eq!(total, 0, "隐藏会话未读不再计入徽标");
    // 消息查询 404 语义(会话隐藏)
    assert!(matches!(
        chat.list_messages(&conv, None, 20).await,
        Err(ChatError::ConversationNotFound)
    ));
}

/// 8) 快捷回复 50 上限:第 51 条 reply_limit_reached;增删查回环。
#[tokio::test]
async fn quick_replies_cap_at_50() {
    let h = harness().await;
    let chat = chat_service(&h);
    for i in 1..=50 {
        chat.add_quick_reply(&format!("快捷回复 {i}")).await.unwrap();
    }
    let err = chat.add_quick_reply("第 51 条").await.unwrap_err();
    assert!(matches!(err, ChatError::ReplyLimitReached), "{err:?}");
    let rows = chat.quick_replies().await.unwrap();
    assert_eq!(rows.len(), 50);
    // 删除一条后可再加
    assert!(chat.delete_quick_reply(&rows[0].id).await.unwrap());
    chat.add_quick_reply("补位").await.unwrap();
    assert_eq!(chat.quick_replies().await.unwrap().len(), 50);
    // 空正文拒绝
    assert!(matches!(
        chat.add_quick_reply("").await.unwrap_err(),
        ChatError::Invalid(_)
    ));
}

/// 9) 图片发送(mock 完整链路):文件落 uploads/chat/(UUID+白名单扩展)、
///    sending→sent、10MB 上限 413 语义、非白名单扩展拒绝、
///    live 能力(false)门禁拒绝;买家备注 upsert 回环。
#[tokio::test]
async fn image_send_full_path_and_capability_gate() {
    let h = harness().await;
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-img", "看图")).await.unwrap();
    let conv = conv_of(&h).await;

    // 成功路径:png 文件入库路径、状态 sent、文件落盘且不入库
    let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let row = chat.send_image(&conv, "截图.PNG", &png).await.unwrap();
    assert_eq!(row.msg_kind, "image");
    assert_eq!(row.status.as_deref(), Some("sent"));
    let rel = row.image_path.clone().unwrap();
    assert!(rel.starts_with("uploads/chat/"), "相对数据目录:{rel}");
    assert!(rel.ends_with(".png"), "扩展归一小写:{rel}");
    assert!(h.uploads_root.join("chat").join(rel.trim_start_matches("uploads/chat/")).exists());
    assert_eq!(
        count(&h, &format!("SELECT COUNT(*) FROM chat_messages WHERE image_path='{rel}'")).await,
        1
    );

    // 白名单外扩展
    assert!(matches!(
        chat.send_image(&conv, "x.bmp", b"BM").await.unwrap_err(),
        ChatError::UnsupportedImageType
    ));
    // 10MB 上限(应用层字节检查 → 413 payload_too_large 语义)
    let oversized = vec![0u8; 10 * 1024 * 1024 + 1];
    assert!(matches!(
        chat.send_image(&conv, "big.png", &oversized).await.unwrap_err(),
        ChatError::PayloadTooLarge
    ));
    // 能力门禁:live 档 chat_send_image=false → 拒绝(不落盘不落库)
    let live = live_caps_service(&h);
    let before = count(&h, "SELECT COUNT(*) FROM chat_messages").await;
    assert!(matches!(
        live.send_image(&conv, "y.png", b"png").await.unwrap_err(),
        ChatError::ImageUnsupported
    ));
    assert_eq!(count(&h, "SELECT COUNT(*) FROM chat_messages").await, before);

    // 买家备注:保存/读取/清除(空串)
    let note_svc = chat_service(&h);
    assert_eq!(note_svc.buyer_note("acct-1", "buyer-1").await.unwrap(), None);
    note_svc.save_buyer_note("acct-1", "buyer-1", "老客户,优先处理").await.unwrap();
    assert_eq!(
        note_svc.buyer_note("acct-1", "buyer-1").await.unwrap(),
        Some("老客户,优先处理".to_string())
    );
    note_svc.save_buyer_note("acct-1", "buyer-1", "").await.unwrap();
    assert_eq!(note_svc.buyer_note("acct-1", "buyer-1").await.unwrap(), Some(String::new()));
}

/// 10) 离线账号发送 → AccountUnavailable;会话不存在 404。
#[tokio::test]
async fn offline_account_rejects_send() {
    let h = harness().await;
    let chat = chat_service(&h);
    chat.ingest(incoming("pm-off", "在吗")).await.unwrap();
    let conv = conv_of(&h).await;
    h.db
        .call(|conn| {
            conn.execute(
                "UPDATE accounts SET status='offline' WHERE id='acct-1'",
                [],
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        chat.send_text(&conv, "离线发不出").await.unwrap_err(),
        ChatError::AccountUnavailable
    ));
    assert!(matches!(
        chat.send_text("conv-none", "不存在").await.unwrap_err(),
        ChatError::ConversationNotFound
    ));
    let _ = utc_now_ms();
}
