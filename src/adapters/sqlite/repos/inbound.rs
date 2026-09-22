//! inbound_events 表仓储:稳定事件 ID 唯一;无稳定 ID 时摘要仅辅助去重。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct InboundRow {
    pub id: String,
    pub account_id: String,
    pub source_event_id: Option<String>,
    pub received_at: i64,
}

/// 记录事件;已有相同 (account_id, source_event_id) 返回 None(重复事件,FR-014)。
pub fn record_event(
    conn: &Connection,
    id: &str,
    account_id: &str,
    source_event_id: Option<&str>,
    payload_digest: &str,
) -> rusqlite::Result<Option<InboundRow>> {
    let now = utc_now_ms();
    if let Some(sid) = source_event_id {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM inbound_events WHERE account_id = ?1 AND source_event_id = ?2",
                params![account_id, sid],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Ok(None);
        }
    }
    conn.execute(
        "INSERT INTO inbound_events(id, account_id, source_event_id, payload_digest, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, account_id, source_event_id, payload_digest, now],
    )?;
    Ok(Some(InboundRow {
        id: id.to_string(),
        account_id: account_id.to_string(),
        source_event_id: source_event_id.map(|s| s.to_string()),
        received_at: now,
    }))
}

pub fn mark_processed(conn: &Connection, id: &str, result: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbound_events SET processed_at = ?1, result = ?2 WHERE id = ?3",
        params![utc_now_ms(), result, id],
    )?;
    Ok(())
}
