//! 交付编排(T031—T035):T1 唯一任务 → T2 冻结快照 → T3 守卫+attempt 先落库
//! → handoff → 结果分类 → 证明 → 独立平台确认 → 重试预算。
//! 网络绝不进入数据库事务;unknown 永不自动重发(宪章 I)。

use std::collections::{HashMap, HashSet};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{
    accounts, cards, deliveries, issues, items, orders, rules, rules_ext, templates,
};
use crate::adapters::windows::keys::DataKey;
use crate::application::delivery::eligibility::{self, Eligibility, EligibilityInput, Reason};
use crate::application::ports::platform::{
    ConfirmOutcome, ContentForSend, PlatformAdapter, PlatformError, RequestContext, SendOutcome,
};
use crate::domain::crypto::{self, Aad, CryptoError};
use crate::domain::rules_ext as ext;
use crate::domain::delivery::state::{
    ConfirmationState, ContentState, ReviewState, confirmation_transition, content_transition,
};
use crate::domain::ids;
use crate::domain::templates::{
    RenderCard, RenderContext, TemplateBindings, render_messages,
};
use crate::domain::time_util::utc_now_ms;
use sha2::{Digest, Sha256};

/// 自动重试预算:初次发送外最多 3 次,间隔 30/60/120 秒(FR-019,spec 默认值)。
pub const MAX_AUTO_RETRIES: i64 = 3;
pub const RETRY_DELAYS_MS: [i64; 3] = [30_000, 60_000, 120_000];

/// 007 T038:规则需配置类待处理(独立于 delivery_ineligible;
/// 仅允许 terminate——修复/确认规则后由正常流程重入,不存在"重发"语义)。
pub const ISSUE_KIND_RULE_NEEDS_CONFIG: &str = "rule_needs_config";
pub const REASON_NEEDS_CONFIRMATION: &str = "needs_confirmation";
pub const REASON_NEEDS_RECONFIGURATION: &str = "needs_reconfiguration";

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
    /// 007 T062(US5):终态交付结果通知;None(默认)零行为变化。
    notify: Option<std::sync::Arc<crate::application::notify::NotifyService>>,
}

struct DeliveryFacts {
    order_id: String,
    account: accounts::AccountRow,
    item: Option<items::ItemRow>,
    rule: Option<rules::RuleRow>,
    rule_ambiguous: bool,
    /// 007 US3(T034):命中规则需确认/需重新配置时的待处理详情
    rule_needs_config: Option<String>,
    /// 007 US3(T038):需配置子类 reason_code(needs_confirmation/needs_reconfiguration)
    rule_needs_code: Option<&'static str>,
    restore_quarantined: bool,
    paid_at: Option<i64>,
    buyer_id: Option<String>,
    /// 可信购买件数(卡密预留 units = 份数×件数;缺失按 1)
    quantity: Option<i64>,
    #[allow(dead_code)]
    sku_key: Option<String>,
    /// 可信规格对(变体匹配事实;单规格/缺失为空;裁决后经 matched_variant 生效)
    #[allow(dead_code)]
    sku_parts: Vec<crate::domain::sku::SkuPart>,
    /// 007 US3(T034):变体命中来源(覆盖规则级默认注入 ContentPlan)
    matched_variant: Option<ext::RuleVariant>,
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

/// 007 T038:verdict 为规则需配置类时,取事实装载阶段的子类 reason_code
/// (Decision::NeedsConfirmation/NeedsReconfiguration 与 code 一一对应)。
fn needs_config_code(facts: &DeliveryFacts, reason: &Reason) -> Option<&'static str> {
    match reason {
        Reason::RuleNeedsConfig(_) => facts.rule_needs_code,
        _ => None,
    }
}

impl<A: PlatformAdapter> DeliveryService<A> {
    pub fn new(db: DbThread, key: DataKey, adapter: A) -> Self {
        Self {
            db,
            key,
            adapter,
            notify: None,
        }
    }

    /// 007 T062:注入通知服务(supervisor 侧;终态 not_sent/unknown →
    /// delivery_result 通知;accepted 不通知——降噪取舍见 research D8)。
    pub fn with_notify(
        mut self,
        notify: std::sync::Arc<crate::application::notify::NotifyService>,
    ) -> Self {
        self.notify = Some(notify);
        self
    }

