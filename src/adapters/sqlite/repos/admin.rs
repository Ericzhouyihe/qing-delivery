//! admin/sessions 表仓储。密码只存 Argon2id 哈希;token 只存 SHA-256。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct AdminRow {
    pub id: String,
    pub username: String,
    pub password_hash: String,
}

/// 原子初始化唯一管理员;已存在时返回 None(唯一约束抵御并发初始化,FR-001)。
pub fn insert_single_admin(
    conn: &Connection,
    id: &str,
    username: &str,
    password_hash: &str,
) -> rusqlite::Result<Option<AdminRow>> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM admin", [], |r| r.get(0))?;
    if count > 0 {
        return Ok(None);
    }
    let now = utc_now_ms();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO admin(id, username, password_hash, created_at, password_changed_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, username, password_hash, now],
    )?;
    tx.commit()?;
    Ok(Some(AdminRow {
        id: id.to_string(),
        username: username.to_string(),
        password_hash: password_hash.to_string(),
    }))
}

pub fn find_admin(conn: &Connection) -> rusqlite::Result<Option<AdminRow>> {
    conn.query_row(
        "SELECT id, username, password_hash FROM admin LIMIT 1",
        [],
        |r| {
            Ok(AdminRow {
                id: r.get(0)?,
                username: r.get(1)?,
                password_hash: r.get(2)?,
            })
        },
    )
    .optional()
}

pub fn update_password_hash(
    conn: &Connection,
    admin_id: &str,
    password_hash: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE admin SET password_hash = ?1, password_changed_at = ?2 WHERE id = ?3",
        params![password_hash, utc_now_ms(), admin_id],
    )?;
    Ok(())
}
