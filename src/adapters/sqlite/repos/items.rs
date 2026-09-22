//! items 表仓储:账号内商品唯一,历史引用不删。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct ItemRow {
    pub id: String,
    pub account_id: String,
    pub external_item_id: String,
    pub title: String,
    pub listing_state: String,
    pub sku_definition: String,
    pub sku_completeness: String,
    pub version: i64,
}

pub fn upsert(
    conn: &Connection,
    id: &str,
    account_id: &str,
    external_item_id: &str,
    title: &str,
    listing_state: &str,
    sku_definition: &str,
    sku_completeness: &str,
) -> rusqlite::Result<ItemRow> {
    let existing = find_by_external(conn, account_id, external_item_id)?;
    let now = utc_now_ms();
    match existing {
        Some(row) => {
            conn.execute(
                "UPDATE items SET title = ?1, listing_state = ?2, sku_definition = ?3,
                        sku_completeness = ?4, version = version + 1, updated_at = ?5
                 WHERE id = ?6",
                params![
                    title,
                    listing_state,
                    sku_definition,
                    sku_completeness,
                    now,
                    row.id
                ],
            )?;
            get(conn, &row.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
        }
        None => {
            conn.execute(
                "INSERT INTO items(id, account_id, external_item_id, title, listing_state,
                     sku_definition, sku_completeness, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    id,
                    account_id,
                    external_item_id,
                    title,
                    listing_state,
                    sku_definition,
                    sku_completeness,
                    now
                ],
            )?;
            get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
        }
    }
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<ItemRow>> {
    conn.query_row(
        "SELECT id, account_id, external_item_id, title, listing_state, sku_definition,
                sku_completeness, version FROM items WHERE id = ?1",
        params![id],
        row,
    )
    .optional()
}

pub fn find_by_external(
    conn: &Connection,
    account_id: &str,
    external_item_id: &str,
) -> rusqlite::Result<Option<ItemRow>> {
    conn.query_row(
        "SELECT id, account_id, external_item_id, title, listing_state, sku_definition,
                sku_completeness, version FROM items
         WHERE account_id = ?1 AND external_item_id = ?2",
        params![account_id, external_item_id],
        row,
    )
    .optional()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        external_item_id: r.get(2)?,
        title: r.get(3)?,
        listing_state: r.get(4)?,
        sku_definition: r.get(5)?,
        sku_completeness: r.get(6)?,
        version: r.get(7)?,
    })
}

/// 账号内商品列表(带规则配置状态徽标计算所需字段)。
pub fn list_for_account(
    conn: &Connection,
    account_id: &str,
    limit: i64,
) -> rusqlite::Result<Vec<ItemRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, external_item_id, title, listing_state, sku_definition,
                sku_completeness, version FROM items
         WHERE account_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![account_id, limit], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