    /// 终态交付结果通知(1—3 行挂钩辅助;未注入时零行为)。
    /// outcome:not_sent(terminal)/unknown;订单号+金额脱敏摘要由服务侧构造。
    fn notify_delivery(&self, facts: &DeliveryFacts, outcome: &str, detail: &str) {
        if let Some(notify) = self.notify.as_ref() {
            notify.notify_order_result(&facts.account.id, &facts.order_id, outcome, detail);
        }
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
        let snapshot_sku_parts = match &snapshot.sku_parts {
            crate::domain::orders::snapshot::FieldStatus::Verified(parts) => parts.clone(),
            _ => Vec::new(),
        };
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
                // 007 US3(T034)规则命中:order_paid 候选(商品精确/任意规格/账号级回退)
                // → 领域裁决(层级+优先级+变体+门禁);仅最高优先级一条执行
                let scope_item = item.as_ref().map(|i| i.id.clone());
                let decision = {
                    let rows = rules_ext::find_order_paid_candidates(
                        conn,
                        &account.id,
                        scope_item.as_deref(),
                        &sku_key,
                    )?;
                    let mut candidates = Vec::with_capacity(rows.len());
                    for row in rows {
                        let variants = rules_ext::list_variants(conn, &row.id)?;
                        candidates.push(rules_ext::to_candidate(&row, &variants));
                    }
                    let scope = ext::MatchScope {
                        item_id: scope_item.as_deref().unwrap_or(""),
                        sku_key: &sku_key,
                        sku_parts: &snapshot_sku_parts,
                    };
                    ext::select(&scope, &candidates)
                };
                let (rule, ambiguous, needs_config, needs_code, matched_variant) = match decision {
                    ext::Decision::Execute { rule_id, variant } => {
                        let row = rules::get(conn, &rule_id)?;
                        (row, false, None, None, variant)
                    }
                    ext::Decision::Ambiguous { .. } => (None, true, None, None, None),
                    ext::Decision::NeedsConfirmation { .. } => (
                        None,
                        false,
                        Some("账号级规则未确认「适用于全部商品」,需确认·暂不发货".to_string()),
                        Some(REASON_NEEDS_CONFIRMATION),
                        None,
                    ),
                    ext::Decision::NeedsReconfiguration { .. } => (
                        None,
                        false,
                        Some("规则引用对象缺失(卡密组/模板),需重新配置后执行".to_string()),
                        Some(REASON_NEEDS_RECONFIGURATION),
                        None,
                    ),
                    ext::Decision::NoRule => (None, false, None, None, None),
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
                    rule_needs_config: needs_config,
                    rule_needs_code: needs_code,
                    restore_quarantined: restore_quarantined > 0,
                    paid_at: snapshot_paid_at,
                    buyer_id: snapshot_buyer,
                    quantity: snapshot_quantity,
                    sku_key: Some(sku_key),
                    sku_parts: snapshot_sku_parts,
                    matched_variant,
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
            rule_needs_config: facts.rule_needs_config.as_deref(),
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
                            self.open_ineligible_issue(
                                &facts,
                                &existing.id,
                                &reason.to_string(),
                                needs_config_code(&facts, &reason),
                            )
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
                self.open_ineligible_issue(
                    &facts,
                    &delivery.id,
                    &reason.to_string(),
                    needs_config_code(&facts, &reason),
                )
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

    /// 不合格原因 → 待处理事项。007 T038:规则需配置类(需确认/需重新配置)
    /// 用独立 kind=rule_needs_config,reason_code 区分子类,
    /// allowed_actions 仅 terminate(修复规则后由正常流程重入);
    /// 其余原因沿用 delivery_ineligible(resend/terminate)。去重靠既有索引。
    async fn open_ineligible_issue(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        reason: &str,
        needs_config_code: Option<&'static str>,
    ) -> Result<(), DeliveryError> {
        let order_id = facts.order_id.clone();
        let account_id = facts.account.id.clone();
        let delivery = delivery_id.to_string();
        let reason_owned = reason.to_string();
        self.db
            .call(move |conn| {
                let (kind, reason_code, allowed) = match needs_config_code {
                    Some(code) => (ISSUE_KIND_RULE_NEEDS_CONFIG, code, "[\"terminate\"]"),
                    None => (
                        "delivery_ineligible",
                        reason_owned.as_str(),
                        "[\"resend\",\"terminate\"]",
                    ),
                };
                issues::open(
                    conn,
                    &ids::new_id("iss"),
                    Some(&order_id),
                    &account_id,
                    Some(&delivery),
                    kind,
                    reason_code,
                    allowed,
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
    /// card_pool:单事务原子预留拼接送文本,research D3 预留协议;
    /// template:逐绑定原子预留后渲染真值消息数组,快照存 JSON 数组
    /// (US2,research D4;source_content_id=`template:{id}`)。
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
        let variant_effect = facts.matched_variant.clone();
        let loaded = self
            .db
            .call({
                let rule_id = rule_id.clone();
                move |conn| -> rusqlite::Result<LoadedSource> {
                    let rule = rules::get(conn, &rule_id)?;
                    let Some(rule) = rule else { return Ok(LoadedSource::Missing) };
                    // T034:变体命中 → 变体来源覆盖规则级默认(source/units/delay 注入 ContentPlan)
                    if let Some(effect) = &variant_effect {
                        match effect.source {
                            ext::VariantSource::CardPool => {
                                let pool_id = effect
                                    .card_pool_id
                                    .clone()
                                    .unwrap_or_default();
                                if pool_id.is_empty() {
                                    return Ok(LoadedSource::Missing);
                                }
                                return Ok(LoadedSource::CardPool {
                                    rule,
                                    pool_id,
                                    units_per_item: effect.units_per_item,
                                    delay_override_seconds: effect.delay_override_seconds,
                                });
                            }
                            ext::VariantSource::Template => {
                                let template_id =
                                    effect.template_id.clone().unwrap_or_default();
                                if template_id.is_empty() {
                                    return Ok(LoadedSource::Missing);
                                }
                                if templates::get_template(conn, &template_id)?.is_none() {
                                    return Ok(LoadedSource::Missing);
                                }
                                let messages = templates::list_messages(conn, &template_id)?;
                                if messages.is_empty() {
                                    return Ok(LoadedSource::Missing);
                                }
                                return Ok(LoadedSource::Template {
                                    template_id,
                                    messages,
                                    bindings_json: effect.template_bindings.clone(),
                                });
                            }
                        }
                    }
                    // 规则级来源(无变体命中):模板优先(US2),其次卡密组,兜底固定内容
                    if let Some(template_id) = rule.template_id.clone().filter(|t| !t.is_empty()) {
                        if templates::get_template(conn, &template_id)?.is_none() {
                            // 引用的模板不存在(删除被拒,此处防御性安全侧处理)
                            return Ok(LoadedSource::Missing);
                        }
                        let messages = templates::list_messages(conn, &template_id)?;
                        if messages.is_empty() {
                            return Ok(LoadedSource::Missing);
                        }
                        return Ok(LoadedSource::Template {
                            bindings_json: rule.template_bindings.clone(),
                            template_id,
                            messages,
                        });
                    }
                    let pool_binding = rule.card_pool_id.clone();
                    match pool_binding.as_deref() {
                        Some(pool_id) if !pool_id.is_empty() => Ok(LoadedSource::CardPool {
                            rule,
                            pool_id: pool_id.to_string(),
                            units_per_item: 1,
                            delay_override_seconds: None,
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
            LoadedSource::CardPool {
                rule,
                pool_id,
                units_per_item,
                delay_override_seconds,
            } => {
                // ---- card_pool 分支:同一 DbThread 事务闭包内原子预留(D3)----
                // 变体命中时 units_per_item/delay_override 覆盖规则级默认(FR-030);
                // delay 的执行侧消费(定时发送)尚未接入——与卡密组组级延时同状态,
                // 当前在计划点解析留待调度器(见 T037 求评/延迟调度)
                let _ = (rule, delay_override_seconds);
                let plan = self
                    .plan_card_pool(facts, delivery_id, &pool_id, units_per_item)
                    .await?;
                match plan {
                    CardPlan::Insufficient => {
                        // 余量不足/停用:整体回滚已完成(事务闭包内逐条预留,任一不足即回滚),
                        // 开 stock_insufficient 事项后返回 false(不部分交付)
                        self.open_stock_issue(facts, delivery_id).await?;
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
            LoadedSource::Template {
                template_id,
                messages,
                bindings_json,
            } => {
                // ---- template 分支(US2,research D4):逐 key 原子预留卡密 →
                // 渲染真值消息数组 → 快照 plaintext 存 JSON 数组 ----
                // T034:变体命中的模板来源以变体绑定覆盖规则级绑定;
                // 绑定 JSON 由规则保存侧校验;坏 JSON 防御性按空绑定处理,
                // 渲染变量缺失 → Invalid(视同 Missing,正门是 US3 保存校验)
                let bindings: TemplateBindings = bindings_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                match self
                    .plan_template(facts, delivery_id, messages, &bindings)
                    .await?
                {
                    TemplatePlan::Insufficient => {
                        // 余量不足/停用:与 US1 卡密分支同路径(不部分交付)
                        self.open_stock_issue(facts, delivery_id).await?;
                        Ok(false)
                    }
                    TemplatePlan::Invalid => Ok(false),
                    TemplatePlan::Ready { messages } => {
                        // 卡密真值只进加密快照;模板表只存占位符明文
                        let payload = serde_json::to_vec(&messages).expect("消息数组序列化");
                        let digest = hex::encode(Sha256::digest(&payload));
                        self.freeze_plaintext(
                            facts,
                            delivery_id,
                            &rule_id,
                            &payload,
                            &digest,
                            format!("template:{template_id}"),
                        )
                        .await
                    }
                }
            }
        }
    }

    /// 余量不足/停用转待处理事项(卡密与模板分支共用,US1 语义)。
    async fn open_stock_issue(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
    ) -> Result<(), DeliveryError> {
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
        Ok(())
    }

    /// 卡密分支单事务计划(D3):data 池循环预留 N=每件份数×购买件数
    /// (T034:变体命中时 units_per_item 覆盖规则级默认 1);任一次取不到 →
    /// 事务回滚(不部分交付)→ Insufficient;text/image 池用固定内容;
    /// api 池暂无本地库存按不足处理(CardSupplier 事务外取卡接线见 T011 端口;
    /// 不在此处外呼网络)。
    async fn plan_card_pool(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        pool_id: &str,
        units_per_item: i64,
    ) -> Result<CardPlan, DeliveryError> {
        let order_id = facts.order_id.clone();
        let delivery = delivery_id.to_string();
        let pool_id = pool_id.to_string();
        let units = facts.quantity.unwrap_or(1).max(1) * units_per_item.max(1);
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

    /// 模板分支单事务计划(US2,research D4):逐卡密绑定原子预留
    /// (复用 plan_card_pool 的预留语义:N=绑定份数×件数;任一不足整体回滚),
    /// 预留完成后以订单事实渲染真值消息数组。渲染函数只接收已取到的值
    /// (T023 约定);系统变量取快照可信事实(买家昵称无来源,回退买家 ID)。
    async fn plan_template(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        messages: Vec<String>,
        bindings: &TemplateBindings,
    ) -> Result<TemplatePlan, DeliveryError> {
        let order_id = facts.order_id.clone();
        let delivery = delivery_id.to_string();
        let quantity = facts.quantity.unwrap_or(1).max(1);
        let card_name = facts
            .item
            .as_ref()
            .map(|i| i.title.clone())
            .unwrap_or_default();
        let buyer = facts.buyer_id.clone().unwrap_or_default();
        let bindings = bindings.clone();
        let key = self.key.key;
        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<TemplatePlan, DeliveryError>> {
                    let tx = conn.transaction()?;
                    let external_order =
                        orders::get_order_external(&tx, &order_id)?.unwrap_or_default();
                    let mut cards_map: HashMap<String, RenderCard> = HashMap::new();
                    for binding in &bindings.cards {
                        let Some(pool) = cards::get_pool(&tx, &binding.pool_id)? else {
                            // 引用的池不存在(删除被引用池会被拒,防御性安全侧)
                            return Ok(Ok(TemplatePlan::Insufficient));
                        };
                        if !pool.enabled {
                            return Ok(Ok(TemplatePlan::Insufficient));
                        }
                        match pool.kind.as_str() {
                            "data" => {
                                let total = (binding.units.max(1) as usize) * (quantity as usize);
                                let mut texts = Vec::with_capacity(total);
                                for _ in 0..total {
                                    let Some(entry) = cards::reserve_one(
                                        &tx,
                                        &binding.pool_id,
                                        &order_id,
                                        &delivery,
                                    )? else {
                                        // 余量不足:tx drop → 整体回滚,不部分交付
                                        return Ok(Ok(TemplatePlan::Insufficient));
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
                                    texts.push(text);
                                }
                                let count = texts.len();
                                cards_map.insert(
                                    binding.key.clone(),
                                    RenderCard {
                                        text: texts.join("\n"),
                                        count,
                                    },
                                );
                            }
                            "text" | "image" => {
                                let Some(envelope) = pool.content_envelope else {
                                    return Ok(Ok(TemplatePlan::Insufficient));
                                };
                                let aad = Aad {
                                    purpose: "card_pool_content".into(),
                                    entity_id: pool.id.clone(),
                                    content_version: None,
                                };
                                let plain = match crypto::open(&key, &aad, &envelope) {
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
                                cards_map.insert(
                                    binding.key.clone(),
                                    RenderCard { text, count: 1 },
                                );
                            }
                            // api:无本地库存;外部取卡不在事务内发起(同 US1 语义)
                            _ => return Ok(Ok(TemplatePlan::Insufficient)),
                        }
                    }
                    let ctx = RenderContext {
                        buyer_nickname: buyer.clone(),
                        order_id: external_order,
                        buyer_id: buyer,
                        card_name,
                        cards: cards_map,
                        custom: bindings.custom.clone(),
                    };
                    match render_messages(&messages, &ctx, false) {
                        Ok(rendered) => {
                            tx.commit()?;
                            Ok(Ok(TemplatePlan::Ready { messages: rendered }))
                        }
                        // 变量缺失等渲染失败:防御路径(规则保存校验是正门)
                        Err(_) => Ok(Ok(TemplatePlan::Invalid)),
                    }
                },
            )
            .await??
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
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: facts.account.id.clone(),
            credential_generation: facts.account.credential_epoch,
            control_generation: facts.account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        if let Some(messages) = parse_message_array(&plaintext) {
            // 模板多消息(007 US2,research D4):逐条顺序发送、逐条留证、
            // 按已确认摘要续发;单条路径(fixed_text/card_pool)不进入此分支
            return self
                .dispatch_messages(
                    facts,
                    &delivery_id,
                    &attempt.id,
                    &snapshot_row,
                    &messages,
                    &buyer,
                    &ctx,
                )
                .await;
        }
        // 单条路径(fixed_text/card_pool,行为与 001/US1 逐字一致)
        // 快照正文由本系统以 UTF-8 字符串封存,解密还原必然合法
        let text = String::from_utf8(plaintext).expect("快照正文应为 UTF-8");
        let content = ContentForSend {
            snapshot_id: snapshot_row.id.clone(),
            text,
            text_digest: snapshot_row.digest.clone(),
        };

        let outcome = self
            .adapter
            .send_text(&ctx, &facts.order_id, &buyer, &content)
            .await;

        self.classify_and_persist(facts, &delivery_id, &attempt.id, outcome)
            .await
    }

    /// 多消息顺序发送(D4):前一条 Accepted 才发下一条;每条 Accepted
    /// 立即 insert 一行 delivery_proofs(同 attempt 多行,content_digest=该条摘要);
    /// 重试入口先读本交付全部 attempt 已确认的 digest 集合,跳过已确认条目;
    /// 中途 Rejected/NotSubmitted/Unknown → 按该条结果走既有分类
    /// (已发条目保留 proofs;attempt 落对应分类)。
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_messages(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        attempt_id: &str,
        snapshot_row: &deliveries::SnapshotRow,
        messages: &[String],
        buyer: &str,
        ctx: &RequestContext,
    ) -> Result<HandleOutcome, DeliveryError> {
        if messages.is_empty() {
            // 防御:冻结侧保证 1..=10 条;空数组按确定未发送处置
            return self
                .classify_and_persist(
                    facts,
                    delivery_id,
                    attempt_id,
                    Ok(SendOutcome::NotSubmitted { retryable: false }),
                )
                .await;
        }
        let confirmed: HashSet<String> = {
            let delivery = delivery_id.to_string();
            self.db
                .call(move |conn| deliveries::confirmed_digests(conn, &delivery))
                .await??
                .into_iter()
                .collect()
        };
        let mut last_proof: Option<crate::application::ports::platform::SendProof> = None;
        let mut last_proof_ref: Option<String> = None;
        for msg in messages {
            let digest = hex::encode(Sha256::digest(msg.as_bytes()));
            if confirmed.contains(&digest) {
                continue; // 已确认条目不重发(按 proof 摘要判定)
            }
            let content = ContentForSend {
                snapshot_id: snapshot_row.id.clone(),
                text: msg.clone(),
                text_digest: digest.clone(),
            };
            let outcome = self
                .adapter
                .send_text(ctx, &facts.order_id, buyer, &content)
                .await;
            match outcome {
                Ok(SendOutcome::Accepted(proof)) => {
                    // 逐条留证:立即落库(崩溃后仍可判定哪些条已确认送达)
                    let proof_ref = ids::new_id("prf");
                    let proof_ref_for_call = proof_ref.clone();
                    let attempt = attempt_id.to_string();
                    let platform_message_id = proof.platform_message_id.clone();
                    let request_id = proof.request_id.clone();
                    let proof_buyer = proof.buyer_id.clone();
                    let digest_owned = digest;
                    self.db
                        .call(move |conn| {
                            deliveries::insert_proof(
                                conn,
                                &proof_ref_for_call,
                                &attempt,
                                "platform",
                                platform_message_id.as_deref(),
                                &request_id,
                                Some(proof_buyer.as_str()),
                                &digest_owned,
                                None,
                            )
                        })
                        .await??;
                    last_proof = Some(proof);
                    last_proof_ref = Some(proof_ref);
                }
                // 中途失败:按该条结果分类收尾(已发条目保留 proofs)
                other => {
                    return self
                        .classify_and_persist(facts, delivery_id, attempt_id, other)
                        .await;
                }
            }
        }
        // 全部条目已 Accepted:收尾与单条 Accepted 分支同构
        let (proof_ref, proof) = match (last_proof_ref, last_proof) {
            (Some(proof_ref), Some(proof)) => (proof_ref, proof),
            (None, _) => {
                // 防御:本轮无新发条目且全部已确认(正常流程不可达)——
                // 以既有 proof 事实收尾 accepted
                let delivery = delivery_id.to_string();
                let latest = self
                    .db
                    .call(move |conn| deliveries::latest_proof_for_delivery(conn, &delivery))
                    .await??;
                let Some(latest) = latest else {
                    return self
                        .classify_and_persist(
                            facts,
                            delivery_id,
                            attempt_id,
                            Ok(SendOutcome::NotSubmitted { retryable: false }),
                        )
                        .await;
                };
                let proof = crate::application::ports::platform::SendProof {
                    request_id: latest.request_id,
                    platform_message_id: latest.platform_message_id,
                    buyer_id: latest.buyer_id.unwrap_or_default(),
                    chat_id: None,
                    content_digest: latest.content_digest,
                };
                (latest.id, proof)
            }
            _ => unreachable!("proof 与 proof_ref 成对出现"),
        };
        self.finish_accepted(facts, delivery_id, attempt_id, &proof_ref, &proof)
            .await
    }

    /// Accepted 收尾(单条与多消息共用):finish_attempt + 卡密扣减 +
    /// 内容轴 accepted + guard terminal + 独立平台确认。
    async fn finish_accepted(
        &self,
        facts: &DeliveryFacts,
        delivery_id: &str,
        attempt_id: &str,
        proof_ref: &str,
        proof: &crate::application::ports::platform::SendProof,
    ) -> Result<HandleOutcome, DeliveryError> {
        let attempt = attempt_id.to_string();
        let proof_ref = proof_ref.to_string();
        let delivery_for_consume = delivery_id.to_string();
        self.db
            .call(move |conn| {
                deliveries::finish_attempt(
                    conn,
                    &attempt,
                    "accepted",
                    None,
                    Some(&proof_ref),
                )?;
                // 卡密分支:平台接纳 → 预留条目扣减为 used(research D3;
                // fixed_text/模板无预留时为空操作;模板分支的绑定预留在此扣减)
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
        self.release_guard(&facts.order_id, "terminal").await?;
        if !result {
            return Ok(HandleOutcome::AlreadyHandled);
        }
        // 独立平台确认(FR-016):开关开启才发起;失败只核验确认步骤
        if facts.account.auto_confirm_enabled {
            self.confirm_shipment(facts, delivery_id, proof).await?;
        }
        Ok(HandleOutcome::Delivered {
            delivery_id: delivery_id.to_string(),
        })
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
                let proof_ref_for_call = proof_ref.clone();
                let attempt = attempt_id.to_string();
                let proof_owned = proof.clone();
                self.db
                    .call(move |conn| {
                        deliveries::insert_proof(
                            conn,
                            &proof_ref_for_call,
                            &attempt,
                            "platform",
                            proof_owned.platform_message_id.as_deref(),
                            &proof_owned.request_id,
                            Some(proof_owned.buyer_id.as_str()),
                            &proof_owned.content_digest,
                            None,
                        )
                    })
                    .await??;
                self.finish_accepted(facts, delivery_id, attempt_id, &proof_ref, &proof)
                    .await
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
                    let notify_reason = reason.clone();
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
                    // 007 T062:terminal not_sent → delivery_result 通知(US5)
                    self.notify_delivery(facts, "not_sent", &notify_reason);
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
                // 007 T062:永久拒绝(terminal not_sent)→ delivery_result 通知(US5)
                self.notify_delivery(facts, "not_sent", &format!("平台永久拒绝:{safe_code}"));
                Ok(HandleOutcome::NotSent {
                    delivery_id: delivery_id.to_string(),
                    retry_scheduled: false,
                    reason: Some(safe_code),
                })
            }
            SendOutcome::Unknown { hint } => {
                // 卡密分支:结果未知保持 reserved 不释放(宪章 I:结果未知不回库、
                // 不换卡、不自动重发),由 delivery_unknown 事项人工裁决后再处置
                let unknown_detail = hint
                    .as_deref()
                    .map(|h| format!("发送结果未知({h})"))
                    .unwrap_or_else(|| "发送结果未知".to_string());
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
                // 007 T062:结果未知 → delivery_result 通知(US5;含脱敏金额摘要)
                self.notify_delivery(facts, "unknown", &unknown_detail);
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

// ---- T012 卡密分支内容计划(research D2/D3);T024 模板分支(research D4) ----

/// freeze_snapshot 装载的内容来源。
/// T034:变体命中时 units_per_item / delay_override_seconds / bindings_json
/// 来自变体(覆盖规则级默认);规则级路径 units_per_item=1、delay=None。
enum LoadedSource {
    /// 固定文本(001 既有行为)
    FixedText {
        content: rules::RuleContentRow,
        rule: rules::RuleRow,
    },
    /// 卡密组来源(rule.card_pool_id 非空,或变体 source=card_pool)
    CardPool {
        rule: rules::RuleRow,
        pool_id: String,
        units_per_item: i64,
        delay_override_seconds: Option<i64>,
    },
    /// 发货模板来源(US2,rule.template_id 非空,或变体 source=template):
    /// 消息列表(占位符明文)+ 绑定 JSON(变体覆盖时来自变体)
    Template {
        template_id: String,
        messages: Vec<String>,
        bindings_json: Option<String>,
    },
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

/// 模板分支单事务规划结果(US2)。
enum TemplatePlan {
    /// 渲染完成的真值消息数组(待封存 JSON 数组快照)
    Ready { messages: Vec<String> },
    /// 余量不足/停用/绑定池不可用:已整体回滚,走 stock_insufficient 路径
    Insufficient,
    /// 渲染失败(变量缺失等,防御路径;正门是规则保存校验)
    Invalid,
}

/// 快照正文是否为模板多消息 JSON 数组(D4):以 '[' 开头且解析成功。
/// fixed_text/card_pool 的单条正文(即使以 '[' 起头)解析失败即走单条路径。
fn parse_message_array(plaintext: &[u8]) -> Option<Vec<String>> {
    if plaintext.first() != Some(&b'[') {
        return None;
    }
    serde_json::from_slice::<Vec<String>>(plaintext).ok()
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
    /// 人工确认平台已发货(007 US6,FR-062):仅包装适配器确认端口;
    /// 确认轴状态由调用方按 ConfirmOutcome 落库,内容轴不受影响。
    pub async fn confirm_shipment_manual(
        &self,
        ctx: &RequestContext,
        external_order_id: &str,
        proof: &crate::application::ports::platform::SendProof,
    ) -> Result<crate::application::ports::platform::ConfirmOutcome, PlatformError> {
        self.adapter
            .confirm_shipment(ctx, external_order_id, proof)
            .await
    }

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
