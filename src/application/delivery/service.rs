//! 交付编排(T031—T035):T1 唯一任务 → T2 冻结快照 → T3 守卫+attempt 先落库
//! → handoff → 结果分类 → 证明 → 独立平台确认 → 重试预算。
//! 网络绝不进入数据库事务;unknown 永不自动重发(宪章 I)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{accounts, cards, deliveries, issues, items, orders, rules};
use crate::adapters::windows::keys::DataKey;
use crate::application::delivery::eligibility::{self, Eligibility, EligibilityInput};
use crate::application::ports::platform::{
    ConfirmOutcome, ContentForSend, PlatformAdapter, PlatformError, RequestContext, SendOutcome,
};
use crate::domain::crypto::{self, Aad, CryptoError};
use crate::domain::delivery::state::{
    ConfirmationState, ContentState, ReviewState, confirmation_transition, content_transition,
};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;
use sha2::{Digest, Sha256};

/// 自动重试预算:初次发送外最多 3 次,间隔 30/60/120 秒(FR-019,spec 默认值)。
pub const MAX_AUTO_RETRIES: i64 = 3;
pub const RETRY_DELAYS_MS: [i64; 3] = [30_000, 60_000, 120_000];

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("平台返回了非法结果变体")]
    InvalidOutcome,
}

#[derive(Debug, PartialEq)]
pub enum HandleOutcome {
    /// 资格不成立:已建任务(pending_verification)并开待处理事项,未发送
    Ineligible { delivery_id: String, reason: String },
    /// 内容已发送且收到严格关联的平台接纳证明
    Delivered { delivery_id: String },
    /// 确定未发送;retry_scheduled 表示预算内已排定自动重试
    NotSent {
        delivery_id: String,
        retry_scheduled: bool,
        reason: Option<String>,
    },
    /// 结果未知:保留原内容与尝试,禁止自动重发/确认(FR-020)
    Unknown { delivery_id: String },
    /// 已有终态/未知 initial 或执行互斥:不重复处理(FR-014)
    AlreadyHandled,
    /// order guard 被其他执行持有(自动/人工互斥)
    Busy,
}

pub struct DeliveryService<A: PlatformAdapter> {
    db: DbThread,
    key: DataKey,
    adapter: A,
}

struct DeliveryFacts {
    order_id: String,
    account: accounts::AccountRow,
    item: Option<items::ItemRow>,
    rule: Option<rules::RuleRow>,
    rule_ambiguous: bool,
    restore_quarantined: bool,
    paid_at: Option<i64>,
    buyer_id: Option<String>,
    /// 可信购买件数(卡密预留 units = 份数×件数;缺失按 1)
    quantity: Option<i64>,
    #[allow(dead_code)]
    sku_key: Option<String>,
}

fn snapshot_sku_key(snapshot: &crate::domain::orders::snapshot::OrderSnapshot) -> Option<String> {
    use crate::domain::sku;
    match (&snapshot.sku_parts, snapshot.sku_single) {
        (crate::domain::orders::snapshot::FieldStatus::Verified(parts), _) => {
            sku::combo_key(parts).ok()
        }
        (crate::domain::orders::snapshot::FieldStatus::Missing, true) => {
            Some(sku::SKU_KEY_SINGLE.to_string())
        }
        _ => None,
    }
}

impl<A: PlatformAdapter> DeliveryService<A> {
    pub fn new(db: DbThread, key: DataKey, adapter: A) -> Self {
        Self { db, key, adapter }
    }

    /// 处理一笔付款候选:核验→建任务→冻结→发送→分类→确认。
    pub async fn handle_payment(
        &self,
        account_id: &str,
        external_order_id: &str,
    ) -> Result<HandleOutcome, DeliveryError> {
        self.handle(account_id, external_order_id, DeliveryTrigger::Auto)
            .await
    }

    /// 历史接管(FR-017):显式路径,允许 paid_at 早于监控起点,其余核验不放宽。
    pub async fn handle_takeover(
        &self,
        account_id: &str,
        external_order_id: &str,
    ) -> Result<HandleOutcome, DeliveryError> {
        self.handle(
            account_id,
            external_order_id,
            DeliveryTrigger::ManualTakeover,
        )
        .await
    }

