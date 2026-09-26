//! 007 US3 规则扩展仓储(T032):rule_variants(随规则保存整体替换)、
//! rules 匹配查询改造(order_paid 候选:商品精确/任意规格/账号级回退)、
//! 列表过滤(trigger_type/enabled/search + 分页)与 trigger_counts 汇总、
//! reply_rules + reply_rule_items(关键词回复,无关联行=账号级)、
//! default_replies + default_reply_log、review_reminder_state。
//! SQL 只在本目录(宪章 II);裁决逻辑在 domain::rules_ext。

use rusqlite::{Connection, OptionalExtension, params};

use crate::adapters::sqlite::repos::rules::RuleRow;
use crate::domain::time_util::{format_rfc3339, utc_now_ms};

fn now_rfc3339() -> String {
    format_rfc3339(utc_now_ms())
}

// ---------- 账号级范围哨兵(零迁移适配) ----------
// 0001 的 rules.item_id 带 REFERENCES items(id) 外键(FK 常开),0004 未重建该表,
// item_id='' 无法直接落库;迁移一经提交不得修改。故账号级规则在存储层指向
// 每账号一条哨兵 items 行(external_item_id 固定),仓储读写双向翻译:
// 对外(domain/HTTP)保持 data-model 约定的 item_id='' 哨兵语义。

/// 哨兵商品的平台外部 ID(真实平台 ID 不会以此开头;商品列表过滤同值)
pub const ACCOUNT_SCOPE_EXTERNAL_ID: &str = "__account_scope__";

fn account_scope_item_id(account_id: &str) -> String {
    format!("scope:{account_id}")
}

/// 确保账号级哨兵 items 行存在(FK 依赖;幂等)。
fn ensure_account_scope_item(conn: &Connection, account_id: &str) -> rusqlite::Result<String> {
    let sentinel = account_scope_item_id(account_id);
    let now = utc_now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO items(id, account_id, external_item_id, title, listing_state,
             sku_definition, sku_completeness, created_at, updated_at)
         VALUES (?1, ?2, ?3, '(账号级·全部商品)', 'on_sale', '[]', 'incomplete', ?4, ?4)",
        params![sentinel, account_id, ACCOUNT_SCOPE_EXTERNAL_ID, now],
    )?;
    Ok(sentinel)
}

/// 存储 item_id(哨兵)→ 语义 item_id('' = 账号级)。
fn translate_scope_id(row_item_id: String, account_id: &str) -> String {
    if row_item_id == account_scope_item_id(account_id) {
        String::new()
    } else {
        row_item_id
    }
}

// ---------- rules 扩展写入与匹配查询 ----------

/// 创建带 US3 扩展列的规则(T033 保存路径;rules::insert 保留给固定内容旧调用方)。
pub struct NewRuleExt<'a> {
    pub id: &'a str,
    pub account_id: &'a str,
    pub item_id: &'a str,
    pub sku_key: &'a str,
    pub enabled: bool,
    pub trigger_type: &'a str,
    pub priority: i64,
    pub card_pool_id: Option<&'a str>,
    pub template_id: Option<&'a str>,
    pub template_bindings: Option<&'a str>,
    pub review_config: Option<&'a str>,
    pub all_items_confirmed: bool,
    pub needs_reconfiguration: bool,
}

