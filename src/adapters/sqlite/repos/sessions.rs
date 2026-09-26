//! sessions 表仓储:服务端会话,12 小时绝对有效期;退出/改密/恢复撤销。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub const SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;

pub struct SessionRow {
    pub token_hash: String,
    pub admin_id: String,
    pub csrf_hash: String,
    pub expires_at: i64,
}

pub fn insert_session(
    conn: &Connection,
    token_hash: &str,
    admin_id: &str,
    csrf_hash: &str,
    ttl_ms: i64,
) -> rusqlite::Result<i64> {
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO sessions(token_hash, admin_id, csrf_hash, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash, admin_id, csrf_hash, now, now + ttl_ms],
    )?;
    Ok(now)
}

/// 查找有效(未撤销、未过期)会话。
pub fn find_valid_session(
    conn: &Connection,
    token_hash: &str,
) -> rusqlite::Result<Option<SessionRow>> {
    let now = utc_now_ms();
    conn.query_row(
        "SELECT token_hash, admin_id, csrf_hash, expires_at FROM sessions
         WHERE token_hash = ?1 AND revoked_at IS NULL AND expires_at > ?2",
        params![token_hash, now],
        |r| {
            Ok(SessionRow {
                token_hash: r.get(0)?,
                admin_id: r.get(1)?,
                csrf_hash: r.get(2)?,
                expires_at: r.get(3)?,
            })
        },
    )
    .optional()
}

pub fn revoke_session(conn: &Connection, token_hash: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET revoked_at = ?1 WHERE token_hash = ?2 AND revoked_at IS NULL",
        params![utc_now_ms(), token_hash],
    )?;
    Ok(())
}

/// 会话查询时轮换 CSRF 哈希;仅对有效会话生效。
pub fn rotate_csrf(
    conn: &Connection,
    token_hash: &str,
    new_csrf_hash: &str,
) -> rusqlite::Result<bool> {
    let now = utc_now_ms();
    let n = conn.execute(
        "UPDATE sessions SET csrf_hash = ?1 WHERE token_hash = ?2 AND revoked_at IS NULL AND expires_at > ?3",
        params![new_csrf_hash, token_hash, now],
    )?;
    Ok(n == 1)
}

pub fn revoke_all_sessions(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET revoked_at = ?1 WHERE revoked_at IS NULL",
        params![utc_now_ms()],
    )?;
    Ok(())
}

/// 007 US7 改密语义:撤销除指定会话外的全部会话(FR-072,当前会话保留)。
pub fn revoke_all_except(conn: &Connection, keep_token_hash: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sessions SET revoked_at = ?1 WHERE revoked_at IS NULL AND token_hash != ?2",
        params![utc_now_ms(), keep_token_hash],
    )?;
    Ok(())
}

pub fn purge_expired(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1",
        params![utc_now_ms()],
    )
}
