//! 规则用例(T057/T058/T059/T060 + 007 US3 T033):精确范围 CRUD、
//! 内容版本不可变、产品限制 1000 Unicode 标量 / 4000 UTF-8 字节(research R7,verbatim)、
//! 保存与执行前双重校验;预览纯校验零发送。
//! US3 扩展:trigger_type/priority/内容来源绑定/变体/求评配置/账号级确认门禁;
//! 同范围同触发同优先级冲突 → EnabledConflict(422 rule_conflict,D1);
//! 引用缺失或账号级未确认 → 落库并标记 needs_reconfiguration(执行旁路,修复后清除);
//! 固定内容路径零迁移零行为变化(FR-039)。

use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{accounts, cards, items, rules, rules_ext, templates};
use crate::adapters::windows::keys::DataKey;
use crate::domain::crypto;
use crate::domain::ids;
use crate::domain::rules_ext::{
    self as ext, ContentSource, ReviewConfig, TriggerType, VariantSource,
};
use crate::domain::templates::TemplateBindings;
use crate::domain::time_util::utc_now_ms;

pub const MAX_UNICODE_SCALARS: usize = 1000;
pub const MAX_UTF8_BYTES: usize = 4000;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("正文为空")]
    EmptyContent,
    #[error(
        "正文超过限制:{scalars} 标量(上限 {MAX_UNICODE_SCALARS})/{bytes} 字节(上限 {MAX_UTF8_BYTES});不截断、不拆分"
    )]
    ContentTooLong { scalars: usize, bytes: usize },
    #[error("同一范围同一触发同优先级已有启用规则(FR-031)")]
    EnabledConflict,
    #[error("规则不存在")]
    NotFound,
    #[error("该规则被 {0} 条历史交付引用,删除被拒绝(保留审计链)")]
    ReferencedByDeliveries(i64),
    #[error("商品不存在或不属于该账号")]
    ItemNotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    /// 007 US3:请求参数无效(优先级/变体/求评配置/来源绑定互斥等)
    #[error("{0}")]
    Invalid(String),
    /// 007 US3:buyer_reviewed 触发未经适配器能力验证(FR-036)
    #[error("评价事件能力未验证,buyer_reviewed 触发暂不开放")]
    UnsupportedTrigger,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, PartialEq)]
pub struct ContentCheck {
    pub valid: bool,
    pub unicode_scalars: usize,
    pub utf8_bytes: usize,
    pub violations: Vec<String>,
}

/// 产品限制校验(保存与执行前同源;保留换行/链接/中文)。
pub fn check_content(text: &str) -> ContentCheck {
    let scalars = text.chars().count();
    let bytes = text.len();
    let mut violations = Vec::new();
    if text.trim().is_empty() {
        violations.push("正文为空".to_string());
    }
    if scalars > MAX_UNICODE_SCALARS || bytes > MAX_UTF8_BYTES {
        violations.push(format!(
            "正文超限:{scalars} 标量/上限 {MAX_UNICODE_SCALARS},{bytes} 字节/上限 {MAX_UTF8_BYTES}"
        ));
    }
    ContentCheck {
        valid: violations.is_empty(),
        unicode_scalars: scalars,
        utf8_bytes: bytes,
        violations,
    }
}

/// 变体草稿(US3;spec_values 集合以分号拼接存储)。
#[derive(Clone, Debug, PartialEq)]
pub struct VariantDraft {
    pub spec_name: String,
    pub spec_values: Vec<String>,
    pub source: VariantSource,
    pub card_pool_id: Option<String>,
    pub template_id: Option<String>,
    pub template_bindings: Option<TemplateBindings>,
    pub units_per_item: i64,
    pub delay_override_seconds: Option<i64>,
}

/// 规则草稿。001/002 存量语义(fixed_text)经 `Default` 完整保留:
/// `RuleDraft { item_id, sku_key, content, enabled, ..Default::default() }`。
#[derive(Clone, Debug)]
pub struct RuleDraft {
    pub item_id: String,
    /// '' = 商品任意规格(变体规则);账号级见 item_id=''
    pub sku_key: String,
    /// fixed_text 正文;card_pool/template 来源可为空
    pub content: String,
    pub enabled: bool,
    pub trigger_type: TriggerType,
    pub priority: i64,
    pub content_source: ContentSource,
    pub card_pool_id: Option<String>,
    pub template_id: Option<String>,
    pub template_bindings: Option<TemplateBindings>,
    pub variants: Vec<VariantDraft>,
    pub review_config: Option<ReviewConfig>,
    /// 账号级(item_id='')「适用于全部商品」确认;false 允许落库但需确认·暂不发货
    pub all_items_confirmed: bool,
}