pub fn insert_ext(conn: &Connection, n: &NewRuleExt<'_>) -> rusqlite::Result<RuleRow> {
    let now = now_rfc3339();
    // item_id='' = 账号级 → 存储指向哨兵 items 行(零迁移 FK 适配)
    let item_id_storage = if n.item_id.is_empty() {
        ensure_account_scope_item(conn, n.account_id)?
    } else {
        n.item_id.to_string()
    };
    conn.execute(
        "INSERT INTO rules(id, account_id, item_id, sku_key, enabled, trigger_type, priority,
             all_items_confirmed, needs_reconfiguration, card_pool_id, template_id,
             template_bindings, review_config, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        params![
            n.id,
            n.account_id,
            item_id_storage,
            n.sku_key,
            n.enabled as i64,
            n.trigger_type,
            n.priority,
            n.all_items_confirmed as i64,
            n.needs_reconfiguration as i64,
            n.card_pool_id,
            n.template_id,
            n.template_bindings,
            n.review_config,
            now,
        ],
    )?;
    get_rule(conn, n.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// 规则读取(rules::get 的 US3 包装:哨兵 item_id 翻译回 '' 账号级语义)。
pub fn get_rule(conn: &Connection, id: &str) -> rusqlite::Result<Option<RuleRow>> {
    let row = crate::adapters::sqlite::repos::rules::get(conn, id)?;
    Ok(row.map(|mut r| {
        r.item_id = translate_scope_id(r.item_id.clone(), &r.account_id);
        r
    }))
}

/// 版本化更新规则元数据(US3 扩展字段整体替换 + enabled);版本不匹配返回 None。
#[allow(clippy::too_many_arguments)]
pub fn update_ext(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    enabled: bool,
    trigger_type: &str,
    priority: i64,
    card_pool_id: Option<&str>,
    template_id: Option<&str>,
    template_bindings: Option<&str>,
    review_config: Option<&str>,
    all_items_confirmed: bool,
    needs_reconfiguration: bool,
) -> rusqlite::Result<Option<RuleRow>> {
    let n = conn.execute(
        "UPDATE rules SET enabled = ?2, trigger_type = ?3, priority = ?4,
                card_pool_id = ?5, template_id = ?6, template_bindings = ?7,
                review_config = ?8, all_items_confirmed = ?9, needs_reconfiguration = ?10,
                version = version + 1, updated_at = ?11
         WHERE id = ?1 AND version = ?12",
        params![
            id,
            enabled as i64,
            trigger_type,
            priority,
            card_pool_id,
            template_id,
            template_bindings,
            review_config,
            all_items_confirmed as i64,
            needs_reconfiguration as i64,
            now_rfc3339(),
            expected_version,
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get_rule(conn, id)
}

/// 同范围同触发同优先级冲突检查(T033 保存侧显式检查;唯一索引兜底)。
/// exclude = 更新场景排除自身;item_id='' 按账号级哨兵存储翻译。
pub fn enabled_same_priority(
    conn: &Connection,
    account_id: &str,
    item_id: &str,
    sku_key: &str,
    trigger_type: &str,
    priority: i64,
    exclude: Option<&str>,
) -> rusqlite::Result<bool> {
    let item_storage = if item_id.is_empty() {
        account_scope_item_id(account_id)
    } else {
        item_id.to_string()
    };
    conn.query_row(
        "SELECT COUNT(*) FROM rules
         WHERE account_id = ?1 AND item_id = ?2 AND sku_key = ?3 AND trigger_type = ?4
           AND priority = ?5 AND enabled = 1 AND id != ?6",
        params![
            account_id,
            item_storage,
            sku_key,
            trigger_type,
            priority,
            exclude.unwrap_or(""),
        ],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
}

/// order_paid 执行候选(T034 匹配查询):trigger_type='order_paid' + enabled=1,
/// 范围 = 商品精确规格 ∪ 商品任意规格(sku_key='') ∪ 账号级(存储哨兵,读出为 '')。
/// 层级/优先级裁决由 domain::rules_ext::select 完成(此处仅收窄候选集)。
/// item = None(订单商品未入库)时仅返回账号级候选。
pub fn find_order_paid_candidates(
    conn: &Connection,
    account_id: &str,
    item_id: Option<&str>,
    sku_key: &str,
) -> rusqlite::Result<Vec<RuleRow>> {
    let sentinel = account_scope_item_id(account_id);
    let mut sql = String::from(
        "SELECT rules.id, rules.account_id,
                CASE WHEN rules.item_id = ?4 THEN '' ELSE rules.item_id END,
                rules.sku_key, rules.enabled,
                rules.current_content_version, rules.version, rules.card_pool_id,
                rules.template_id, rules.template_bindings, rules.trigger_type, rules.priority,
                rules.all_items_confirmed, rules.needs_reconfiguration, rules.review_config
         FROM rules WHERE rules.account_id = ?1 AND rules.trigger_type = 'order_paid'
           AND rules.enabled = 1",
    );
    match item_id {
        Some(_) => {
            sql.push_str(
                " AND ((rules.item_id = ?2 AND (rules.sku_key = ?3 OR rules.sku_key = ''))
                    OR (rules.item_id = ?4 AND (rules.sku_key = ?3 OR rules.sku_key = '')))",
            );
        }
        None => {
            sql.push_str(
                " AND rules.item_id = ?4 AND (rules.sku_key = ?3 OR rules.sku_key = '')",
            );
        }
    }
    sql.push_str(" ORDER BY rules.priority ASC, rules.created_at ASC, rules.id ASC");
    let mut stmt = conn.prepare(&sql)?;
    let item_bind = item_id.unwrap_or("");
    let rows = stmt
        .query_map(params![account_id, item_bind, sku_key, sentinel], ext_rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// rules 查询行映射(find_order_paid_candidates/list_rules_filtered 共用;列序同 SQL)。
fn ext_rule_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RuleRow> {
    Ok(RuleRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        item_id: r.get(2)?,
        sku_key: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        current_content_version: r.get(5)?,
        version: r.get(6)?,
        card_pool_id: r.get(7)?,
        template_id: r.get(8)?,
        template_bindings: r.get(9)?,
        trigger_type: r.get(10)?,
        priority: r.get(11)?,
        all_items_confirmed: r.get::<_, i64>(12)? != 0,
        needs_reconfiguration: r.get::<_, i64>(13)? != 0,
        review_config: r.get(14)?,
    })
}

/// 账号规则列表(FR-038):trigger_type/enabled 过滤 + search(商品标题/规格键包含)
/// + limit 分页;优先级升序便于运营核对执行序。
pub fn list_rules_filtered(
    conn: &Connection,
    account_id: &str,
    trigger_type: Option<&str>,
    enabled: Option<bool>,
    search: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<RuleRow>> {
    let mut sql = String::from(
        "SELECT rules.id, rules.account_id, rules.item_id, rules.sku_key, rules.enabled,
                rules.current_content_version, rules.version, rules.card_pool_id,
                rules.template_id, rules.template_bindings, rules.trigger_type, rules.priority,
                rules.all_items_confirmed, rules.needs_reconfiguration, rules.review_config
         FROM rules LEFT JOIN items ON rules.item_id = items.id
         WHERE rules.account_id = ?1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(account_id.to_string())];
    if let Some(tt) = trigger_type {
        sql.push_str(" AND rules.trigger_type = ?");
        binds.push(Box::new(tt.to_string()));
    }
    if let Some(e) = enabled {
        sql.push_str(" AND rules.enabled = ?");
        binds.push(Box::new(e as i64));
    }
    if let Some(s) = search.filter(|s| !s.is_empty()) {
        sql.push_str(
            " AND (items.title LIKE '%' || ? || '%' ESCAPE '\\' OR rules.sku_key LIKE '%' || ? || '%' ESCAPE '\\')",
        );
        let escaped = like_escape(s);
        binds.push(Box::new(escaped.clone()));
        binds.push(Box::new(escaped));
    }
    sql.push_str(" ORDER BY rules.priority ASC, rules.created_at ASC, rules.id ASC LIMIT ?");
    binds.push(Box::new(limit));
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), ext_rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // 账号级哨兵 item_id 翻译回 ''(对外语义)
    let mut rows = rows;
    for row in &mut rows {
        row.item_id = translate_scope_id(row.item_id.clone(), &row.account_id);
    }
    Ok(rows)
}

/// LIKE 通配符转义(与 templates.rs 同口径:搜索只按字面包含)。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 触发类型计数汇总(contracts §3 trigger_counts)。
pub fn trigger_counts(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT trigger_type, COUNT(*) FROM rules WHERE account_id = ?1 GROUP BY trigger_type",
    )?;
    let rows = stmt
        .query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 仓储行 → 领域匹配候选(裁决在 domain::rules_ext::select,delivery/catalog 共用)。
pub fn to_candidate(
    rule: &RuleRow,
    variants: &[VariantRow],
) -> crate::domain::rules_ext::RuleCandidate {
    crate::domain::rules_ext::RuleCandidate {
        id: rule.id.clone(),
        item_id: rule.item_id.clone(),
        sku_key: rule.sku_key.clone(),
        priority: rule.priority,
        all_items_confirmed: rule.all_items_confirmed,
        needs_reconfiguration: rule.needs_reconfiguration,
        variants: variants
            .iter()
            .map(|v| crate::domain::rules_ext::RuleVariant {
                spec_name: v.spec_name.clone(),
                spec_values: v.spec_values.clone(),
                source: v.source,
                card_pool_id: v.card_pool_id.clone(),
                template_id: v.template_id.clone(),
                template_bindings: v.template_bindings.clone(),
                units_per_item: v.units_per_item,
                delay_override_seconds: v.delay_override_seconds,
            })
            .collect(),
    }
}

// ---------- rule_variants ----------

pub struct VariantRow {
    pub id: String,
    pub rule_id: String,
    pub spec_name: String,
    /// 分号分隔值集合(读侧拆 Vec)
    pub spec_values: Vec<String>,
    pub source: crate::domain::rules_ext::VariantSource,
    pub card_pool_id: Option<String>,
    pub template_id: Option<String>,
    pub template_bindings: Option<String>,
    pub units_per_item: i64,
    pub delay_override_seconds: Option<i64>,
    pub position: i64,
}

/// 新变体行(spec_values 以分号拼接存储;position 由插入顺序决定)。
pub struct NewVariant<'a> {
    pub id: &'a str,
    pub spec_name: &'a str,
    pub spec_values: &'a [String],
    pub source: crate::domain::rules_ext::VariantSource,
    pub card_pool_id: Option<&'a str>,
    pub template_id: Option<&'a str>,
    pub template_bindings: Option<&'a str>,
    pub units_per_item: i64,
    pub delay_override_seconds: Option<i64>,
}

/// 变体整体替换(保存语义:删旧插新;调用方与规则行更新同事务/同闭包)。
pub fn replace_variants(
    conn: &Connection,
    rule_id: &str,
    variants: &[NewVariant<'_>],
) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM rule_variants WHERE rule_id = ?1", params![rule_id])?;
    for (i, v) in variants.iter().enumerate() {
        let joined = v.spec_values.join(";");
        conn.execute(
            "INSERT INTO rule_variants(id, rule_id, spec_name, spec_value, source, card_pool_id,
                 template_id, template_bindings, units_per_item, delay_override_seconds, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                v.id,
                rule_id,
                v.spec_name,
                joined,
                v.source.as_str(),
                v.card_pool_id,
                v.template_id,
                v.template_bindings,
                v.units_per_item,
                v.delay_override_seconds,
                (i + 1) as i64,
            ],
        )?;
    }
    Ok(())
}

pub fn list_variants(conn: &Connection, rule_id: &str) -> rusqlite::Result<Vec<VariantRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, rule_id, spec_name, spec_value, source, card_pool_id, template_id,
                template_bindings, units_per_item, delay_override_seconds, position
         FROM rule_variants WHERE rule_id = ?1 ORDER BY position",
    )?;
    let rows = stmt
        .query_map(params![rule_id], |r| {
            let source = r.get::<_, String>(4)?;
            Ok(VariantRow {
                id: r.get(0)?,
                rule_id: r.get(1)?,
                spec_name: r.get(2)?,
                spec_values: r
                    .get::<_, String>(3)?
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
                source: crate::domain::rules_ext::VariantSource::parse(&source)
                    .unwrap_or(crate::domain::rules_ext::VariantSource::CardPool),
                card_pool_id: r.get(5)?,
                template_id: r.get(6)?,
                template_bindings: r.get(7)?,
                units_per_item: r.get(8)?,
                delay_override_seconds: r.get(9)?,
                position: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 变体引用的卡组/模板清单(模板/卡组删除侧引用扫描用,T023/T009 语义)。
pub fn variant_pool_ids(conn: &Connection, rule_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT card_pool_id FROM rule_variants WHERE rule_id = ?1 AND card_pool_id IS NOT NULL")?;
    let rows = stmt
        .query_map(params![rule_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- reply_rules(关键词回复) ----------

pub struct ReplyRuleRow {
    pub id: String,
    pub account_id: String,
    pub keyword: String,
    pub reply_kind: String,
    pub reply_text: Option<String>,
    pub reply_image_url: Option<String>,
    pub enabled: bool,
    /// 无关联行 = 账号级(data-model)
    pub item_ids: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn insert_reply_items(
    conn: &Connection,
    reply_rule_id: &str,
    item_ids: &[String],
) -> rusqlite::Result<()> {
    for item in item_ids {
        conn.execute(
            "INSERT INTO reply_rule_items(reply_rule_id, item_id) VALUES (?1, ?2)",
            params![reply_rule_id, item],
        )?;
    }
    Ok(())
}

fn reply_rule_row(conn: &Connection, id: &str) -> rusqlite::Result<Option<ReplyRuleRow>> {
    let base = conn
        .query_row(
            "SELECT id, account_id, keyword, reply_kind, reply_text, reply_image_url, enabled
             FROM reply_rules WHERE id = ?1",
            params![id],
            |r| {
                Ok(ReplyRuleRow {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    keyword: r.get(2)?,
                    reply_kind: r.get(3)?,
                    reply_text: r.get(4)?,
                    reply_image_url: r.get(5)?,
                    enabled: r.get::<_, i64>(6)? != 0,
                    item_ids: Vec::new(),
                })
            },
        )
        .optional()?;
    match base {
        Some(mut row) => {
            let mut stmt = conn.prepare(
                "SELECT item_id FROM reply_rule_items WHERE reply_rule_id = ?1 ORDER BY item_id",
            )?;
            row.item_ids = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(row))
        }
        None => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn insert_reply_rule(
    conn: &Connection,
    id: &str,
    account_id: &str,
    keyword: &str,
    reply_kind: &str,
    reply_text: Option<&str>,
    reply_image_url: Option<&str>,
    enabled: bool,
    item_ids: &[String],
) -> rusqlite::Result<ReplyRuleRow> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO reply_rules(id, account_id, keyword, reply_kind, reply_text, reply_image_url,
             enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![id, account_id, keyword, reply_kind, reply_text, reply_image_url, enabled as i64, now],
    )?;
    insert_reply_items(conn, id, item_ids)?;
    reply_rule_row(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn list_reply_rules(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<ReplyRuleRow>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM reply_rules WHERE account_id = ?1 ORDER BY created_at, id",
    )?;
    let ids = stmt
        .query_map(params![account_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(row) = reply_rule_row(conn, &id)? {
            out.push(row);
        }
    }
    Ok(out)
}

/// 整体替换更新(keyword/回复内容/enabled/关联商品);不存在返回 None。
#[allow(clippy::too_many_arguments)]
pub fn update_reply_rule(
    conn: &Connection,
    id: &str,
    keyword: &str,
    reply_kind: &str,
    reply_text: Option<&str>,
    reply_image_url: Option<&str>,
    enabled: bool,
    item_ids: &[String],
) -> rusqlite::Result<Option<ReplyRuleRow>> {
    let n = conn.execute(
        "UPDATE reply_rules SET keyword = ?2, reply_kind = ?3, reply_text = ?4,
                reply_image_url = ?5, enabled = ?6, updated_at = ?7
         WHERE id = ?1",
        params![id, keyword, reply_kind, reply_text, reply_image_url, enabled as i64, now_rfc3339()],
    )?;
    if n == 0 {
        return Ok(None);
    }
    conn.execute("DELETE FROM reply_rule_items WHERE reply_rule_id = ?1", params![id])?;
    insert_reply_items(conn, id, item_ids)?;
    reply_rule_row(conn, id)
}

pub fn delete_reply_rule(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM reply_rules WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// 匹配查询(research D7 关键词环节的数据面):包含匹配忽略大小写,
/// 商品级优先于账号级,enabled=1;返回序即分流优先序(执行编排属 T036)。
pub fn match_reply_rules(
    conn: &Connection,
    account_id: &str,
    item_id: Option<&str>,
    text: &str,
) -> rusqlite::Result<Vec<ReplyRuleRow>> {
    let all = list_reply_rules(conn, account_id)?;
    let needle = text.to_lowercase();
    let mut matches: Vec<ReplyRuleRow> = all
        .into_iter()
        .filter(|r| {
            r.enabled
                && !r.keyword.is_empty()
                && needle.contains(&r.keyword.to_lowercase())
                && match item_id {
                    Some(item) => r.item_ids.is_empty() || r.item_ids.iter().any(|i| i == item),
                    None => r.item_ids.is_empty(),
                }
        })
        .collect();
    // 商品级(有关联行)优先于账号级;同级保持配置顺序
    matches.sort_by_key(|r| r.item_ids.is_empty());
    Ok(matches)
}

// ---------- default_replies / default_reply_log ----------

pub struct DefaultReplyRow {
    pub account_id: String,
    pub enabled: bool,
    pub reply_text: Option<String>,
    pub reply_image_url: Option<String>,
    pub reply_once: bool,
    pub updated_at: String,
}

pub struct DefaultReplyLogRow {
    pub id: String,
    pub buyer_id: String,
    pub state: String,
    pub sent_at: String,
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_default_reply(
    conn: &Connection,
    account_id: &str,
    enabled: bool,
    reply_text: Option<&str>,
    reply_image_url: Option<&str>,
    reply_once: bool,
) -> rusqlite::Result<DefaultReplyRow> {
    conn.execute(
        "INSERT INTO default_replies(account_id, enabled, reply_text, reply_image_url, reply_once, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(account_id) DO UPDATE SET
             enabled = excluded.enabled, reply_text = excluded.reply_text,
             reply_image_url = excluded.reply_image_url, reply_once = excluded.reply_once,
             updated_at = excluded.updated_at",
        params![
            account_id,
            enabled as i64,
            reply_text,
            reply_image_url,
            reply_once as i64,
            now_rfc3339(),
        ],
    )?;
    get_default_reply(conn, account_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_default_reply(
    conn: &Connection,
    account_id: &str,
) -> rusqlite::Result<Option<DefaultReplyRow>> {
    conn.query_row(
        "SELECT account_id, enabled, reply_text, reply_image_url, reply_once, updated_at
         FROM default_replies WHERE account_id = ?1",
        params![account_id],
        |r| {
            Ok(DefaultReplyRow {
                account_id: r.get(0)?,
                enabled: r.get::<_, i64>(1)? != 0,
                reply_text: r.get(2)?,
                reply_image_url: r.get(3)?,
                reply_once: r.get::<_, i64>(4)? != 0,
                updated_at: r.get(5)?,
            })
        },
    )
    .optional()
}

/// 默认回复发送留痕(state ∈ accepted/failed/unknown;发送编排属 T036)。
pub fn append_default_reply_log(
    conn: &Connection,
    id: &str,
    account_id: &str,
    buyer_id: &str,
    state: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO default_reply_log(id, account_id, buyer_id, state, sent_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, account_id, buyer_id, state, now_rfc3339()],
    )?;
    Ok(())
}

/// reply_once 判定:同 (account,buyer) 存在 accepted 行(data-model)。
pub fn has_accepted_default_reply(
    conn: &Connection,
    account_id: &str,
    buyer_id: &str,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM default_reply_log
         WHERE account_id = ?1 AND buyer_id = ?2 AND state = 'accepted'",
        params![account_id, buyer_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
}

pub fn list_default_reply_log(
    conn: &Connection,
    account_id: &str,
    limit: i64,
) -> rusqlite::Result<Vec<DefaultReplyLogRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, buyer_id, state, sent_at FROM default_reply_log
         WHERE account_id = ?1 ORDER BY sent_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![account_id, limit], |r| {
            Ok(DefaultReplyLogRow {
                id: r.get(0)?,
                buyer_id: r.get(1)?,
                state: r.get(2)?,
                sent_at: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 清空回复记录(FR-034);返回删除行数。
pub fn clear_default_reply_log(conn: &Connection, account_id: &str) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "DELETE FROM default_reply_log WHERE account_id = ?1",
        params![account_id],
    )?;
    Ok(n)
}

// ---------- review_reminder_state(求评计划,T037 消费) ----------

pub struct ReminderStateRow {
    pub order_db_id: String,
    pub rule_id: String,
    pub reminded_count: i64,
    pub last_reminded_at: Option<String>,
    pub next_due_at: Option<String>,
}

pub fn get_reminder(
    conn: &Connection,
    order_db_id: &str,
    rule_id: &str,
) -> rusqlite::Result<Option<ReminderStateRow>> {
    conn.query_row(
        "SELECT order_db_id, rule_id, reminded_count, last_reminded_at, next_due_at
         FROM review_reminder_state WHERE order_db_id = ?1 AND rule_id = ?2",
        params![order_db_id, rule_id],
        |r| {
            Ok(ReminderStateRow {
                order_db_id: r.get(0)?,
                rule_id: r.get(1)?,
                reminded_count: r.get(2)?,
                last_reminded_at: r.get(3)?,
                next_due_at: r.get(4)?,
            })
        },
    )
    .optional()
}

/// 幂等 upsert(首次插入 0 计数;T037 扫描后推进)。
pub fn upsert_reminder(
    conn: &Connection,
    order_db_id: &str,
    rule_id: &str,
    reminded_count: i64,
    last_reminded_at: Option<&str>,
    next_due_at: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO review_reminder_state(order_db_id, rule_id, reminded_count, last_reminded_at, next_due_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(order_db_id, rule_id) DO UPDATE SET
             reminded_count = excluded.reminded_count,
             last_reminded_at = excluded.last_reminded_at,
             next_due_at = excluded.next_due_at",
        params![order_db_id, rule_id, reminded_count, last_reminded_at, next_due_at],
    )?;
    Ok(())
}

/// 到期扫描(T037 每小时定时器消费):next_due_at ≤ now 且有求评配置的规则。
pub fn list_due_reminders(
    conn: &Connection,
    now_rfc3339: &str,
    limit: i64,
) -> rusqlite::Result<Vec<ReminderStateRow>> {
    let mut stmt = conn.prepare(
        "SELECT order_db_id, rule_id, reminded_count, last_reminded_at, next_due_at
         FROM review_reminder_state WHERE next_due_at IS NOT NULL AND next_due_at <= ?1
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now_rfc3339, limit], |r| {
            Ok(ReminderStateRow {
                order_db_id: r.get(0)?,
                rule_id: r.get(1)?,
                reminded_count: r.get(2)?,
                last_reminded_at: r.get(3)?,
                next_due_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- 求评候选扫描(T037;发送与状态推进在应用层,网络不进事务) ----------

/// 求评扫描候选行:accepted 交付 × 订单事实 × 账号门禁标志 × 匹配的求评规则
/// × 现有状态行(缺失 = 首轮未排,由 wait_hours 推导首次到期)。
pub struct ReviewCandidateRow {
    pub order_db_id: String,
    pub account_id: String,
    pub buyer_id: Option<String>,
    /// 内容已接纳的时间锚点(deliveries.updated_at;后续确认轴更新只会推迟,安全侧)
    pub accepted_at_ms: i64,
    pub rule_id: String,
    /// review_config JSON 原文(应用层解析 ReviewConfig,坏行跳过)
    pub review_config: String,
    pub runtime_enabled: bool,
    pub status: String,
    pub control_epoch: i64,
    pub credential_epoch: i64,
    pub reminded_count: i64,
    pub last_reminded_at: Option<String>,
    pub next_due_at: Option<String>,
}

/// 全量候选装载(T037 run_once 消费):只做读取与规则匹配,
/// 不写任何表;到期判定与发送在应用层。
pub fn list_review_candidates(conn: &Connection) -> rusqlite::Result<Vec<ReviewCandidateRow>> {
    struct Accepted {
        order_db_id: String,
        account_id: String,
        buyer_id: Option<String>,
        item_id: Option<String>,
        accepted_at_ms: i64,
        runtime_enabled: bool,
        status: String,
        control_epoch: i64,
        credential_epoch: i64,
    }
    let mut stmt = conn.prepare(
        "SELECT o.id, o.account_id, o.buyer_id, o.item_id, d.updated_at,
                a.runtime_enabled, a.status, a.control_epoch, a.credential_epoch
         FROM deliveries d
         JOIN orders o ON o.id = d.order_id
         JOIN accounts a ON a.id = o.account_id
         WHERE d.kind = 'initial' AND d.content_state = 'accepted'
         ORDER BY d.updated_at, o.id",
    )?;
    let accepted = stmt
        .query_map([], |r| {
            Ok(Accepted {
                order_db_id: r.get(0)?,
                account_id: r.get(1)?,
                buyer_id: r.get(2)?,
                item_id: r.get(3)?,
                accepted_at_ms: r.get(4)?,
                runtime_enabled: r.get::<_, i64>(5)? != 0,
                status: r.get(6)?,
                control_epoch: r.get(7)?,
                credential_epoch: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if accepted.is_empty() {
        return Ok(Vec::new());
    }

    // 启用中的求评规则(优先级升序;账号级哨兵行不在此翻译,匹配在下方)
    struct ReviewRule {
        id: String,
        account_id: String,
        item_id: String,
        all_items_confirmed: bool,
        review_config: String,
    }
    let mut stmt = conn.prepare(
        "SELECT id, account_id, item_id, all_items_confirmed, review_config
         FROM rules
         WHERE trigger_type = 'review_missing_timeout' AND enabled = 1
           AND review_config IS NOT NULL AND needs_reconfiguration = 0
         ORDER BY priority ASC, created_at ASC, id ASC",
    )?;
    let rules = stmt
        .query_map([], |r| {
            Ok(ReviewRule {
                id: r.get(0)?,
                account_id: r.get(1)?,
                item_id: r.get(2)?,
                all_items_confirmed: r.get::<_, i64>(3)? != 0,
                review_config: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::new();
    for a in accepted {
        let sentinel = account_scope_item_id(&a.account_id);
        // 范围匹配:商品精确(orders.item_id 现阶段无写入方,分支为 US6 同步后的
        // 前向兼容;当前实际命中走账号级)∪ 账号级(须已确认「适用于全部商品」,
        // 与 T033 门禁一致);优先级升序遍历取第一条适用规则。
        let Some(rule) = rules.iter().find(|r| {
            let item_scope = match a.item_id.as_deref() {
                Some(item) if !item.is_empty() => r.item_id == item,
                _ => false,
            };
            let account_scope = r.item_id == sentinel && r.all_items_confirmed;
            r.account_id == a.account_id && (item_scope || account_scope)
        }) else {
            continue;
        };
        let state = get_reminder(conn, &a.order_db_id, &rule.id)?;
        let (count, last, next) = match state {
            Some(s) => (s.reminded_count, s.last_reminded_at, s.next_due_at),
            None => (0, None, None),
        };
        out.push(ReviewCandidateRow {
            order_db_id: a.order_db_id,
            account_id: a.account_id,
            buyer_id: a.buyer_id,
            accepted_at_ms: a.accepted_at_ms,
            rule_id: rule.id.clone(),
            review_config: rule.review_config.clone(),
            runtime_enabled: a.runtime_enabled,
            status: a.status,
            control_epoch: a.control_epoch,
            credential_epoch: a.credential_epoch,
            reminded_count: count,
            last_reminded_at: last,
            next_due_at: next,
        });
    }
    Ok(out)
}
