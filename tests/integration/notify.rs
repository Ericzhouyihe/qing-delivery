//! US5 通知渠道集成测试(T057 转绿,007 D8 语义):
//! 1) 绑定覆盖:绑定账号仅发绑定渠道;未绑定/无账号事件走全部启用渠道(FR-052);
//! 2) 订阅过滤:event_types 空=全部;订阅外事件不发送(FR-051);
//! 3) unknown 投递 → notification_deliveries 留痕 + issue kind=notify_unknown
//!    (无订单去重索引 idx_issues_open_dedup_account 生效:同账号同 kind 同 reason
//!    只一条 open;FR-054 结果未知不盲目重发);
//! 4) secrets 脱敏:回读只含 *_configured;编辑留空不覆盖;替换则更新(FR-050);
//! 5) 渠道删除:绑定经 FK CASCADE 联清;投递留痕保留审计(channel_id 可残留);
//! 6) 系统设置:system_settings 明文键 + system_secrets 信封键(值仅内部用)。
//! 发送方为记录型假实现(不发网);网络绝不进 DbThread 事务由实现保证。

use std::sync::{Arc, Mutex};

use qing_delivery::adapters::notify::{NotifySender, SendState};
use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::sqlite::repos::accounts;
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys::{self, DataKey};
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::notify::{ChannelDraft, NotifyEvent, NotifyService};
use qing_delivery::domain::notify::ChannelKind;

/// 记录型假发送方:记录每次调用(渠道 id + 秘密明文快照),按渠道返回可配置状态。
struct FakeSender {
    calls: Mutex<Vec<(String, Vec<(String, String)>)>>, // (channel_id, secrets 快照)
    /// 每渠道返回状态:0=Accepted、1=NotSent、2=Unknown;默认 0
    states: Mutex<std::collections::HashMap<String, u8>>,
}

impl FakeSender {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            states: Mutex::new(std::collections::HashMap::new()),
        })
    }

    fn set_state(&self, channel_id: &str, state: u8) {
        self.states
            .lock()
            .unwrap()
            .insert(channel_id.to_string(), state);
    }

    fn calls_for(&self, channel_id: &str) -> Vec<Vec<(String, String)>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == channel_id)
            .map(|(_, secrets)| secrets.clone())
            .collect()
    }

    fn total_calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn last_secret(&self, channel_id: &str, key: &str) -> Option<String> {
        self.calls_for(channel_id)
            .last()
            .and_then(|secrets| secrets.iter().find(|(k, _)| k == key).cloned())
            .map(|(_, v)| v)
    }
}

#[async_trait::async_trait]
impl NotifySender for FakeSender {
    async fn send(
        &self,
        channel: &qing_delivery::domain::notify::ChannelConfig,
        _event: &qing_delivery::domain::notify::NotifyEventData,
    ) -> Result<SendState, String> {
        self.calls
            .lock()
            .unwrap()
            .push((channel.id.clone(), channel.secrets.clone().into_iter().collect()));
        let state = self.states.lock().unwrap().get(&channel.id).copied().unwrap_or(0);
        Ok(match state {
            1 => SendState::NotSent("4xx 测试失败".into()),
            2 => SendState::Unknown("超时测试".into()),
            _ => SendState::Accepted,
        })
    }
}

struct Harness {
    db: DbThread,
    key: DataKey,
    fake: Arc<FakeSender>,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = DataDir::resolve(Some(dir.path())).unwrap();
    let lock = DirLock::acquire(&data_dir.root).unwrap();
    let key = keys::load_or_create(&data_dir.root).unwrap();
    let db = DbThread::spawn(&data_dir.join("us5-notify.db")).unwrap();
    let dir_path = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &dir_path))
        .await
        .unwrap()
        .unwrap();
    db.call(|conn| {
        accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "测试店铺")
    })
    .await
    .unwrap()
    .unwrap();
    let data_key = DataKey {
        key_id: key.key_id.clone(),
        key: key.key,
    };
    let fake = FakeSender::new();
    let _ = NotifyService::new(db.clone(), data_key)
        .with_sender(ChannelKind::Webhook, fake.clone() as Arc<dyn NotifySender>);
    Harness {
        db,
        key: DataKey {
            key_id: key.key_id.clone(),
            key: key.key,
        },
        fake,
        _dir: dir,
        _lock: lock,
    }
}

fn service(h: &Harness, extra: Option<(ChannelKind, Arc<dyn NotifySender>)>) -> NotifyService {
    let mut svc = NotifyService::new(h.db.clone(), h.key.clone());
    svc = svc.with_sender(ChannelKind::Webhook, h.fake.clone() as Arc<dyn NotifySender>);
    if let Some((kind, sender)) = extra {
        svc = svc.with_sender(kind, sender);
    }
    svc
}

