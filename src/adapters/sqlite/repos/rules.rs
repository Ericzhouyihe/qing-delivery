//! rules/rule_contents 表仓储:启用范围唯一、内容版本不可变。
//! 正文以 AES-GCM 信封存储,密钥由应用层传入;仓储只搬字节。

use rusqlite::{Connection, OptionalExtension, params};

use crate::adapters::sqlite::repos::items::ItemRow;
use crate::domain::crypto::{Aad, Envelope};
use crate::domain::time_util::utc_now_ms;

pub struct RuleRow {
    pub id: String,
    pub account_id: String,
    pub item_id: String,
    pub sku_key: String,
    pub enabled: bool,
    pub current_content_version: i64,
    pub version: i64,
    /// 007 增量 1:内容来源卡密组(None/空 = fixed_text)
    pub card_pool_id: Option<String>,
    /// 007 增量 1(US2):内容来源发货模板(None/空 = 非模板来源)
    pub template_id: Option<String>,
    /// 007 增量 1(US2):模板绑定 JSON `{cards:[{key,pool_id,units}],custom:{k:v}}`
    pub template_bindings: Option<String>,
    /// 007 增量 1(US3,T032):触发类型(order_paid/review_missing_timeout/buyer_reviewed)
    pub trigger_type: String,
    /// 007 增量 1(US3,T032):优先级(数字越小越高,默认 100)
    pub priority: i64,
    /// 007 增量 1(US3,T032):账号级规则「适用于全部商品」确认标记(FR-032)
    pub all_items_confirmed: bool,
    /// 007 增量 1(US3,T032):需重新配置(引用缺失/未确认)→ 执行旁路
    pub needs_reconfiguration: bool,
    /// 007 增量 1(US3,T032):求评配置 JSON(wait_hours/interval_hours/max_count/text)
    pub review_config: Option<String>,
}

/// rules 表 US3 扩展列的统一 SELECT 片段(T032;新旧查询共用)。
const RULE_EXT_COLS: &str = ", trigger_type, priority, all_items_confirmed,
    needs_reconfiguration, review_config";

fn ext_rule_fields(r: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<(
    String,
    i64,
    bool,
    bool,
    Option<String>,
)> {
    Ok((
        r.get(base)?,
        r.get(base + 1)?,
        r.get::<_, i64>(base + 2)? != 0,
        r.get::<_, i64>(base + 3)? != 0,
        r.get(base + 4)?,
    ))
}

pub struct RuleContentRow {
    pub rule_id: String,
    pub content_version: i64,
    pub envelope: Envelope,
    pub text_digest: String,
    pub char_count: i64,
    pub byte_count: i64,
}

/// 创建规则(不带版本);启用唯一冲突由部分唯一索引拒绝(FR-008)。
pub fn insert(
    conn: &Connection,
    id: &str,
    account_id: &str,
    item_id: &str,
    sku_key: &str,
    enabled: bool,
) -> rusqlite::Result<RuleRow> {
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO rules(id, account_id, item_id, sku_key, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![id, account_id, item_id, sku_key, enabled as i64, now],
    )?;
    get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// 版本化更新(含禁用);expected_version 不匹配返回 None。
pub fn update_enabled(
    conn: &Connection,
    id: &str,
    enabled: bool,
    expected_version: i64,
) -> rusqlite::Result<Option<RuleRow>> {
    let _ = conn.execute(
        "UPDATE rules SET enabled = ?1, version = version + 1, updated_at = ?2
         WHERE id = ?3 AND version = ?4",
        params![enabled as i64, utc_now_ms(), id, expected_version],
    )?;
    get(conn, id)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<RuleRow>> {
    conn.query_row(
        &format!(
            "SELECT id, account_id, item_id, sku_key, enabled, current_content_version, version,
                card_pool_id, template_id, template_bindings{RULE_EXT_COLS}
             FROM rules WHERE id = ?1"
        ),
        params![id],
        rule_row,
    )
    .optional()
}

/// 共享行映射(T032 起 rules 全部查询列一致)。
fn rule_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RuleRow> {
    let (trigger_type, priority, all_confirmed, needs_reconf, review_config) =
        ext_rule_fields(r, 10)?;
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
        trigger_type,
        priority,
        all_items_confirmed: all_confirmed,
        needs_reconfiguration: needs_reconf,
        review_config,
    })
}

/// 精确匹配范围的启用规则;多条启用(历史数据异常)返回 None 表示歧义。
/// 007 US3 后执行侧改用 rules_ext::load_order_paid_candidates(T032/D1);
/// 本函数保留给固定内容旧路径与诊断。
pub fn find_enabled_exact(
    conn: &Connection,
    account_id: &str,
    item_id: &str,
    sku_key: &str,
) -> rusqlite::Result<Option<RuleRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, account_id, item_id, sku_key, enabled, current_content_version, version,
                card_pool_id, template_id, template_bindings{RULE_EXT_COLS}
         FROM rules WHERE account_id = ?1 AND item_id = ?2 AND sku_key = ?3 AND enabled = 1"
    ))?;
    let rows = stmt
        .query_map(params![account_id, item_id, sku_key], rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 1 {
        // 执行时发现歧义:不得任选一条(FR-008);None 由调用方结合计数判断
        return Ok(None);
    }
    Ok(rows.into_iter().next())
}