    async fn handle(
        &self,
        account_id: &str,
        external_order_id: &str,
        trigger: DeliveryTrigger,
    ) -> Result<HandleOutcome, DeliveryError> {
        // ---- 快照(协议优先)----
        let account_row = self
            .db
            .call({
                let account_id = account_id.to_string();
                move |conn| accounts::get(conn, &account_id)
            })
            .await??
            .ok_or_else(|| PlatformError::IncompleteFacts("账号不存在".into()))?;
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: account_row.id.clone(),
            credential_generation: account_row.credential_epoch,
            control_generation: account_row.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        let snapshot = self
            .adapter
            .fetch_order_snapshot(&ctx, external_order_id)
            .await?;

        // ---- 事实装载与资格核验(单次窄读)----
        let external_item = snapshot.item_id.verified().cloned();
        let order_sku_key = snapshot_sku_key(&snapshot);
        let snapshot_paid_at = snapshot.paid_at_ms.verified().copied();
        let snapshot_buyer = snapshot.buyer_id.verified().cloned();
        let snapshot_quantity = snapshot.quantity.verified().copied().map(|q| q as i64);
        let account_id_owned = account_id.to_string();
        let external_order = external_order_id.to_string();
        let facts = self
            .db
            .call(move |conn| -> rusqlite::Result<DeliveryFacts> {
                let account = accounts::get(conn, &account_id_owned)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                let order = orders::find_or_create(
                    conn,
                    &ids::new_id("ord"),
                    "xianyu",
                    &account.id,
                    &external_order,
                )?;
                let item = match &external_item {
                    Some(ext) => items::find_by_external(conn, &account.id, ext)?,
                    None => None,
                };
                let sku_key = order_sku_key
                    .clone()
                    .unwrap_or_else(|| "incomplete".to_string());
                let (rule, ambiguous) = match &item {
                    Some(item) => {
                        let count =
                            rules::count_enabled_exact(conn, &account.id, &item.id, &sku_key)?;
                        (
                            rules::find_enabled_exact(conn, &account.id, &item.id, &sku_key)?,
                            count > 1,
                        )
                    }
                    None => (None, false),
                };
                let restore_quarantined: i64 = conn.query_row(
                    "SELECT restore_epoch FROM installation WHERE id = 'singleton'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(DeliveryFacts {
                    order_id: order.0.id,
                    account,
                    item,
                    rule,
                    rule_ambiguous: ambiguous,
                    restore_quarantined: restore_quarantined > 0,
                    paid_at: snapshot_paid_at,
                    buyer_id: snapshot_buyer,
                    quantity: snapshot_quantity,
                    sku_key: Some(sku_key),
                })
            })
            .await??;

        // ---- 资格核验 ----
        let input = EligibilityInput {
            account_status: &facts.account.status,
            runtime_enabled: facts.account.runtime_enabled,
            auto_delivery_enabled: facts.account.auto_delivery_enabled,
            restore_quarantined: facts.restore_quarantined,
            monitor_since_ms: facts.account.monitor_since,
            snapshot: &snapshot,
            item_listing_state: facts
                .item
                .as_ref()
                .map(|i| i.listing_state.as_str())
                .unwrap_or("unknown"),
            enabled_rule: facts.rule.as_ref().map(|r| r.id.as_str()),
            rule_ambiguous: facts.rule_ambiguous,
            allow_historical: matches!(trigger, DeliveryTrigger::ManualTakeover),
        };
        let verdict = eligibility::verify(&input);

        // ---- T1:唯一 initial(所有路径都先有任务才能谈状态)----
        let delivery = self
            .db
            .call({
                let order_id = facts.order_id.clone();
                move |conn| deliveries::create_initial(conn, &ids::new_id("dlv"), &order_id)
            })
            .await??;
        let Some(delivery) = delivery else {
            // 已有 initial:按既有状态处置
            let existing = self
                .db
                .call({
                    let order_id = facts.order_id.clone();
                    move |conn| deliveries::find_initial(conn, &order_id)
                })
                .await??;
            let Some(existing) = existing else {
                return Ok(HandleOutcome::AlreadyHandled);
            };
            return match existing.content_state.as_str() {
                "accepted" | "unknown" | "terminated" => Ok(HandleOutcome::AlreadyHandled),
                _ => {
                    // pending_verification/queued/not_sent:资格仍需通过
                    match verdict {
                        Eligibility::Eligible(rule_id) => {
                            // 首次通过资格但尚未冻结快照的任务先补 T2
                            let frozen =
                                self.freeze_snapshot(&facts, &existing.id, &rule_id).await?;
                            if !frozen {
                                return Ok(HandleOutcome::AlreadyHandled);
                            }
                            self.merge_facts(&facts, &snapshot).await?;
                            self.dispatch(&facts, &snapshot, existing.id).await
                        }
                        Eligibility::NotEligible(reason) => {
                            self.open_ineligible_issue(&facts, &existing.id, &reason.to_string())
                                .await?;
                            Ok(HandleOutcome::Ineligible {
                                delivery_id: existing.id,
                                reason: reason.to_string(),
                            })
                        }
                    }
                }
            };
        };