fn webhook_draft(name: &str) -> ChannelDraft {
    ChannelDraft {
        kind: "webhook".into(),
        name: name.into(),
        enabled: true,
        config: serde_json::json!({ "url": "https://example.com/hook" }),
        secrets: Default::default(),
        event_types: vec![],
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

fn offline_event() -> NotifyEvent {
    NotifyEvent::AccountOffline {
        account_id: "acct-1".into(),
        account_name: Some("测试店铺".into()),
    }
}

/// 1) 绑定覆盖(FR-052):绑定账号仅发绑定渠道;未绑定账号走全部启用渠道。
#[tokio::test]
async fn 绑定覆盖_绑定账号仅发绑定渠道_未绑定走全部() {
    let h = harness().await;
    let svc = service(&h, None);
    let a = svc.create_channel(webhook_draft("全部A")).await.unwrap();
    let b = svc.create_channel(webhook_draft("全部B")).await.unwrap();
    assert_ne!(a.id, b.id, "两个独立渠道");

    // 未绑定:事件带账号 → 全部启用渠道
    svc.fanout(&offline_event()).await;
    assert_eq!(h.fake.total_calls(), 2, "未绑定账号走全部启用渠道");
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM notification_deliveries").await,
        2
    );

    // 覆盖式绑定到 B:仅向 B 发送
    svc.put_bindings("acct-1", &[b.id.clone()]).await.unwrap();
    h.fake.calls.lock().unwrap().clear();
    svc.fanout(&offline_event()).await;
    assert_eq!(h.fake.total_calls(), 1, "绑定后仅发绑定渠道");
    let sent_channel: String = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                conn.query_row(
                    "SELECT channel_id FROM notification_deliveries ORDER BY created_at DESC, id DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent_channel, b.id, "发送对象必须是绑定渠道 B");

    // 事件无账号维度(系统级)→ 全部启用渠道
    h.fake.calls.lock().unwrap().clear();
    svc.fanout(&NotifyEvent::SystemError {
        account_id: None,
        reason: "备份失败".into(),
    })
    .await;
    assert_eq!(h.fake.total_calls(), 2, "无账号事件按全部启用渠道处理");
}

/// 2) 订阅过滤(FR-051):订阅外事件不发送;空订阅=全部。
#[tokio::test]
async fn 订阅过滤_订阅外事件不发送() {
    let h = harness().await;
    let svc = service(&h, None);
    let mut only_offline = webhook_draft("只订掉线");
    only_offline.event_types = vec!["account_offline".into()];
    let sub = svc.create_channel(only_offline).await.unwrap();
    let all = svc.create_channel(webhook_draft("全部")).await.unwrap();

    // 掉线事件:两个渠道都发
    svc.fanout(&offline_event()).await;
    assert_eq!(h.fake.calls_for(&sub.id).len(), 1);
    assert_eq!(h.fake.calls_for(&all.id).len(), 1);

    // 交付结果事件:仅订阅外不发送;全部渠道照发
    svc.fanout(&NotifyEvent::DeliveryResult {
        account_id: "acct-1".into(),
        order_ref: "EXT-9".into(),
        outcome: "not_sent".into(),
        summary: "订单 EXT-9 交付未发出".into(),
    })
    .await;
    assert_eq!(
        h.fake.calls_for(&sub.id).len(),
        1,
        "订阅外的 delivery_result 不发送"
    );
    assert_eq!(h.fake.calls_for(&all.id).len(), 2, "空订阅=全部,第二条也发");

    // not_sent 状态留痕(渠道级失败仍记录)
    let states: String = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                conn.query_row(
                    "SELECT group_concat(state) FROM (SELECT state FROM notification_deliveries ORDER BY id)",
                    [],
                    |r| r.get(0),
                )
                .unwrap(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert!(states.split(',').all(|s| s == "accepted"), "假发送方默认 Accepted");
}

/// 3) unknown 投递 → 留痕 + notify_unknown 待处理(去重索引生效;FR-054)。
#[tokio::test]
async fn unknown投递_转待处理且同账号同因由只开一条() {
    let h = harness().await;
    let svc = service(&h, None);
    let ch = svc.create_channel(webhook_draft("会超时")).await.unwrap();
    h.fake.set_state(&ch.id, 2); // Unknown

    let ch_id = ch.id.clone();
    svc.fanout(&offline_event()).await;
    // 留痕:state=unknown + error_hint
    let (state, hint): (String, Option<String>) = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                conn.query_row(
                    "SELECT state, error_hint FROM notification_deliveries WHERE channel_id = ?1",
                    rusqlite::params![ch_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, "unknown");
    assert!(hint.unwrap_or_default().contains("超时"), "error_hint 带原因");

    // issue:kind=notify_unknown、allowed_actions=["terminate"]
    let (kind, allowed, cnt): (String, String, i64) = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(
                conn.query_row(
                    "SELECT kind, allowed_actions, COUNT(*) FROM issues
                     WHERE kind='notify_unknown' AND state='open'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind, "notify_unknown");
    assert_eq!(allowed, "[\"terminate\"]");
    assert_eq!(cnt, 1, "首次投递 unknown 开一条待处理");

    // 同账号同 kind 同 reason 再次 unknown:去重索引生效,仍只有一条 open
    svc.fanout(&offline_event()).await;
    svc.fanout(&offline_event()).await;
    let open_count = count(
        &h,
        "SELECT COUNT(*) FROM issues WHERE kind='notify_unknown' AND state='open'",
    )
    .await;
    assert_eq!(open_count, 1, "无订单去重:同账号同 kind 同 reason 只一条 open");
    // 投递留痕逐次记录(3 次尝试;FR-054 每次尝试都留痕,不盲目重发)
    let ch_id = ch.id.clone();
    let deliveries = h
        .db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(
                "SELECT COUNT(*) FROM notification_deliveries WHERE channel_id = ?1",
                rusqlite::params![ch_id],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deliveries, 3, "每次尝试都留痕(不盲目重发,但记录每次结果)");
}

/// 4) secrets 脱敏(FR-050):回读只含 *_configured;留空不覆盖;替换则更新。
#[tokio::test]
async fn secrets脱敏_留空不覆盖_替换则更新() {
    let h = harness().await;
    let tg_fake = FakeSender::new();
    let svc = service(&h, Some((ChannelKind::Telegram, tg_fake.clone() as Arc<dyn NotifySender>)));

    let mut draft = ChannelDraft {
        kind: "telegram".into(),
        name: "机器人".into(),
        enabled: true,
        config: serde_json::json!({ "chat_id": "-100123" }),
        secrets: [("bot_token".to_string(), "TOK-1".to_string())].into(),
        event_types: vec![],
    };
    let ch = svc.create_channel(draft.clone()).await.unwrap();

    // 回读:只有 secrets_configured 键名列表,无明文
    assert_eq!(ch.secrets_configured, vec!["bot_token".to_string()]);
    let serialized = serde_json::to_string(&ch).unwrap();
    assert!(!serialized.contains("TOK-1"), "API 回读绝不包含秘密明文");

    // 库内:secrets 为信封,config 明文里无秘密
    let stored: (Option<Vec<u8>>, String) = h
        .db
        .call({
            let id = ch.id.clone();
            move |conn| {
                Ok::<_, rusqlite::Error>(
                    conn.query_row(
                        "SELECT secrets_ciphertext, config FROM notification_channels WHERE id=?1",
                        rusqlite::params![id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap(),
                )
            }
        })
        .await
        .unwrap()
        .unwrap();
    assert!(stored.0.is_some(), "秘密整包信封落库");
    assert!(!stored.1.contains("TOK-1"), "config 明文不含秘密");

    // 发送侧可见明文(内存解密)
    svc.fanout(&offline_event()).await;
    assert_eq!(
        tg_fake.last_secret(&ch.id, "bot_token").as_deref(),
        Some("TOK-1")
    );

    // 编辑留空(None)= 不覆盖
    draft.secrets.clear();
    let _ = svc
        .update_channel(&ch.id, ch.version, draft.clone())
        .await
        .unwrap();
    svc.fanout(&offline_event()).await;
    assert_eq!(
        tg_fake.last_secret(&ch.id, "bot_token").as_deref(),
        Some("TOK-1"),
        "留空保存不覆盖既有秘密"
    );

    // 替换则更新
    draft.secrets.insert("bot_token".into(), "TOK-2".into());
    let updated = svc
        .update_channel(&ch.id, updated_version(&svc, &ch.id).await, draft)
        .await
        .unwrap();
    assert_eq!(updated.secrets_configured, vec!["bot_token".to_string()]);
    svc.fanout(&offline_event()).await;
    assert_eq!(
        tg_fake.last_secret(&ch.id, "bot_token").as_deref(),
        Some("TOK-2"),
        "替换后发送侧看到新值"
    );

    // 未知秘密键拒绝(创建侧白名单)
    let mut bad = webhook_draft("坏键");
    bad.secrets.insert("token".into(), "x".into());
    assert!(svc.create_channel(bad).await.is_err());
}

async fn updated_version(svc: &NotifyService, id: &str) -> i64 {
    svc.get_channel(id).await.unwrap().version
}

/// 5) 渠道删除:绑定 FK CASCADE 联清;投递留痕保留审计。
#[tokio::test]
async fn 渠道删除_绑定联清_投递留痕保留() {
    let h = harness().await;
    let svc = service(&h, None);
    let ch = svc.create_channel(webhook_draft("待删")).await.unwrap();
    svc.put_bindings("acct-1", &[ch.id.clone()])
        .await
        .unwrap();
    svc.fanout(&offline_event()).await;

    assert!(svc.delete_channel(&ch.id).await.unwrap(), "删除成功");
    assert!(svc.get_channel(&ch.id).await.is_err(), "渠道不再可读");
    assert!(
        svc.get_bindings("acct-1").await.unwrap().is_empty(),
        "绑定经 CASCADE 联清"
    );
    // 投递留痕保留(channel_id 残留无害:无 FK,审计保留)
    let kept = count(
        &h,
        "SELECT COUNT(*) FROM notification_deliveries WHERE channel_id IS NOT NULL",
    )
    .await;
    assert_eq!(kept, 1, "删除渠道后投递审计保留");
    // 再删返回 false(幂等语义:404 由传输层判断)
    assert!(!svc.delete_channel(&ch.id).await.unwrap());
}

/// 6) 系统设置:明文键 + 信封秘密键(读取只回 configured;值仅内部用)。
#[tokio::test]
async fn 系统设置_明文与信封键_值仅内部读取() {
    let h = harness().await;
    let svc = service(&h, None);
    svc.put_system_smtp(&qing_delivery::application::notify::SmtpConfig {
        host: "smtp.example.com".into(),
        port: 465,
        encryption: "ssl".into(),
        from_address: "shop@example.com".into(),
        from_name: Some("轻交付".into()),
        username: Some("shop@example.com".into()),
        password: Some("SECRET-PW".into()),
    })
    .await
    .unwrap();

    let got = svc.get_system_smtp().await.unwrap();
    assert_eq!(got.host, "smtp.example.com");
    assert_eq!(got.port, 465);
    assert_eq!(got.encryption, "ssl");
    assert!(got.password_configured, "密码已配置(布尔)");
    let serialized = serde_json::to_string(&got).unwrap();
    assert!(!serialized.contains("SECRET-PW"), "系统秘密永不回显明文");

    // 内部读取(发送侧使用)
    assert_eq!(
        svc.read_system_secret("smtp_password").await.unwrap().as_deref(),
        Some("SECRET-PW")
    );

    // 库内为信封四件套
    let sealed: i64 = count(
        &h,
        "SELECT COUNT(*) FROM system_secrets WHERE key='smtp_password' AND ciphertext IS NOT NULL AND nonce IS NOT NULL",
    )
    .await;
    assert_eq!(sealed, 1);
    let plain: i64 = count(
        &h,
        "SELECT COUNT(*) FROM system_settings WHERE key='smtp_host' AND value='smtp.example.com'",
    )
    .await;
    assert_eq!(plain, 1, "非机密设置明文存储");

    // 更新留空密码 = 不修改
    svc.put_system_smtp(&qing_delivery::application::notify::SmtpConfig {
        host: "smtp2.example.com".into(),
        port: 587,
        encryption: "starttls".into(),
        from_address: "shop2@example.com".into(),
        from_name: None,
        username: None,
        password: None,
    })
    .await
    .unwrap();
    let got2 = svc.get_system_smtp().await.unwrap();
    assert_eq!(got2.host, "smtp2.example.com");
    assert!(got2.password_configured, "留空保存不覆盖系统密码");
    assert_eq!(
        svc.read_system_secret("smtp_password").await.unwrap().as_deref(),
        Some("SECRET-PW")
    );
}

/// 附:测试投递(即时发送一次,不经订阅过滤;contracts §5 test 端点语义)。
#[tokio::test]
async fn 测试投递_即时发送一次并返回结构化结果() {
    let h = harness().await;
    let svc = service(&h, None);
    let mut only_offline = webhook_draft("只订掉线");
    only_offline.event_types = vec!["account_offline".into()];
    let ch = svc.create_channel(only_offline).await.unwrap();

    // 测试投递不受订阅过滤限制(管理员显式测试该渠道)
    let state = svc.test_send(&ch.id).await.unwrap();
    assert!(matches!(state, SendState::Accepted));
    assert_eq!(h.fake.calls_for(&ch.id).len(), 1);

    h.fake.set_state(&ch.id, 1); // 4xx
    let state2 = svc.test_send(&ch.id).await.unwrap();
    assert!(matches!(state2, SendState::NotSent(_)));
    // 测试投递也留痕(state=not_sent)
    let rows = count(
        &h,
        "SELECT COUNT(*) FROM notification_deliveries WHERE event_kind='test'",
    )
    .await;
    assert_eq!(rows, 2, "每次测试投递留痕");
}