impl Default for RuleDraft {
    fn default() -> Self {
        RuleDraft {
            item_id: String::new(),
            sku_key: String::new(),
            content: String::new(),
            enabled: false,
            trigger_type: TriggerType::default(),
            priority: ext::DEFAULT_PRIORITY,
            content_source: ContentSource::FixedText,
            card_pool_id: None,
            template_id: None,
            template_bindings: None,
            variants: Vec::new(),
            review_config: None,
            all_items_confirmed: false,
        }
    }
}

#[derive(Debug)]
pub struct SavedRule {
    pub id: String,
    pub version: i64,
    pub content_version: i64,
}

#[derive(Clone)]
pub struct RuleService {
    db: DbThread,
    key: DataKey,
}

/// 保存侧校验与装配(创建/更新共用):返回规整后的落库参数。
/// 返回 (trigger_str, priority, bindings_json, variants, review_json, refs_ok)。
fn assemble(draft: &RuleDraft) -> Result<AssembledRule, RuleError> {
    // 触发门禁:buyer_reviewed 仅在适配器声明并验证评价事件能力后开放(FR-036;
    // 当前能力集未声明 → 一律 422 unsupported_capability)
    if draft.trigger_type == TriggerType::BuyerReviewed {
        return Err(RuleError::UnsupportedTrigger);
    }
    ext::validate_priority(draft.priority).map_err(|e| RuleError::Invalid(e.to_string()))?;
    // 内容来源绑定互斥:card_pool_id/template_id 只能出现与来源一致的一个;
    // 带变体的规则是"容器"(来源在各变体),规则级绑定必须留空
    let pool = draft
        .card_pool_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let template = draft
        .template_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let has_variants = !draft.variants.is_empty();
    match draft.content_source {
        ContentSource::FixedText => {
            if pool.is_some() || template.is_some() {
                return Err(RuleError::Invalid(
                    "固定内容来源不能绑定卡密组或模板".into(),
                ));
            }
        }
        ContentSource::CardPool => {
            if pool.is_none() && !has_variants {
                return Err(RuleError::Invalid(
                    "卡密组来源必须指定 card_pool_id(或提供变体)".into(),
                ));
            }
            if template.is_some() || draft.template_bindings.is_some() {
                return Err(RuleError::Invalid(
                    "卡密组来源不能同时绑定模板(来源互斥)".into(),
                ));
            }
        }
        ContentSource::Template => {
            if template.is_none() && !has_variants {
                return Err(RuleError::Invalid(
                    "模板来源必须指定 template_id(或提供变体)".into(),
                ));
            }
            if pool.is_some() {
                return Err(RuleError::Invalid(
                    "模板来源不能同时绑定卡密组(来源互斥)".into(),
                ));
            }
        }
    }
    // 变体校验:来源一致、份数/延时边界、规格形态(全空=兜底变体)
    let mut variants = Vec::with_capacity(draft.variants.len());
    for (i, v) in draft.variants.iter().enumerate() {
        let idx = i + 1;
        ext::validate_units_per_item(v.units_per_item)
            .map_err(|e| RuleError::Invalid(format!("第 {idx} 个变体:{}", e)))?;
        ext::validate_delay_override(v.delay_override_seconds)
            .map_err(|e| RuleError::Invalid(format!("第 {idx} 个变体:{}", e)))?;
        let has_name = !v.spec_name.trim().is_empty();
        let values: Vec<String> = v
            .spec_values
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if has_name != !values.is_empty() {
            return Err(RuleError::Invalid(format!(
                "第 {idx} 个变体的规格名与规格值须成对出现(或全空=兜底变体)"
            )));
        }
        let vpool = v
            .card_pool_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let vtpl = v
            .template_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match v.source {
            VariantSource::CardPool => {
                if vpool.is_none() {
                    return Err(RuleError::Invalid(format!(
                        "第 {idx} 个变体(卡密组来源)须指定 card_pool_id"
                    )));
                }
                if vtpl.is_some() {
                    return Err(RuleError::Invalid(format!(
                        "第 {idx} 个变体来源互斥:卡密组不能绑模板"
                    )));
                }
            }
            VariantSource::Template => {
                if vtpl.is_none() {
                    return Err(RuleError::Invalid(format!(
                        "第 {idx} 个变体(模板来源)须指定 template_id"
                    )));
                }
                if vpool.is_some() {
                    return Err(RuleError::Invalid(format!(
                        "第 {idx} 个变体来源互斥:模板不能绑卡密组"
                    )));
                }
            }
        }
        variants.push(NewVariantOwned {
            spec_name: v.spec_name.trim().to_string(),
            spec_values: values,
            source: v.source,
            card_pool_id: vpool.map(str::to_string),
            template_id: vtpl.map(str::to_string),
            template_bindings: v
                .template_bindings
                .as_ref()
                .map(|b| serde_json::to_string(b).expect("模板绑定序列化")),
            units_per_item: v.units_per_item,
            delay_override_seconds: v.delay_override_seconds,
        });
    }
    // 求评配置:仅超时未评价触发可携带(FR-035)
    let review_json = match &draft.review_config {
        Some(cfg) => {
            if draft.trigger_type != TriggerType::ReviewMissingTimeout {
                return Err(RuleError::Invalid(
                    "求评配置仅适用于「超时未评价」触发类型".into(),
                ));
            }
            ext::validate_review_config(cfg).map_err(|e| RuleError::Invalid(e.to_string()))?;
            Some(serde_json::to_string(cfg).expect("求评配置序列化"))
        }
        None => None,
    };
    let bindings_json = match (&draft.content_source, &draft.template_bindings) {
        (ContentSource::Template, Some(b)) => {
            Some(serde_json::to_string(b).expect("模板绑定序列化"))
        }
        _ => None,
    };
    Ok(AssembledRule {
        trigger: draft.trigger_type.as_str().to_string(),
        priority: draft.priority,
        pool_id: pool.map(str::to_string),
        template_id: template.map(str::to_string),
        bindings_json,
        variants,
        review_json,
    })
}