pub fn count_enabled_exact(
    conn: &Connection,
    account_id: &str,
    item_id: &str,
    sku_key: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM rules WHERE account_id = ?1 AND item_id = ?2 AND sku_key = ?3 AND enabled = 1",
        params![account_id, item_id, sku_key],
        |r| r.get(0),
    )
}

/// 追加不可变内容版本(唯一约束 (rule_id, content_version) 拒绝重复)。
pub fn insert_content(
    conn: &Connection,
    id: &str,
    rule_id: &str,
    content_version: i64,
    envelope: &Envelope,
    text_digest: &str,
    char_count: i64,
    byte_count: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rule_contents(id, rule_id, content_version, ciphertext, nonce, key_id,
             format_version, text_digest, char_count, byte_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            rule_id,
            content_version,
            envelope.ciphertext,
            envelope.nonce.as_slice(),
            envelope.key_id,
            envelope.format_version as i64,
            text_digest,
            char_count,
            byte_count,
            utc_now_ms()
        ],
    )?;
    conn.execute(
        "UPDATE rules SET current_content_version = ?1, version = version + 1, updated_at = ?2
         WHERE id = ?3",
        params![content_version, utc_now_ms(), rule_id],
    )?;
    Ok(())
}

pub fn get_content(
    conn: &Connection,
    rule_id: &str,
    content_version: i64,
) -> rusqlite::Result<Option<RuleContentRow>> {
    conn.query_row(
        "SELECT rule_id, content_version, ciphertext, nonce, key_id, format_version,
                text_digest, char_count, byte_count
         FROM rule_contents WHERE rule_id = ?1 AND content_version = ?2",
        params![rule_id, content_version],
        |r| {
            let nonce_slice: Vec<u8> = r.get(3)?;
            let mut nonce = [0u8; crate::domain::crypto::NONCE_LEN];
            if nonce_slice.len() == nonce.len() {
                nonce.copy_from_slice(&nonce_slice);
            }
            Ok(RuleContentRow {
                rule_id: r.get(0)?,
                content_version: r.get(1)?,
                envelope: Envelope {
                    ciphertext: r.get(2)?,
                    nonce,
                    key_id: r.get(4)?,
                    format_version: r.get::<_, i64>(5)? as u16,
                },
                text_digest: r.get(6)?,
                char_count: r.get(7)?,
                byte_count: r.get(8)?,
            })
        },
    )
    .optional()
}

pub fn rule_aad(rule_id: &str, content_version: i64) -> Aad {
    Aad {
        purpose: "rule_content".into(),
        entity_id: rule_id.into(),
        content_version: Some(content_version),
    }
}

/// 便捷:取商品当前启用规则的 SKU 键(调试/测试辅助)。
pub fn item_of_rule(conn: &Connection, rule: &RuleRow) -> rusqlite::Result<Option<ItemRow>> {
    crate::adapters::sqlite::repos::items::get(conn, &rule.item_id)
}

/// 账号内规则列表(不含正文)。US3 过滤/搜索/分页扩展见 rules_ext::list_rules_filtered。
pub fn list_for_account(
    conn: &Connection,
    account_id: &str,
    limit: i64,
) -> rusqlite::Result<Vec<RuleRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, account_id, item_id, sku_key, enabled, current_content_version, version,
                card_pool_id, template_id, template_bindings{RULE_EXT_COLS}
         FROM rules WHERE account_id = ?1 ORDER BY created_at DESC LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![account_id, limit], rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 删除结果:被交付历史引用时拒绝(与卡组/模板的引用保护同语义,保留审计链)。
pub enum RuleDeleteOutcome {
    Deleted,
    NotFound,
    VersionConflict,
    ReferencedByDeliveries(i64),
}

/// 删除规则:清 rule_contents 与 review_reminder_state(rule_variants 由 FK 级联),
/// 被 deliveries.rule_id 引用时拒绝;expected_version 乐观锁。
pub fn delete(
    conn: &Connection,
    account_id: &str,
    rule_id: &str,
    expected_version: i64,
) -> rusqlite::Result<RuleDeleteOutcome> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT account_id, version FROM rules WHERE id = ?1",
            params![rule_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((owner, version)) = row else {
        return Ok(RuleDeleteOutcome::NotFound);
    };
    if owner != account_id {
        return Ok(RuleDeleteOutcome::NotFound);
    }
    if version != expected_version {
        return Ok(RuleDeleteOutcome::VersionConflict);
    }
    let refs: i64 = conn.query_row(
        "SELECT COUNT(*) FROM deliveries WHERE rule_id = ?1",
        params![rule_id],
        |r| r.get(0),
    )?;
    if refs > 0 {
        return Ok(RuleDeleteOutcome::ReferencedByDeliveries(refs));
    }
    conn.execute("DELETE FROM rule_contents WHERE rule_id = ?1", params![rule_id])?;
    conn.execute("DELETE FROM review_reminder_state WHERE rule_id = ?1", params![rule_id])?;
    conn.execute("DELETE FROM rules WHERE id = ?1", params![rule_id])?;
    Ok(RuleDeleteOutcome::Deleted)
}
