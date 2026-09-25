//! card_pools/card_entries 仓储(T009):组 CRUD(乐观锁)、追加去重、
//! 分状态计数、条目分页、原子预留(RETURNING+状态谓词防双配)、释放/扣减。
//! 明文只在应用层出现;仓储只搬信封字节与摘要。时间戳为 RFC3339 TEXT。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::crypto::Envelope;
use crate::domain::time_util::{format_rfc3339, utc_now_ms};

fn now_rfc3339() -> String {
    format_rfc3339(utc_now_ms())
}

pub struct CardPoolRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub description: String,
    pub content_envelope: Option<Envelope>,
    pub api_config_envelope: Option<Envelope>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

const POOL_COLS: &str = "id, name, kind, enabled, delay_seconds, description,
    content_ciphertext, content_nonce, key_id, format_version,
    api_config_ciphertext, api_config_nonce, api_key_id, api_format_version,
    version, created_at, updated_at";

/// 信封四件套列 → Envelope(nonce 定长;异常长度静默零 nonce,解密必然失败并上报)。
fn envelope_of(nonce: Vec<u8>, ciphertext: Vec<u8>, key_id: String, v: i64) -> Envelope {
    let mut fixed = [0u8; crate::domain::crypto::NONCE_LEN];
    if nonce.len() == fixed.len() {
        fixed.copy_from_slice(&nonce);
    }
    Envelope {
        nonce: fixed,
        ciphertext,
        key_id,
        format_version: v as u16,
    }
}