struct NewVariantOwned {
    spec_name: String,
    spec_values: Vec<String>,
    source: VariantSource,
    card_pool_id: Option<String>,
    template_id: Option<String>,
    template_bindings: Option<String>,
    units_per_item: i64,
    delay_override_seconds: Option<i64>,
}

struct AssembledRule {
    trigger: String,
    priority: i64,
    pool_id: Option<String>,
    template_id: Option<String>,
    bindings_json: Option<String>,
    variants: Vec<NewVariantOwned>,
    review_json: Option<String>,
}

/// 落库前的引用完整性核验材料:全部被引用的卡组/模板 id(含变体)。
fn referenced_ids(assembled: &AssembledRule) -> (Vec<String>, Vec<String>) {
    let mut pools: Vec<String> = assembled.pool_id.iter().cloned().collect();
    let mut tpls: Vec<String> = assembled.template_id.iter().cloned().collect();
    for v in &assembled.variants {
        if let Some(p) = &v.card_pool_id {
            pools.push(p.clone());
        }
        if let Some(t) = &v.template_id {
            tpls.push(t.clone());
        }
    }
    (pools, tpls)
}

impl RuleService {
    pub fn new(db: DbThread, key: DataKey) -> Self {
        Self { db, key }
    }

    /// 预览:纯校验,不保存、不发平台请求(US3-2)。
    pub fn preview(&self, text: &str) -> ContentCheck {
        check_content(text)
    }