        match verdict {
            Eligibility::NotEligible(reason) => {
                self.open_ineligible_issue(&facts, &delivery.id, &reason.to_string())
                    .await?;
                Ok(HandleOutcome::Ineligible {
                    delivery_id: delivery.id,
                    reason: reason.to_string(),
                })
            }
            Eligibility::Eligible(rule_id) => {
                // 订单事实回填(快照可信字段)
                self.merge_facts(&facts, &snapshot).await?;
                // ---- T2:冻结内容快照并绑定规则版本 ----
                let frozen = self.freeze_snapshot(&facts, &delivery.id, &rule_id).await?;
                if !frozen {
                    return Ok(HandleOutcome::AlreadyHandled);
                }
                self.dispatch(&facts, &snapshot, delivery.id).await
            }
        }
    }

    async fn open_ineligible_issue(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        reason: &str,
    ) -> Result<(), DeliveryError> {
        let order_id = facts.order_id.clone();
        let account_id = facts.account.id.clone();
        let delivery = delivery_id.to_string();
        let reason_owned = reason.to_string();
        self.db
            .call(move |conn| {
                issues::open(
                    conn,
                    &ids::new_id("iss"),
                    Some(&order_id),
                    &account_id,
                    Some(&delivery),
                    "delivery_ineligible",
                    &reason_owned,
                    "[\"resend\",\"terminate\"]",
                )
            })
            .await??;
        Ok(())
    }

    async fn merge_facts(
        &self,
        facts: &DeliveryFacts,
        snapshot: &crate::domain::orders::snapshot::OrderSnapshot,
    ) -> Result<(), DeliveryError> {
        use crate::domain::orders::snapshot::FieldStatus;
        // 先取 owned 值,闭包内构造借用视图
        let buyer_id = facts.buyer_id.clone();
        let paid_at = facts.paid_at;
        let quantity = snapshot.quantity.verified().copied().map(|q| q as i64);
        let (amount_minor, currency) = match &snapshot.amount {
            FieldStatus::Verified(money) => (Some(money.minor_units), Some(money.currency.clone())),
            _ => (None, None),
        };
        let sku_pairs = match &snapshot.sku_parts {
            FieldStatus::Verified(parts) => Some(serde_json::to_string(parts).unwrap_or_default()),
            _ => None,
        };
        let sku_complete = matches!(snapshot.sku_parts, FieldStatus::Verified(_));
        let trade_type = match snapshot.trade_type.verified() {
            Some(crate::domain::orders::snapshot::TradeType::Ordinary) => {
                Some("ordinary".to_string())
            }
            _ => None,
        };
        let order_id = facts.order_id.clone();
        self.db
            .call(move |conn| {
                let m = orders::OrderFactMerge {
                    buyer_id: buyer_id.as_deref(),
                    paid_at,
                    quantity,
                    amount_minor,
                    currency: currency.as_deref(),
                    sku_pairs: sku_pairs.as_deref(),
                    sku_complete: Some(sku_complete),
                    trade_type: trade_type.as_deref(),
                };
                orders::merge_order_facts(conn, &order_id, &m)
            })
            .await??;
        Ok(())
    }

    /// T2:解析内容来源(research D2 ContentPlan)→ 冻结快照绑定任务进入 queued。
    /// fixed_text:解密规则当前版本正文(既有行为,零改动);
    /// card_pool:单事务原子预留拼接送文本,research D3 预留协议。
    async fn freeze_snapshot(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        rule_id: &str,
    ) -> Result<bool, DeliveryError> {
        // 幂等:已有快照绑定即视为冻结完成(不重复绑定、不换内容版本)
        let delivery_id_probe = delivery_id.to_string();
        let existing = self
            .db
            .call({
                let delivery = delivery_id_probe;
                move |conn| deliveries::get(conn, &delivery)
            })
            .await??;
        if let Some(row) = existing
            && row.content_snapshot_id.is_some()
        {
            return Ok(true);
        }
        let rule_id = rule_id.to_string();
        let loaded = self
            .db
            .call({
                let rule_id = rule_id.clone();
                move |conn| -> rusqlite::Result<LoadedSource> {
                    let rule = rules::get(conn, &rule_id)?;
                    let Some(rule) = rule else { return Ok(LoadedSource::Missing) };
                    let pool_binding = rule.card_pool_id.clone();
                    match pool_binding.as_deref() {
                        Some(pool_id) if !pool_id.is_empty() => Ok(LoadedSource::CardPool {
                            rule,
                            pool_id: pool_id.to_string(),
                        }),
                        _ => {
                            let content =
                                rules::get_content(conn, &rule_id, rule.current_content_version)?;
                            match content {
                                Some(content) => Ok(LoadedSource::FixedText { content, rule }),
                                None => Ok(LoadedSource::Missing),
                            }
                        }
                    }
                }
            })
            .await??;
        match loaded {
            LoadedSource::Missing => Ok(false),
            LoadedSource::FixedText { content, rule } => {
                // ---- fixed_text 分支(行为与 001 完全一致)----
                let plaintext = crypto::open(
                    &self.key.key,
                    &rules::rule_aad(&rule.id, content.content_version),
                    &content.envelope,
                )?;
                let source = format!("rule:{rule_id}:v{}", content.content_version);
                self.freeze_plaintext(facts, delivery_id, &rule_id, &plaintext, &content.text_digest, source)
                    .await
            }
            LoadedSource::CardPool { rule, pool_id } => {
                // ---- card_pool 分支:同一 DbThread 事务闭包内原子预留(D3)----
                let _ = rule; // 规则行仅用于来源判定;绑定仍用 rule_id
                let plan = self.plan_card_pool(facts, delivery_id, &pool_id).await?;
                match plan {
                    CardPlan::Insufficient => {
                        // 余量不足/停用:整体回滚已完成(事务闭包内逐条预留,任一不足即回滚),
                        // 开 stock_insufficient 事项后返回 false(不部分交付)
                        let order_id = facts.order_id.clone();
                        let account_id = facts.account.id.clone();
                        let delivery = delivery_id.to_string();
                        self.db
                            .call(move |conn| {
                                issues::open(
                                    conn,
                                    &ids::new_id("iss"),
                                    Some(&order_id),
                                    &account_id,
                                    Some(&delivery),
                                    "stock_insufficient",
                                    "out_of_stock",
                                    "[\"terminate\"]",
                                )
                            })
                            .await??;
                        Ok(false)
                    }
                    CardPlan::Entries { pool_id, cards } => {
                        let text = cards
                            .iter()
                            .map(|c| c.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let source = format!(
                            "card:{pool_id}:{}",
                            cards
                                .iter()
                                .map(|c| c.id.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        );
                        let digest = hex::encode(Sha256::digest(text.as_bytes()));
                        self.freeze_plaintext(
                            facts,
                            delivery_id,
                            &rule_id,
                            text.as_bytes(),
                            &digest,
                            source,
                        )
                        .await
                    }
                    CardPlan::FixedContent { pool_id, plaintext } => {
                        // text/image 池:固定内容直接作为冻结正文(无条目预留)
                        let source = format!("card:{pool_id}");
                        let digest = hex::encode(Sha256::digest(&plaintext));
                        self.freeze_plaintext(
                            facts,
                            delivery_id,
                            &rule_id,
                            &plaintext,
                            &digest,
                            source,
                        )
                        .await
                    }
                }
            }
        }
    }

    /// 卡密分支单事务计划(D3):data 池循环预留 N=件数(份数首版=1);
    /// 任一次取不到 → 事务回滚(不部分交付)→ Insufficient;
    /// text/image 池用固定内容;api 池暂无本地库存按不足处理
    /// (CardSupplier 事务外取卡接线见 T011 端口;不在此处外呼网络)。
    async fn plan_card_pool(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        pool_id: &str,
    ) -> Result<CardPlan, DeliveryError> {
        let order_id = facts.order_id.clone();
        let delivery = delivery_id.to_string();
        let pool_id = pool_id.to_string();
        let units = facts.quantity.unwrap_or(1).max(1);
        let key = self.key.key;
        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<CardPlan, DeliveryError>> {
                    let tx = conn.transaction()?;
                    let Some(pool) = cards::get_pool(&tx, &pool_id)? else {
                        // 引用的池不存在(删除被引用池会被拒,此处防御性安全侧处理)
                        return Ok(Ok(CardPlan::Insufficient));
                    };
                    if !pool.enabled {
                        // 停用池不预留(quickstart US1-6:转待处理,不部分交付)
                        return Ok(Ok(CardPlan::Insufficient));
                    }
                    match pool.kind.as_str() {
                        "data" => {
                            let mut reserved: Vec<ReservedCard> = Vec::with_capacity(units as usize);
                            for _ in 0..units {
                                let Some(entry) =
                                    cards::reserve_one(&tx, &pool_id, &order_id, &delivery)?
                                else {
                                    // 余量不足:tx 在 return 时 drop → 整体回滚,不部分交付
                                    return Ok(Ok(CardPlan::Insufficient));
                                };
                                let aad = Aad {
                                    purpose: "card_entry".into(),
                                    entity_id: entry.id.clone(),
                                    content_version: None,
                                };
                                let plain = match crypto::open(&key, &aad, &entry.envelope) {
                                    Ok(p) => p,
                                    Err(e) => return Ok(Err(DeliveryError::Crypto(e))),
                                };
                                let text = match String::from_utf8(plain) {
                                    Ok(t) => t,
                                    Err(_) => {
                                        return Ok(Err(DeliveryError::Crypto(
                                            CryptoError::OpenFailed,
                                        )))
                                    }
                                };
                                reserved.push(ReservedCard { id: entry.id, text });
                            }
                            tx.commit()?;
                            Ok(Ok(CardPlan::Entries {
                                pool_id,
                                cards: reserved,
                            }))
                        }
                        "text" | "image" => {
                            let Some(envelope) = pool.content_envelope else {
                                return Ok(Ok(CardPlan::Insufficient));
                            };
                            let aad = Aad {
                                purpose: "card_pool_content".into(),
                                entity_id: pool.id.clone(),
                                content_version: None,
                            };
                            let plaintext = match crypto::open(&key, &aad, &envelope) {
                                Ok(p) => p,
                                Err(e) => return Ok(Err(DeliveryError::Crypto(e))),
                            };
                            tx.commit()?;
                            Ok(Ok(CardPlan::FixedContent { pool_id, plaintext }))
                        }
                        // api:无本地库存;外部取卡不在事务内发起(端口见 application::ports::cards)
                        _ => Ok(Ok(CardPlan::Insufficient)),
                    }
                },
            )
            .await?? // DbError → rusqlite::Error 逐层剥离;业务错误原样透传
    }

    /// 冻结收尾(fixed_text 与卡密分支共用):快照 AAD 重新封装 → 绑定任务。
    async fn freeze_plaintext(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        rule_id: &str,
        plaintext: &[u8],
        digest: &str,
        source_content_id: String,
    ) -> Result<bool, DeliveryError> {
        let snapshot_id = ids::new_id("snap");
        let aad = Aad {
            purpose: "content_snapshot".into(),
            entity_id: snapshot_id.clone(),
            content_version: None,
        };
        let envelope = crypto::seal(&self.key.key, &self.key.key_id, &aad, plaintext);
        let order_id = facts.order_id.clone();
        let delivery = delivery_id.to_string();
        let rule_id = rule_id.to_string();
        let digest = digest.to_string();
        self.db
            .call(move |conn| {
                deliveries::insert_snapshot(
                    conn,
                    &deliveries::NewSnapshot {
                        id: &snapshot_id,
                        order_id: &order_id,
                        source_content_id: &source_content_id,
                        envelope: &envelope,
                        digest: &digest,
                    },
                )?;
                let current = deliveries::get(conn, &delivery)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                deliveries::bind_snapshot(conn, &delivery, &rule_id, &snapshot_id, current.version)
            })
            .await??;
        Ok(true)
    }

    /// T3:guard → attempt(dispatching, request_id 先落库)→ handoff → 分类。
    async fn dispatch(
        &self,
        facts: &DeliveryFacts,
        snapshot: &crate::domain::orders::snapshot::OrderSnapshot,
        delivery_id: String,
    ) -> Result<HandleOutcome, DeliveryError> {
        // guard:自动/人工共用互斥
        let guard = self
            .db
            .call({
                let order_id = facts.order_id.clone();
                let delivery = delivery_id.clone();
                move |conn| deliveries::acquire_guard(conn, &order_id, &delivery, "")
            })
            .await??;
        if guard.is_none() {
            return Ok(HandleOutcome::Busy);
        }

        // T3 持久化 attempt(请求 ID 先于 handoff 落库;事务结果不明确不发送)
        let current = self
            .db
            .call({
                let delivery = delivery_id.clone();
                move |conn| deliveries::get(conn, &delivery)
            })
            .await??;
        let Some(current) = current else {
            return Ok(HandleOutcome::AlreadyHandled);
        };
        let request_id = ids::new_request_key();
        let attempt_id = ids::new_id("att");
        let attempt_for_prepare = attempt_id.clone();
        let attempt = self
            .db
            .call({
                let delivery = delivery_id.clone();
                let request_id = request_id.clone();
                move |conn| {
                    deliveries::prepare_attempt(
                        conn,
                        &attempt_for_prepare,
                        &delivery,
                        "initial",
                        &request_id,
                        Some(current.version),
                    )
                }
            })
            .await??;
        let Some(attempt) = attempt else {
            // 版本竞争或已有未决尝试:释放 guard,交由既有执行
            self.release_guard(&facts.order_id, "released").await?;
            return Ok(HandleOutcome::Busy);
        };
        // dispatching 状态持久化(带状态机校验),崩溃恢复据此归类 unknown
        let delivery_for_state = delivery_id.clone();
        self.set_full_state(
            &delivery_for_state,
            ContentState::Dispatching,
            ReviewState::None,
            None,
            None,
        )
        .await?;

        // 暂停屏障执行侧(T048):handoff 前重读账号,控制代次或开关变化即中止。
        // 新鲜度窗口内平台外部状态仍可能变化——不能承诺与平台原子(计划 §正常交付 5)
        {
            let account_probe = facts.account.id.clone();
            let expected_epoch = facts.account.control_epoch;
            let fresh = self
                .db
                .call(move |conn| accounts::get(conn, &account_probe))
                .await??;
            let fresh = fresh.ok_or(PlatformError::IncompleteFacts("账号不存在".into()))?;
            if fresh.control_epoch != expected_epoch
                || !fresh.runtime_enabled
                || !fresh.auto_delivery_enabled
                || fresh.status != "online"
            {
                self.db
                    .call({
                        let attempt = attempt.id.clone();
                        move |conn| {
                            deliveries::finish_attempt(
                                conn,
                                &attempt,
                                "cancelled",
                                Some("control_epoch_changed"),
                                None,
                            )
                        }
                    })
                    .await??;
                self.set_full_state(
                    &delivery_id,
                    ContentState::NotSent,
                    ReviewState::None,
                    None,
                    None,
                )
                .await?;
                self.release_guard(&facts.order_id, "released").await?;
                return Ok(HandleOutcome::NotSent {
                    delivery_id: delivery_id.to_string(),
                    retry_scheduled: false,
                    reason: Some("控制代次变化或开关关闭,已取消发送".into()),
                });
            }
        }

        // handoff:网络在数据库事务之外
        self.db
            .call({
                let attempt_id = attempt_id.clone();
                move |conn| deliveries::mark_handoff(conn, &attempt_id)
            })
            .await??;
        let buyer = snapshot
            .buyer_id
            .verified()
            .cloned()
            .ok_or(PlatformError::IncompleteFacts("买家身份缺失".into()))?;
        let snapshot_row = self
            .db
            .call({
                let snap_id = current.content_snapshot_id.clone().unwrap_or_default();
                move |conn| deliveries::get_snapshot(conn, &snap_id)
            })
            .await??;
        let Some(snapshot_row) = snapshot_row else {
            self.release_guard(&facts.order_id, "released").await?;
            return Err(DeliveryError::Platform(PlatformError::IncompleteFacts(
                "内容快照缺失".into(),
            )));
        };
        let snap_aad = Aad {
            purpose: "content_snapshot".into(),
            entity_id: snapshot_row.id.clone(),
            content_version: None,
        };
        let plaintext = crypto::open(&self.key.key, &snap_aad, &snapshot_row.envelope)?;
        // 快照正文由本系统以 UTF-8 字符串封存,解密还原必然合法
        let text = String::from_utf8(plaintext).expect("快照正文应为 UTF-8");
        let content = ContentForSend {
            snapshot_id: snapshot_row.id.clone(),
            text,
            text_digest: snapshot_row.digest.clone(),
        };

        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: facts.account.id.clone(),
            credential_generation: facts.account.credential_epoch,
            control_generation: facts.account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        let outcome = self
            .adapter
            .send_text(&ctx, &facts.order_id, &buyer, &content)
            .await;

        self.classify_and_persist(facts, &delivery_id, &attempt.id, outcome)
            .await
    }

    async fn classify_and_persist(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        attempt_id: &str,
        outcome: Result<SendOutcome, PlatformError>,
    ) -> Result<HandleOutcome, DeliveryError> {
        // 结果分类:传输错误若可能已提交按 unknown 处理(§6)
        let outcome = match outcome {
            Ok(o) => o,
            Err(PlatformError::CancelledBeforeSubmit) => {
                SendOutcome::NotSubmitted { retryable: true }
            }
            Err(PlatformError::Timeout)
            | Err(PlatformError::NetworkUnavailable)
            | Err(PlatformError::MalformedResponse) => SendOutcome::Unknown { hint: None },
            Err(e) => return Err(e.into()),
        };
        let order_id = facts.order_id.clone();
        match outcome {
            SendOutcome::Accepted(proof) => {
                let proof_ref = ids::new_id("prf");
                let attempt = attempt_id.to_string();
                let proof_owned = proof.clone();
                let delivery_for_consume = delivery_id.to_string();
                self.db
                    .call(move |conn| {
                        deliveries::insert_proof(
                            conn,
                            &proof_ref,
                            &attempt,
                            "platform",
                            proof_owned.platform_message_id.as_deref(),
                            &proof_owned.request_id,
                            Some(proof_owned.buyer_id.as_str()),
                            &proof_owned.content_digest,
                            None,
                        )?;
                        deliveries::finish_attempt(
                            conn,
                            &attempt,
                            "accepted",
                            None,
                            Some(&proof_ref),
                        )?;
                        // 卡密分支:平台接纳 → 预留条目扣减为 used(research D3;
                        // fixed_text 路径无预留,空操作)
                        cards::consume_by_delivery(conn, &delivery_for_consume)
                    })
                    .await??;
                let result = self
                    .set_content_state(
                        delivery_id,
                        ContentState::Accepted,
                        ReviewState::Resolved,
                        "platform",
                    )
                    .await?;
                self.release_guard(&order_id, "terminal").await?;
                if !result {
                    return Ok(HandleOutcome::AlreadyHandled);
                }
                // 独立平台确认(FR-016):开关开启才发起;失败只核验确认步骤
                if facts.account.auto_confirm_enabled {
                    self.confirm_shipment(facts, delivery_id, &proof).await?;
                }
                Ok(HandleOutcome::Delivered {
                    delivery_id: delivery_id.to_string(),
                })
            }
            SendOutcome::NotSubmitted { retryable } => {
                self.db
                    .call({
                        let attempt = attempt_id.to_string();
                        move |conn| {
                            deliveries::finish_attempt(
                                conn,
                                &attempt,
                                "not_sent",
                                Some("not_submitted"),
                                None,
                            )
                        }
                    })
                    .await??;
                let current = self.get_delivery(delivery_id).await?;
                let Some(current) = current else {
                    return Ok(HandleOutcome::AlreadyHandled);
                };
                let used = current.retry_count;
                let (scheduled, new_state, review, next_retry) =
                    if retryable && used < MAX_AUTO_RETRIES {
                        let delay =
                            RETRY_DELAYS_MS[(used.max(0) as usize).min(RETRY_DELAYS_MS.len() - 1)];
                        (
                            true,
                            ContentState::NotSent,
                            ReviewState::None,
                            Some(utc_now_ms() + delay),
                        )
                    } else {
                        // 预算耗尽或永久拒绝:required,只能显式人工动作
                        (false, ContentState::NotSent, ReviewState::Required, None)
                    };
                self.set_full_state(delivery_id, new_state, review, Some(used + 1), next_retry)
                    .await?;
                self.release_guard(&order_id, "released").await?;
                if !scheduled {
                    let delivery = delivery_id.to_string();
                    let account = facts.account.id.clone();
                    let reason = if retryable {
                        "自动重试预算耗尽".to_string()
                    } else {
                        "平台拒绝接收".to_string()
                    };
                    let order = order_id.clone();
                    self.db
                        .call(move |conn| {
                            // 卡密分支:terminal not_sent(确定未发送)→ 释放预留回库存
                            // (research D3;fixed_text 路径无预留,空操作)
                            cards::release_by_delivery(conn, &delivery)?;
                            issues::open(
                                conn,
                                &ids::new_id("iss"),
                                Some(&order),
                                &account,
                                Some(&delivery),
                                "delivery_retry_exhausted",
                                &reason,
                                "[\"resend\",\"terminate\"]",
                            )
                        })
                        .await??;
                }
                Ok(HandleOutcome::NotSent {
                    delivery_id: delivery_id.to_string(),
                    retry_scheduled: scheduled,
                    reason: None,
                })
            }
            SendOutcome::Rejected { safe_code } => {
                // 卡密分支:平台永久拒绝保留预留不释放——人工补发复用同一冻结
                // 快照与同一张卡;只有 terminal not_sent(NotSubmitted 预算耗尽)才释放
                let code_for_attempt = safe_code.clone();
                self.db
                    .call({
                        let attempt = attempt_id.to_string();
                        move |conn| {
                            deliveries::finish_attempt(
                                conn,
                                &attempt,
                                "not_sent",
                                Some(&code_for_attempt),
                                None,
                            )
                        }
                    })
                    .await??;
                self.set_full_state(
                    delivery_id,
                    ContentState::NotSent,
                    ReviewState::Required,
                    None,
                    None,
                )
                .await?;
                self.release_guard(&order_id, "released").await?;
                let delivery = delivery_id.to_string();
                let account = facts.account.id.clone();
                let code = safe_code.clone();
                let order = order_id.clone();
                self.db
                    .call(move |conn| {
                        issues::open(
                            conn,
                            &ids::new_id("iss"),
                            Some(&order),
                            &account,
                            Some(&delivery),
                            "delivery_rejected",
                            &format!("平台永久拒绝:{code}"),
                            "[\"resend\",\"terminate\"]",
                        )
                    })
                    .await??;
                Ok(HandleOutcome::NotSent {
                    delivery_id: delivery_id.to_string(),
                    retry_scheduled: false,
                    reason: Some(safe_code),
                })
            }
            SendOutcome::Unknown { hint } => {
                // 卡密分支:结果未知保持 reserved 不释放(宪章 I:结果未知不回库、
                // 不换卡、不自动重发),由 delivery_unknown 事项人工裁决后再处置
                self.db
                    .call({
                        let attempt = attempt_id.to_string();
                        move |conn| {
                            deliveries::finish_attempt(
                                conn,
                                &attempt,
                                "unknown",
                                hint.as_deref(),
                                None,
                            )
                        }
                    })
                    .await??;
                self.set_content_state(
                    delivery_id,
                    ContentState::Unknown,
                    ReviewState::Required,
                    "none",
                )
                .await?;
                // guard 不释放:未知结果禁止自动重试,只能显式人工解决(data-model)
                self.release_guard(&order_id, "unknown_manual").await?;
                let delivery = delivery_id.to_string();
                let account = facts.account.id.clone();
                let order = order_id.clone();
                self.db
                    .call(move |conn| {
                        issues::open(
                            conn,
                            &ids::new_id("iss"),
                            Some(&order),
                            &account,
                            Some(&delivery),
                            "delivery_unknown",
                            "发送结果未知,需要人工核对",
                            "[\"resend\",\"mark_received\",\"terminate\"]",
                        )
                    })
                    .await??;
                Ok(HandleOutcome::Unknown {
                    delivery_id: delivery_id.to_string(),
                })
            }
        }
    }

    /// 独立平台确认:失败/未知只影响确认轴,绝不触发正文发送(§8)。
    async fn confirm_shipment(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        proof: &crate::application::ports::platform::SendProof,
    ) -> Result<(), DeliveryError> {
        let request_id = ids::new_request_key();
        let attempt_id = ids::new_id("att");
        let attempt_for_prepare = attempt_id.clone();
        let prepared = self
            .db
            .call({
                let delivery = delivery_id.to_string();
                move |conn| {
                    deliveries::prepare_attempt(
                        conn,
                        &attempt_for_prepare,
                        &delivery,
                        "confirmation",
                        &request_id,
                        None,
                    )
                }
            })
            .await??;
        if prepared.is_none() {
            // 已有确认 attempt 在途/完成:独立轴不阻塞正文结果
            return Ok(());
        }
        // 发起确认:内容 accepted 且开关开启(调用方已保证);Disabled→Pending→Dispatching
        self.set_confirmation_state(delivery_id, ConfirmationState::Pending)
            .await?;
        self.set_confirmation_state(delivery_id, ConfirmationState::Dispatching)
            .await?;
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: facts.account.id.clone(),
            credential_generation: facts.account.credential_epoch,
            control_generation: facts.account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        let order_id = facts.order_id.clone();
        let external = self
            .db
            .call(move |conn| orders::get_order_external(conn, &order_id))
            .await??;
        let outcome = match self
            .adapter
            .confirm_shipment(&ctx, external.as_deref().unwrap_or(""), proof)
            .await
        {
            Ok(o) => o,
            Err(PlatformError::Timeout | PlatformError::NetworkUnavailable) => {
                ConfirmOutcome::Unknown
            }
            Err(e) => return Err(e.into()),
        };
        let attempt = attempt_id.clone();
        let _delivery = delivery_id.to_string();
        match outcome {
            ConfirmOutcome::Accepted => {
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(conn, &attempt, "accepted", None, None)
                    })
                    .await??;
                self.set_confirmation_state(delivery_id, ConfirmationState::Accepted)
                    .await?;
            }
            ConfirmOutcome::Rejected { safe_code } => {
                let code = safe_code.clone();
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(conn, &attempt, "not_sent", Some(&code), None)
                    })
                    .await??;
                self.set_confirmation_state(delivery_id, ConfirmationState::Rejected)
                    .await?;
            }
            ConfirmOutcome::NotSubmitted => {
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(conn, &attempt, "cancelled", None, None)
                    })
                    .await??;
                self.set_confirmation_state(delivery_id, ConfirmationState::Pending)
                    .await?;
            }
            ConfirmOutcome::Unknown => {
                self.db
                    .call(move |conn| {
                        deliveries::finish_attempt(conn, &attempt, "unknown", None, None)
                    })
                    .await??;
                self.set_confirmation_state(delivery_id, ConfirmationState::Unknown)
                    .await?;
            }
        }
        Ok(())
    }

    async fn get_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<deliveries::DeliveryRow>, DeliveryError> {
        let delivery = delivery_id.to_string();
        Ok(self
            .db
            .call(move |conn| deliveries::get(conn, &delivery))
            .await??)
    }

    async fn set_content_state(
        &self,
        delivery_id: &str,
        to: ContentState,
        review: ReviewState,
        evidence: &str,
    ) -> Result<bool, DeliveryError> {
        let delivery = delivery_id.to_string();
        let evidence = evidence.to_string();
        let updated = self
            .db
            .call(move |conn| {
                let current = deliveries::get(conn, &delivery)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                // 唯一约束之外的最终防线:状态机校验
                content_transition(parse_content(&current.content_state), to)
                    .map_err(illegal_transition_sql)?;
                conn.execute(
                    "UPDATE deliveries SET content_state = ?2, review_state = ?3,
                            evidence_origin = ?4, version = version + 1, updated_at = ?5
                     WHERE id = ?1",
                    rusqlite::params![
                        delivery,
                        to.as_str(),
                        review.as_str(),
                        evidence,
                        utc_now_ms()
                    ],
                )?;
                Ok::<_, rusqlite::Error>(true)
            })
            .await??;
        Ok(updated)
    }

    async fn set_full_state(
        &self,
        delivery_id: &str,
        to: ContentState,
        review: ReviewState,
        retry_count: Option<i64>,
        next_retry_at: Option<i64>,
    ) -> Result<(), DeliveryError> {
        let delivery = delivery_id.to_string();
        self.db
            .call(move |conn| {
                let current = deliveries::get(conn, &delivery)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                content_transition(parse_content(&current.content_state), to)
                    .map_err(illegal_transition_sql)?;
                conn.execute(
                    "UPDATE deliveries SET content_state = ?2, review_state = ?3,
                            retry_count = COALESCE(?4, retry_count), next_retry_at = ?5,
                            version = version + 1, updated_at = ?6
                     WHERE id = ?1",
                    rusqlite::params![
                        delivery,
                        to.as_str(),
                        review.as_str(),
                        retry_count,
                        next_retry_at,
                        utc_now_ms()
                    ],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await??;
        Ok(())
    }

    async fn set_confirmation_state(
        &self,
        delivery_id: &str,
        to: ConfirmationState,
    ) -> Result<(), DeliveryError> {
        let delivery = delivery_id.to_string();
        self.db
            .call(move |conn| {
                let current = deliveries::get(conn, &delivery)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                confirmation_transition(parse_confirmation(&current.confirmation_state), to)
                    .map_err(illegal_transition_sql)?;
                conn.execute(
                    "UPDATE deliveries SET confirmation_state = ?2, version = version + 1,
                            updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![delivery, to.as_str(), utc_now_ms()],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await??;
        Ok(())
    }

    async fn release_guard(&self, order_id: &str, state: &str) -> Result<(), DeliveryError> {
        let order = order_id.to_string();
        let state = state.to_string();
        self.db
            .call(move |conn| deliveries::release_guard(conn, &order, &state))
            .await??;
        Ok(())
    }
}

fn parse_content(s: &str) -> ContentState {
    match s {
        "pending_verification" => ContentState::PendingVerification,
        "queued" => ContentState::Queued,
        "dispatching" => ContentState::Dispatching,
        "not_sent" => ContentState::NotSent,
        "accepted" => ContentState::Accepted,
        "unknown" => ContentState::Unknown,
        _ => ContentState::Terminated,
    }
}

fn parse_confirmation(s: &str) -> ConfirmationState {
    match s {
        "pending" => ConfirmationState::Pending,
        "dispatching" => ConfirmationState::Dispatching,
        "accepted" => ConfirmationState::Accepted,
        "rejected" => ConfirmationState::Rejected,
        "unknown" => ConfirmationState::Unknown,
        "terminated" => ConfirmationState::Terminated,
        _ => ConfirmationState::Disabled,
    }
}

/// 状态机拒绝迁移 → 数据库错误(最终防线;正常路径不应触达)。
fn illegal_transition_sql(e: crate::domain::delivery::state::IllegalTransition) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

/// 触发来源:自动路径比较监控起点;显式接管允许历史(其余核验相同)。
#[derive(Clone, Copy, Debug)]
pub enum DeliveryTrigger {
    Auto,
    ManualTakeover,
}

// ---- T012 卡密分支内容计划(research D2/D3) ----

/// freeze_snapshot 装载的内容来源。
enum LoadedSource {
    /// 固定文本(001 既有行为)
    FixedText {
        content: rules::RuleContentRow,
        rule: rules::RuleRow,
    },
    /// 卡密组来源(rule.card_pool_id 非空)
    CardPool { rule: rules::RuleRow, pool_id: String },
    /// 规则或内容缺失 → 冻结失败(既有语义)
    Missing,
}

/// 卡密分支单事务规划结果。
enum CardPlan {
    /// data 池:预留成功的条目(明文仅存在至快照封存)
    Entries { pool_id: String, cards: Vec<ReservedCard> },
    /// text/image 池:固定内容直接冻结
    FixedContent { pool_id: String, plaintext: Vec<u8> },
    /// 余量不足/停用/无本地库存:已回滚,调用方开 stock_insufficient 事项
    Insufficient,
}

struct ReservedCard {
    id: String,
    text: String,
}

impl<A: PlatformAdapter> DeliveryService<A> {
    /// 追溯透传:扫描服务复用同一适配器(只读,不触发发送)。
    pub async fn trace(
        &self,
        ctx: &RequestContext,
        from_ms: i64,
        to_ms: i64,
        max_pages: u32,
    ) -> Result<
        (
            Vec<String>,
            crate::application::ports::platform::TraceReport,
        ),
        PlatformError,
    > {
        self.adapter
            .trace_sold_orders(ctx, from_ms, to_ms, max_pages)
            .await
    }
}

impl<A: PlatformAdapter> DeliveryService<A> {
    /// 人工补发使用的发送通道:与自动路径同一适配器入口(互斥由 guard 保证)。
    pub async fn send_manual(
        &self,
        ctx: &RequestContext,
        order_db_id: &str,
        _external_order_id: &str,
        buyer_id: &str,
        content: &ContentForSend,
    ) -> Result<SendOutcome, PlatformError> {
        self.adapter
            .send_text(ctx, order_db_id, buyer_id, content)
            .await
    }
}