fn pool_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CardPoolRow> {
    let content_envelope = match r.get::<_, Option<Vec<u8>>>(6)? {
        Some(ciphertext) => Some(envelope_of(
            r.get(7)?,
            ciphertext,
            r.get(8)?,
            r.get::<_, Option<i64>>(9)?.unwrap_or(1),
        )),
        None => None,
    };
    let api_config_envelope = match r.get::<_, Option<Vec<u8>>>(10)? {
        Some(ciphertext) => Some(envelope_of(
            r.get(11)?,
            ciphertext,
            r.get(12)?,
            r.get::<_, Option<i64>>(13)?.unwrap_or(1),
        )),
        None => None,
    };
    Ok(CardPoolRow {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        delay_seconds: r.get(4)?,
        description: r.get(5)?,
        content_envelope,
        api_config_envelope,
        version: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

pub struct NewPool<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub kind: &'a str,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub description: &'a str,
    /// kind=text/image 的固定内容信封
    pub content_envelope: Option<&'a Envelope>,
    /// kind=api 的整份配置信封
    pub api_config_envelope: Option<&'a Envelope>,
}

pub fn insert_pool(conn: &Connection, p: &NewPool<'_>) -> rusqlite::Result<CardPoolRow> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO card_pools(id, name, kind, enabled, delay_seconds, description,
             content_ciphertext, content_nonce, key_id, format_version,
             api_config_ciphertext, api_config_nonce, api_key_id, api_format_version,
             version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1, ?15, ?15)",
        params![
            p.id,
            p.name,
            p.kind,
            p.enabled as i64,
            p.delay_seconds,
            p.description,
            p.content_envelope.map(|e| e.ciphertext.as_slice()),
            p.content_envelope.map(|e| e.nonce.as_slice()),
            p.content_envelope.map(|e| e.key_id.as_str()),
            p.content_envelope.map(|e| e.format_version as i64),
            p.api_config_envelope.map(|e| e.ciphertext.as_slice()),
            p.api_config_envelope.map(|e| e.nonce.as_slice()),
            p.api_config_envelope.map(|e| e.key_id.as_str()),
            p.api_config_envelope.map(|e| e.format_version as i64),
            now,
        ],
    )?;
    get_pool(conn, p.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// 版本化更新;expected_version 不匹配返回 None。信封字段 Some = 整体替换。
pub struct PoolPatch<'a> {
    pub name: Option<&'a str>,
    pub enabled: Option<bool>,
    pub delay_seconds: Option<i64>,
    pub description: Option<&'a str>,
    pub content_envelope: Option<&'a Envelope>,
    pub api_config_envelope: Option<&'a Envelope>,
}

pub fn update_pool(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    patch: &PoolPatch<'_>,
) -> rusqlite::Result<Option<CardPoolRow>> {
    let n = conn.execute(
        "UPDATE card_pools SET
             name = COALESCE(?2, name),
             enabled = COALESCE(?3, enabled),
             delay_seconds = COALESCE(?4, delay_seconds),
             description = COALESCE(?5, description),
             content_ciphertext = COALESCE(?6, content_ciphertext),
             content_nonce = COALESCE(?7, content_nonce),
             key_id = COALESCE(?8, key_id),
             format_version = COALESCE(?9, format_version),
             api_config_ciphertext = COALESCE(?10, api_config_ciphertext),
             api_config_nonce = COALESCE(?11, api_config_nonce),
             api_key_id = COALESCE(?12, api_key_id),
             api_format_version = COALESCE(?13, api_format_version),
             version = version + 1, updated_at = ?14
         WHERE id = ?1 AND version = ?15",
        params![
            id,
            patch.name,
            patch.enabled.map(|b| b as i64),
            patch.delay_seconds,
            patch.description,
            patch.content_envelope.map(|e| e.ciphertext.as_slice()),
            patch.content_envelope.map(|e| e.nonce.as_slice()),
            patch.content_envelope.map(|e| e.key_id.as_str()),
            patch.content_envelope.map(|e| e.format_version as i64),
            patch.api_config_envelope.map(|e| e.ciphertext.as_slice()),
            patch.api_config_envelope.map(|e| e.nonce.as_slice()),
            patch.api_config_envelope.map(|e| e.key_id.as_str()),
            patch.api_config_envelope.map(|e| e.format_version as i64),
            now_rfc3339(),
            expected_version,
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get_pool(conn, id)
}

pub fn get_pool(conn: &Connection, id: &str) -> rusqlite::Result<Option<CardPoolRow>> {
    conn.query_row(
        &format!("SELECT {POOL_COLS} FROM card_pools WHERE id = ?1"),
        params![id],
        pool_row,
    )
    .optional()
}

/// 组名唯一性由应用层校验(data-model);本查询提供判重原料。
pub fn find_pool_by_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<CardPoolRow>> {
    conn.query_row(
        &format!("SELECT {POOL_COLS} FROM card_pools WHERE name = ?1"),
        params![name],
        pool_row,
    )
    .optional()
}

/// LIKE 通配符转义(搜索只按字面包含)。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn list_pools(
    conn: &Connection,
    kind: Option<&str>,
    search: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<CardPoolRow>> {
    let mut sql = format!("SELECT {POOL_COLS} FROM card_pools WHERE 1 = 1");
    let mut binds: Vec<&dyn rusqlite::ToSql> = Vec::new();
    let kind_owned;
    if let Some(k) = kind {
        sql.push_str(" AND kind = ?");
        kind_owned = k.to_string();
        binds.push(&kind_owned);
    }
    let search_owned;
    if let Some(s) = search {
        sql.push_str(" AND name LIKE '%' || ? || '%' ESCAPE '\\'");
        search_owned = like_escape(s);
        binds.push(&search_owned);
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    binds.push(&limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(binds.as_slice())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(pool_row(r)?);
    }
    Ok(out)
}

/// 删除结果:被规则/变体引用时拒绝(应用层转 409 referenced_resource)。
#[derive(Debug, PartialEq)]
pub enum DeletePoolOutcome {
    Deleted,
    Referenced { rules: i64, variants: i64 },
}

/// 删除组:被 rules.card_pool_id / rule_variants.card_pool_id 引用时拒绝,
/// 并把引用规则置 needs_reconfiguration=1(规则页据此显示"需重新配置")。
/// 未被引用时删除(card_entries 随外键级联清理)。
pub fn delete_pool(conn: &Connection, id: &str) -> rusqlite::Result<DeletePoolOutcome> {
    let rules: i64 = conn.query_row(
        "SELECT COUNT(*) FROM rules WHERE card_pool_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    let variants: i64 = conn.query_row(
        "SELECT COUNT(*) FROM rule_variants WHERE card_pool_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    if rules > 0 || variants > 0 {
        if rules > 0 {
            conn.execute(
                "UPDATE rules SET needs_reconfiguration = 1, version = version + 1,
                        updated_at = ?2 WHERE card_pool_id = ?1",
                params![id, now_rfc3339()],
            )?;
        }
        return Ok(DeletePoolOutcome::Referenced { rules, variants });
    }
    conn.execute("DELETE FROM card_pools WHERE id = ?1", params![id])?;
    Ok(DeletePoolOutcome::Deleted)
}

/// 分状态计数(DTO stock 投影)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StockCounts {
    pub available: i64,
    pub reserved: i64,
    pub used: i64,
    pub disabled: i64,
    pub pending_api: i64,
}

impl StockCounts {
    pub fn total(&self) -> i64 {
        self.available + self.reserved + self.used + self.disabled + self.pending_api
    }
}

pub fn state_counts(conn: &Connection, pool_id: &str) -> rusqlite::Result<StockCounts> {
    let mut stmt =
        conn.prepare("SELECT state, COUNT(*) FROM card_entries WHERE pool_id = ?1 GROUP BY state")?;
    let rows = stmt
        .query_map(params![pool_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut counts = StockCounts::default();
    for (state, n) in rows {
        match state.as_str() {
            "available" => counts.available = n,
            "reserved" => counts.reserved = n,
            "used" => counts.used = n,
            "disabled" => counts.disabled = n,
            "pending_api" => counts.pending_api = n,
            _ => {}
        }
    }
    Ok(counts)
}

pub struct NewEntry<'a> {
    pub id: &'a str,
    pub envelope: &'a Envelope,
    pub content_digest: &'a str,
}

pub struct AppendOutcome {
    pub appended: usize,
    pub skipped_duplicate: usize,
}

/// 追加条目:池内 content_digest 去重(重复行跳过不入库)。
/// 空行过滤由应用层完成;本函数假定入参均为非空内容。
pub fn append_entries(
    conn: &Connection,
    pool_id: &str,
    items: &[NewEntry<'_>],
) -> rusqlite::Result<AppendOutcome> {
    let mut outcome = AppendOutcome {
        appended: 0,
        skipped_duplicate: 0,
    };
    for item in items {
        let dup: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM card_entries WHERE pool_id = ?1 AND content_digest = ?2 LIMIT 1",
                params![pool_id, item.content_digest],
                |r| r.get(0),
            )
            .optional()?;
        if dup.is_some() {
            outcome.skipped_duplicate += 1;
            continue;
        }
        conn.execute(
            "INSERT INTO card_entries(id, pool_id, state, origin, content_ciphertext,
                 content_nonce, key_id, format_version, content_digest, created_at)
             VALUES (?1, ?2, 'available', 'stock', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item.id,
                pool_id,
                item.envelope.ciphertext,
                item.envelope.nonce.as_slice(),
                item.envelope.key_id,
                item.envelope.format_version as i64,
                item.content_digest,
                now_rfc3339(),
            ],
        )?;
        outcome.appended += 1;
    }
    Ok(outcome)
}

/// 条目审计行:永不携带信封/明文,摘要仅供比对。
pub struct EntryAuditRow {
    pub id: String,
    pub state: String,
    pub origin: String,
    pub content_digest: String,
    pub reserved_order_id: Option<String>,
    pub reserved_delivery_id: Option<String>,
    pub request_key: Option<String>,
    pub reserved_at: Option<String>,
    pub used_at: Option<String>,
    pub created_at: String,
}

/// 条目分页:state 过滤,cursor = "created_at|id"(新→旧)。
/// 多取一行判断是否还有下一页。
pub fn list_entries(
    conn: &Connection,
    pool_id: &str,
    state: Option<&str>,
    cursor: Option<(&str, &str)>,
    limit: i64,
) -> rusqlite::Result<(Vec<EntryAuditRow>, Option<String>)> {
    let mut sql = String::from(
        "SELECT id, state, origin, content_digest, reserved_order_id, reserved_delivery_id,
                request_key, reserved_at, used_at, created_at
         FROM card_entries WHERE pool_id = ?",
    );
    let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&pool_id];
    let state_owned;
    if let Some(s) = state {
        sql.push_str(" AND state = ?");
        state_owned = s.to_string();
        binds.push(&state_owned);
    }
    let (cur_created, cur_id);
    if let Some((c, i)) = cursor {
        sql.push_str(" AND (created_at, id) < (?, ?)");
        cur_created = c.to_string();
        cur_id = i.to_string();
        binds.push(&cur_created);
        binds.push(&cur_id);
    }
    let fetch_limit = limit + 1;
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    binds.push(&fetch_limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(binds.as_slice())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(EntryAuditRow {
            id: r.get(0)?,
            state: r.get(1)?,
            origin: r.get(2)?,
            content_digest: r.get(3)?,
            reserved_order_id: r.get(4)?,
            reserved_delivery_id: r.get(5)?,
            request_key: r.get(6)?,
            reserved_at: r.get(7)?,
            used_at: r.get(8)?,
            created_at: r.get(9)?,
        });
    }
    let next_cursor = if out.len() as i64 > limit {
        out.pop();
        out.last()
            .map(|last| format!("{}|{}", last.created_at, last.id))
    } else {
        None
    };
    Ok((out, next_cursor))
}

/// 预留成功的条目(含信封供发送前解密)。
pub struct ReservedEntry {
    pub id: String,
    pub envelope: Envelope,
    pub content_digest: String,
}

/// 原子预留一条:单条 UPDATE 的子查询取最早可用条目,状态谓词防双配;
/// 循环 N 次由调用方在自己的事务闭包内驱动,任一次取不到即整体回滚(D3)。
pub fn reserve_one(
    conn: &Connection,
    pool_id: &str,
    order_id: &str,
    delivery_id: &str,
) -> rusqlite::Result<Option<ReservedEntry>> {
    conn.query_row(
        "UPDATE card_entries SET state = 'reserved', reserved_order_id = ?2,
                reserved_delivery_id = ?3, reserved_at = ?4
         WHERE id = (SELECT id FROM card_entries
                     WHERE pool_id = ?1 AND state = 'available'
                     ORDER BY created_at, id LIMIT 1)
         RETURNING id, content_ciphertext, content_nonce, key_id, format_version,
                   content_digest",
        params![pool_id, order_id, delivery_id, now_rfc3339()],
        |r| {
            Ok(ReservedEntry {
                id: r.get(0)?,
                envelope: envelope_of(r.get(2)?, r.get(1)?, r.get(3)?, r.get(4)?),
                content_digest: r.get(5)?,
            })
        },
    )
    .optional()
}

/// 释放:仅 reserved → available 并清空绑定(确定未发送才调用;used 永不复用)。
pub fn release_by_delivery(conn: &Connection, delivery_id: &str) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE card_entries SET state = 'available', reserved_order_id = NULL,
                reserved_delivery_id = NULL, reserved_at = NULL
         WHERE reserved_delivery_id = ?1 AND state = 'reserved'",
        params![delivery_id],
    )?;
    Ok(n)
}