    /// 创建规则(不带 expected_version);同范围同触发同优先级启用冲突
    /// → EnabledConflict(422 rule_conflict;唯一索引兜底)。
    pub async fn create(&self, account_id: &str, draft: RuleDraft) -> Result<SavedRule, RuleError> {
        // 固定内容来源正文校验(001 行为逐字保留);其他来源正文可空
        if draft.content_source == ContentSource::FixedText {
            let check = check_content(&draft.content);
            if !check.valid {
                return Err(if draft.content.trim().is_empty() {
                    RuleError::EmptyContent
                } else {
                    RuleError::ContentTooLong {
                        scalars: check.unicode_scalars,
                        bytes: check.utf8_bytes,
                    }
                });
            }
        }
        let assembled = assemble(&draft)?;
        let key = self.key.key;
        let key_id = self.key.key_id.clone();
        let account = account_id.to_string();
        let draft_content = draft.content.clone();
        let draft_enabled = draft.enabled;
        let draft_item = draft.item_id.clone();
        let draft_sku = draft.sku_key.clone();
        let confirmed = draft.all_items_confirmed;
        let (pools_ref, tpls_ref) = referenced_ids(&assembled);
        let trigger = assembled.trigger.clone();
        let priority = assembled.priority;

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<SavedRule, RuleError>> {
                    // 账号必须存在(账号级规则的归属前提)
                    if accounts::get(conn, &account)?.is_none() {
                        return Ok(Err(RuleError::ItemNotFound));
                    }
                    // 归属检查:商品必须属于该账号;账号级(item_id='')免检
                    if !draft_item.is_empty() {
                        match items::get(conn, &draft_item)? {
                            Some(item) if item.account_id == account => {}
                            _ => return Ok(Err(RuleError::ItemNotFound)),
                        }
                    }
                    // 同范围同触发同优先级启用冲突(应用层显式检查;索引兜底,D1)
                    if draft_enabled
                        && rules_ext::enabled_same_priority(
                            conn,
                            &account,
                            &draft_item,
                            &draft_sku,
                            &trigger,
                            priority,
                            None,
                        )?
                    {
                        return Ok(Err(RuleError::EnabledConflict));
                    }
                    // 引用完整性:缺失 → 落库但标记需重新配置(执行旁路,不静默失败)
                    let mut refs_ok = true;
                    for pool in &pools_ref {
                        if cards::get_pool(conn, pool)?.is_none() {
                            refs_ok = false;
                        }
                    }
                    for tpl in &tpls_ref {
                        if templates::get_template(conn, tpl)?.is_none() {
                            refs_ok = false;
                        }
                    }
                    // 需重新配置标记仅承载"引用缺失"(FR/edge cases);
                    // 账号级未确认由 all_items_confirmed=false 表达(FR-032:
                    // 需确认·暂不发货),两态在执行侧分别裁决(NeedsConfirmation/
                    // NeedsReconfiguration),提示语各自可操作
                    let needs_flag = !refs_ok;
                    let rule = rules_ext::insert_ext(
                        conn,
                        &rules_ext::NewRuleExt {
                            id: &ids::new_id("rul"),
                            account_id: &account,
                            item_id: &draft_item,
                            sku_key: &draft_sku,
                            enabled: draft_enabled,
                            trigger_type: &trigger,
                            priority,
                            card_pool_id: assembled.pool_id.as_deref(),
                            template_id: assembled.template_id.as_deref(),
                            template_bindings: assembled.bindings_json.as_deref(),
                            review_config: assembled.review_json.as_deref(),
                            all_items_confirmed: confirmed,
                            needs_reconfiguration: needs_flag,
                        },
                    )?;
                    // 内容 v1:固定内容=加密正文;其他来源=原样(可为空,仅占位审计)
                    let digest = hex::encode(Sha256::digest(draft_content.as_bytes()));
                    let envelope = crypto::seal(
                        &key,
                        &key_id,
                        &rules::rule_aad(&rule.id, 1),
                        draft_content.as_bytes(),
                    );
                    rules::insert_content(
                        conn,
                        &ids::new_id("rc"),
                        &rule.id,
                        1,
                        &envelope,
                        &digest,
                        draft_content.chars().count() as i64,
                        draft_content.len() as i64,
                    )?;
                    // 变体整体替换语义(首存即全量)
                    let variant_ids: Vec<String> = assembled
                        .variants
                        .iter()
                        .map(|_| ids::new_id("rv"))
                        .collect();
                    let new_variants: Vec<rules_ext::NewVariant<'_>> = assembled
                        .variants
                        .iter()
                        .zip(variant_ids.iter())
                        .map(|(v, vid)| rules_ext::NewVariant {
                            id: vid.as_str(),
                            spec_name: &v.spec_name,
                            spec_values: &v.spec_values,
                            source: v.source,
                            card_pool_id: v.card_pool_id.as_deref(),
                            template_id: v.template_id.as_deref(),
                            template_bindings: v.template_bindings.as_deref(),
                            units_per_item: v.units_per_item,
                            delay_override_seconds: v.delay_override_seconds,
                        })
                        .collect();
                    rules_ext::replace_variants(conn, &rule.id, &new_variants)?;
                    let fresh =
                        rules::get(conn, &rule.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(SavedRule {
                        id: rule.id,
                        version: fresh.version,
                        content_version: 1,
                    }))
                },
            )
            .await??
    }

    /// 版本化全量更新(US3):范围(item/sku)不可变,其余字段整体替换;
    /// 引用修复/确认后 needs_reconfiguration 清除(保存即修复)。
    pub async fn update_full(
        &self,
        account_id: &str,
        rule_id: &str,
        expected_version: i64,
        draft: RuleDraft,
    ) -> Result<SavedRule, RuleError> {
        if draft.content_source == ContentSource::FixedText {
            let check = check_content(&draft.content);
            if !check.valid {
                return Err(if draft.content.trim().is_empty() {
                    RuleError::EmptyContent
                } else {
                    RuleError::ContentTooLong {
                        scalars: check.unicode_scalars,
                        bytes: check.utf8_bytes,
                    }
                });
            }
        }
        let assembled = assemble(&draft)?;
        let key = self.key.key;
        let key_id = self.key.key_id.clone();
        let account = account_id.to_string();
        let rule_owned = rule_id.to_string();
        let content = draft.content.clone();
        let enabled = draft.enabled;
        let confirmed = draft.all_items_confirmed;
        let (pools_ref, tpls_ref) = referenced_ids(&assembled);
        let trigger = assembled.trigger.clone();
        let priority = assembled.priority;

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<SavedRule, RuleError>> {
                    // rules_ext::get_rule:账号级哨兵 item_id 翻译回 ''(语义一致比较)
                    let Some(rule) = rules_ext::get_rule(conn, &rule_owned)? else {
                        return Ok(Err(RuleError::NotFound));
                    };
                    if rule.account_id != account {
                        return Ok(Err(RuleError::NotFound));
                    }
                    if rule.version != expected_version {
                        return Ok(Err(RuleError::VersionConflict));
                    }
                    // 范围不可变(匹配层级与既有快照绑定依赖范围稳定)
                    if rule.item_id != draft.item_id || rule.sku_key != draft.sku_key {
                        return Ok(Err(RuleError::Invalid(
                            "规则范围(商品/规格)不可修改,请删除后新建".into(),
                        )));
                    }
                    if enabled
                        && rules_ext::enabled_same_priority(
                            conn,
                            &account,
                            &rule.item_id,
                            &rule.sku_key,
                            &trigger,
                            priority,
                            Some(&rule_owned),
                        )?
                    {
                        return Ok(Err(RuleError::EnabledConflict));
                    }
                    // 引用完整性与确认门禁(保存即修复:齐全 → 清除标记)
                    let mut refs_ok = true;
                    for pool in &pools_ref {
                        if cards::get_pool(conn, pool)?.is_none() {
                            refs_ok = false;
                        }
                    }
                    for tpl in &tpls_ref {
                        if templates::get_template(conn, tpl)?.is_none() {
                            refs_ok = false;
                        }
                    }
                    // 同 create:needs_reconfiguration 仅承载引用缺失;未确认走
                    // all_items_confirmed(保存确认即修复,标记随之清除)
                    let needs_flag = !refs_ok;
                    let updated = rules_ext::update_ext(
                        conn,
                        &rule_owned,
                        expected_version,
                        enabled,
                        &trigger,
                        priority,
                        assembled.pool_id.as_deref(),
                        assembled.template_id.as_deref(),
                        assembled.bindings_json.as_deref(),
                        assembled.review_json.as_deref(),
                        confirmed,
                        needs_flag,
                    )?;
                    let Some(updated) = updated else {
                        return Ok(Err(RuleError::VersionConflict));
                    };
                    // 内容版本化追加:固定内容校验过;其他来源非空才追加(空=保持现版)
                    let mut content_version = updated.current_content_version;
                    if !content.is_empty() && content_version > 0 {
                        let current = rules::get_content(conn, &rule_owned, content_version)?;
                        let unchanged = current
                            .map(|c| c.text_digest == hex::encode(Sha256::digest(content.as_bytes())))
                            .unwrap_or(false);
                        if !unchanged {
                            content_version += 1;
                            let digest = hex::encode(Sha256::digest(content.as_bytes()));
                            let envelope = crypto::seal(
                                &key,
                                &key_id,
                                &rules::rule_aad(&rule_owned, content_version),
                                content.as_bytes(),
                            );
                            rules::insert_content(
                                conn,
                                &ids::new_id("rc"),
                                &rule_owned,
                                content_version,
                                &envelope,
                                &digest,
                                content.chars().count() as i64,
                                content.len() as i64,
                            )?;
                        }
                    }
                    let variant_ids: Vec<String> = assembled
                        .variants
                        .iter()
                        .map(|_| ids::new_id("rv"))
                        .collect();
                    let new_variants: Vec<rules_ext::NewVariant<'_>> = assembled
                        .variants
                        .iter()
                        .zip(variant_ids.iter())
                        .map(|(v, vid)| rules_ext::NewVariant {
                            id: vid.as_str(),
                            spec_name: &v.spec_name,
                            spec_values: &v.spec_values,
                            source: v.source,
                            card_pool_id: v.card_pool_id.as_deref(),
                            template_id: v.template_id.as_deref(),
                            template_bindings: v.template_bindings.as_deref(),
                            units_per_item: v.units_per_item,
                            delay_override_seconds: v.delay_override_seconds,
                        })
                        .collect();
                    rules_ext::replace_variants(conn, &rule_owned, &new_variants)?;
                    let fresh =
                        rules::get(conn, &rule_owned)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(SavedRule {
                        id: fresh.id,
                        version: fresh.version,
                        content_version: fresh.current_content_version,
                    }))
                },
            )
            .await??
    }

    /// 版本化更新(编辑/禁用/启用均走此处);expected_version 必填。
    /// 001 兼容入口:仅内容与启停;US3 扩展字段保持原值。
    pub async fn update(
        &self,
        account_id: &str,
        rule_id: &str,
        expected_version: i64,
        new_content: Option<String>,
        enabled: Option<bool>,
    ) -> Result<SavedRule, RuleError> {
        let key = self.key.key;
        let key_id = self.key.key_id.clone();
        let account = account_id.to_string();
        let rule_owned = rule_id.to_string();

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<SavedRule, RuleError>> {
                    let Some(rule) = rules::get(conn, &rule_owned)? else {
                        return Ok(Err(RuleError::NotFound));
                    };
                    if rule.account_id != account {
                        return Ok(Err(RuleError::NotFound));
                    }
                    if rule.version != expected_version {
                        return Ok(Err(RuleError::VersionConflict));
                    }
                    // 启用冲突:同范围同触发同优先级(D1 收窄语义;排除自身)
                    if enabled == Some(true)
                        && !rule.enabled
                        && rules_ext::enabled_same_priority(
                            conn,
                            &account,
                            &rule.item_id,
                            &rule.sku_key,
                            &rule.trigger_type,
                            rule.priority,
                            Some(&rule_owned),
                        )?
                    {
                        return Ok(Err(RuleError::EnabledConflict));
                    }
                    // 内容更新 = 新的不可变版本;旧版本与已冻结快照不受影响(FR-009)
                    let mut content_version = rule.current_content_version;
                    if let Some(text) = &new_content {
                        let check = check_content(text);
                        if !check.valid {
                            return Ok(Err(if text.trim().is_empty() {
                                RuleError::EmptyContent
                            } else {
                                RuleError::ContentTooLong {
                                    scalars: check.unicode_scalars,
                                    bytes: check.utf8_bytes,
                                }
                            }));
                        }
                        content_version += 1;
                        let digest = hex::encode(Sha256::digest(text.as_bytes()));
                        let envelope = crypto::seal(
                            &key,
                            &key_id,
                            &rules::rule_aad(&rule.id, content_version),
                            text.as_bytes(),
                        );
                        rules::insert_content(
                            conn,
                            &ids::new_id("rc"),
                            &rule.id,
                            content_version,
                            &envelope,
                            &digest,
                            text.chars().count() as i64,
                            text.len() as i64,
                        )?;
                    }
                    if let Some(enable) = enabled {
                        conn.execute(
                            "UPDATE rules SET enabled = ?2, version = version + 1, updated_at = ?3
                         WHERE id = ?1",
                            rusqlite::params![rule_owned, enable as i64, utc_now_ms()],
                        )?;
                    }
                    let fresh = rules::get(conn, &rule_owned)?
                        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(SavedRule {
                        id: fresh.id,
                        version: fresh.version,
                        content_version: fresh.current_content_version,
                    }))
                },
            )
            .await??
    }

    /// 匹配预览(T059 + US3 T034 同源裁决):确定性匹配,不发送正文。
    /// 优先级模型下同范围多条启用不再视为歧义(取层级+优先级最高一条);
    /// 需确认/需重新配置的规则同样算「已命中」(执行门禁在交付侧)。
    pub async fn match_preview(
        &self,
        account_id: &str,
        item_id: &str,
        sku_key: &str,
    ) -> Result<MatchPreview, RuleError> {
        let account = account_id.to_string();
        let item = item_id.to_string();
        let sku = sku_key.to_string();
        let result = self
            .db
            .call(move |conn| -> rusqlite::Result<MatchPreview> {
                let rows =
                    rules_ext::find_order_paid_candidates(conn, &account, Some(&item), &sku)?;
                let mut candidates = Vec::with_capacity(rows.len());
                let mut versions = std::collections::HashMap::new();
                for row in rows {
                    let variants = rules_ext::list_variants(conn, &row.id)?;
                    versions.insert(row.id.clone(), row.version);
                    candidates.push(rules_ext::to_candidate(&row, &variants));
                }
                let scope = ext::MatchScope {
                    item_id: &item,
                    sku_key: &sku,
                    sku_parts: &[],
                };
                match ext::select(&scope, &candidates) {
                    ext::Decision::Execute { rule_id, .. }
                    | ext::Decision::NeedsConfirmation { rule_id }
                    | ext::Decision::NeedsReconfiguration { rule_id } => {
                        Ok(MatchPreview::Matched {
                            rule_version: versions.get(&rule_id).copied().unwrap_or(0),
                            rule_id,
                        })
                    }
                    ext::Decision::Ambiguous { .. } => Ok(MatchPreview::Ambiguous),
                    ext::Decision::NoRule => Ok(MatchPreview::None),
                }
            })
            .await??;
        Ok(result)
    }
}

#[derive(Debug, PartialEq)]
pub enum MatchPreview {
    Matched { rule_id: String, rule_version: i64 },
    None,
    Ambiguous,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;
    use crate::adapters::sqlite::repos::accounts;
    use crate::domain::sku::{SkuPart, combo_key};

    async fn setup() -> (tempfile::TempDir, RuleService, DbThread) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir =
            crate::adapters::windows::datadir::DataDir::resolve(Some(dir.path())).unwrap();
        let key = crate::adapters::windows::keys::load_or_create(&data_dir.root).unwrap();
        let db = DbThread::spawn(&data_dir.join("rules.db")).unwrap();
        let dir_path = data_dir.root.clone();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        db.call(|conn| {
            accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
            items::upsert(
                conn, "item-1", "acct-1", "EXT-1", "资料", "on_sale", "[]", "single",
            )
        })
        .await
        .unwrap()
        .unwrap();
        (dir, RuleService::new(db.clone(), key), db)
    }

    fn text_ok() -> String {
        "链接 https://example.com/d\n提取码 ab12".to_string()
    }

    #[tokio::test]
    async fn content_limits_enforced_verbatim() {
        let (_d, svc, _db) = setup().await;
        // 空正文拒绝
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: "  ".into(),
                    enabled: false,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::EmptyContent)
        ));
        // 超长拒绝:1001 个标量
        let too_long = "字".repeat(MAX_UNICODE_SCALARS + 1);
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: too_long,
                    enabled: false,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::ContentTooLong { .. })
        ));
        // 字节数超限但标量不超(ASCII 4001)
        let too_many_bytes = "a".repeat(MAX_UTF8_BYTES + 1);
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: too_many_bytes,
                    enabled: false,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::ContentTooLong { .. })
        ));
        // 预览同源校验
        let p = svc.preview(&"a".repeat(MAX_UTF8_BYTES + 1));
        assert!(!p.valid);
    }

    #[tokio::test]
    async fn enabled_scope_uniqueness_and_version_update() {
        let (_d, svc, db) = setup().await;
        let saved = svc
            .create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 同范围同优先级第二条启用 → 冲突(默认优先级同为 100)
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::EnabledConflict)
        ));
        // 禁用的第二条允许
        let second = svc
            .create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 内容更新 → 版本 2,旧版本保留
        let updated = svc
            .update(
                "acct-1",
                &saved.id,
                saved.version,
                Some("新内容 v2".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.content_version, 2);
        let old_probe = saved.id.clone();
        let old = db
            .call(move |conn| rules::get_content(conn, &old_probe, 1))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(old.content_version, 1, "v1 is retained immutably");

        // 禁用后第二条可启用
        let saved_id = saved.id.clone();
        svc.update("acct-1", &saved_id, updated.version, None, Some(false))
            .await
            .unwrap();
        svc.update("acct-1", &second.id, second.version, None, Some(true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn match_preview_exact() {
        let (_d, svc, _db) = setup().await;
        let combo = combo_key(&[SkuPart {
            property_id: "p1".into(),
            value_id: "v1".into(),
            property_label: String::new(),
            value_label: String::new(),
        }])
        .unwrap();
        assert_eq!(
            svc.match_preview("acct-1", "item-1", &combo).await.unwrap(),
            MatchPreview::None
        );
        svc.create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: combo.clone(),
                content: text_ok(),
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            svc.match_preview("acct-1", "item-1", &combo).await.unwrap(),
            MatchPreview::Matched { .. }
        ));
        // 其他组合不命中(不做子串/部分匹配)
        let other = combo_key(&[SkuPart {
            property_id: "p1".into(),
            value_id: "v2".into(),
            property_label: String::new(),
            value_label: String::new(),
        }])
        .unwrap();
        assert_eq!(
            svc.match_preview("acct-1", "item-1", &other).await.unwrap(),
            MatchPreview::None
        );
    }

    /// US3:同优先级冲突按 D1 收窄;不同优先级允许并存。
    #[tokio::test]
    async fn same_priority_conflict_narrowed_to_same_priority() {
        let (_d, svc, _db) = setup().await;
        svc.create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: text_ok(),
                enabled: true,
                priority: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // 不同优先级:允许并存(多规格/多条并存模型)
        svc.create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: "single".into(),
                content: text_ok(),
                enabled: true,
                priority: 200,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // 同优先级:拒绝
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                    priority: 200,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::EnabledConflict)
        ));
    }

    /// US3:buyer_reviewed 能力门禁与账号级确认语义。
    #[tokio::test]
    async fn buyer_reviewed_gated_and_account_confirm_semantics() {
        let (_d, svc, _db) = setup().await;
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: false,
                    trigger_type: TriggerType::BuyerReviewed,
                    ..Default::default()
                }
            )
            .await,
            Err(RuleError::UnsupportedTrigger)
        ));
        // 账号级(item_id='')免商品归属检查;未确认允许落库
        let saved = svc
            .create(
                "acct-1",
                RuleDraft {
                    item_id: String::new(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                    all_items_confirmed: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let id = saved.id.clone();
        let (confirmed_stored, needs_reconf) = svc_match_flags(&svc, &id).await;
        assert!(!confirmed_stored, "all_items_confirmed 如实保存为 false");
        assert!(
            !needs_reconf,
            "未确认不折叠进 needs_reconfiguration(引用缺失专用);需确认由确认标记表达"
        );
    }

    async fn svc_match_flags(svc: &RuleService, rule_id: &str) -> (bool, bool) {
        svc.db
            .call({
                let id = rule_id.to_string();
                move |conn| {
                    let row = rules_ext::get_rule(conn, &id)?.unwrap();
                    Ok::<_, rusqlite::Error>((
                        row.all_items_confirmed,
                        row.needs_reconfiguration,
                    ))
                }
            })
            .await
            .unwrap()
            .unwrap()
    }
}

impl RuleService {
    /// 删除规则(007 US3:补齐 contracts §3 遗漏的 DELETE 端点用例)。
    /// 被交付历史引用时拒绝,保留审计链;乐观锁 expected_version。
    pub async fn delete(
        &self,
        account_id: &str,
        rule_id: &str,
        expected_version: i64,
    ) -> Result<(), RuleError> {
        let account = account_id.to_string();
        let rule = rule_id.to_string();
        let outcome = self
            .db
            .call(
                move |conn| -> rusqlite::Result<rules::RuleDeleteOutcome> {
                    rules::delete(conn, &account, &rule, expected_version)
                },
            )
            .await
            .map_err(RuleError::from)??;
        match outcome {
            rules::RuleDeleteOutcome::Deleted => Ok(()),
            rules::RuleDeleteOutcome::NotFound => Err(RuleError::NotFound),
            rules::RuleDeleteOutcome::VersionConflict => Err(RuleError::VersionConflict),
            rules::RuleDeleteOutcome::ReferencedByDeliveries(n) => {
                Err(RuleError::ReferencedByDeliveries(n))
            }
        }
    }
}