/// 按订单释放(订单终态 canceled/refunded/terminated 的人工处置路径)。
pub fn release_by_order(conn: &Connection, order_id: &str) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE card_entries SET state = 'available', reserved_order_id = NULL,
                reserved_delivery_id = NULL, reserved_at = NULL
         WHERE reserved_order_id = ?1 AND state = 'reserved'",
        params![order_id],
    )?;
    Ok(n)
}

/// 扣减:仅 reserved → used(平台接纳证明后;used 永不复用)。
pub fn consume_by_delivery(conn: &Connection, delivery_id: &str) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE card_entries SET state = 'used', used_at = ?2
         WHERE reserved_delivery_id = ?1 AND state = 'reserved'",
        params![delivery_id, now_rfc3339()],
    )?;
    Ok(n)
}

/// 按订单查预留条目(审计/测试辅助;不含明文)。
pub fn find_reserved_by_order(
    conn: &Connection,
    order_id: &str,
) -> rusqlite::Result<Vec<EntryAuditRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, state, origin, content_digest, reserved_order_id, reserved_delivery_id,
                request_key, reserved_at, used_at, created_at
         FROM card_entries WHERE reserved_order_id = ?1 AND state = 'reserved'
         ORDER BY created_at, id",
    )?;
    let rows = stmt
        .query_map(params![order_id], |r| {
            Ok(EntryAuditRow {
                id: r.get(0)?,
                state: r.get(1)?,
                origin: r.get(2)?,
                content_digest: r.get(3)?,
                reserved_order_id: r.get(4)?,
                reserved_delivery_id: r.get(5)?,
                request_key: r.get(6)?,
                reserved_at: r.get(7)?,
                used_at: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
