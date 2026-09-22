//! accounts 表仓储:账号身份、开关与代次。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct AccountRow {
    pub id: String,
    pub platform: String,
    pub external_user_id: String,
    pub display_name: String,
    pub runtime_enabled: bool,
    pub auto_delivery_enabled: bool,
    pub auto_confirm_enabled: bool,
    pub monitor_since: Option<i64>,
    pub status: String,
    pub control_epoch: i64,
    pub credential_epoch: i64,
    pub version: i64,
}

/// 插入或按 (platform, external_user_id) 幂等返回现有账号;三开关默认关闭(FR-003)。
pub fn upsert_identity(
    conn: &Connection,
    id: &str,
    platform: &str,
    external_user_id: &str,
    display_name: &str,
) -> rusqlite::Result<AccountRow> {
    let existing = find_by_external(conn, platform, external_user_id)?;
    if let Some(row) = existing {
        return Ok(row);
    }
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO accounts(id, platform, external_user_id, display_name, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, platform, external_user_id, display_name, now],
    )?;
    get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<AccountRow>> {
    conn.query_row(
        "SELECT id, platform, external_user_id, display_name, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version
         FROM accounts WHERE id = ?1",
        params![id],
        row,
    )
    .optional()
}

pub fn find_by_external(
    conn: &Connection,
    platform: &str,
    external_user_id: &str,
) -> rusqlite::Result<Option<AccountRow>> {
    conn.query_row(
        "SELECT id, platform, external_user_id, display_name, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version
         FROM accounts WHERE platform = ?1 AND external_user_id = ?2",
        params![platform, external_user_id],
        row,
    )
    .optional()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: r.get(0)?,
        platform: r.get(1)?,
        external_user_id: r.get(2)?,
        display_name: r.get(3)?,
        runtime_enabled: r.get(4)?,
        auto_delivery_enabled: r.get(5)?,
        auto_confirm_enabled: r.get(6)?,
        monitor_since: r.get(7)?,
        status: r.get(8)?,
        control_epoch: r.get(9)?,
        credential_epoch: r.get(10)?,
        version: r.get(11)?,
    })
}

/// 更新运行开关与状态(版本化;monitor_since 只首次写入,暂停不重置,FR-003)。
pub fn set_control(
    conn: &Connection,
    id: &str,
    status: &str,
    runtime_enabled: Option<bool>,
    auto_delivery_enabled: Option<bool>,
    auto_confirm_enabled: Option<bool>,
    monitor_since: Option<i64>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE accounts SET
             status = ?2,
             runtime_enabled = COALESCE(?3, runtime_enabled),
             auto_delivery_enabled = COALESCE(?4, auto_delivery_enabled),
             auto_confirm_enabled = COALESCE(?5, auto_confirm_enabled),
             monitor_since = COALESCE(monitor_since, ?6),
             version = version + 1, updated_at = ?7
         WHERE id = ?1",
        params![
            id,
            status,
            runtime_enabled.map(|b| b as i64),
            auto_delivery_enabled.map(|b| b as i64),
            auto_confirm_enabled.map(|b| b as i64),
            monitor_since,
            utc_now_ms()
        ],
    )?;
    Ok(n == 1)
}

/// 恢复隔离:全部账号暂停(仅运行开关;不解除逐单隔离,US5 恢复流程调用)。
pub fn mark_all_paused_for_restore(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE accounts SET runtime_enabled = 0, version = version + 1, updated_at = ?1",
        params![utc_now_ms()],
    )?;
    Ok(())
}

/// 列表查询(T052:账号摘要,不含凭证等敏感列)。
pub fn list(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<AccountRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, platform, external_user_id, display_name, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version
         FROM accounts ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(AccountRow {
                id: r.get(0)?,
                platform: r.get(1)?,
                external_user_id: r.get(2)?,
                display_name: r.get(3)?,
                runtime_enabled: r.get::<_, i64>(4)? != 0,
                auto_delivery_enabled: r.get::<_, i64>(5)? != 0,
                auto_confirm_enabled: r.get::<_, i64>(6)? != 0,
                monitor_since: r.get(7)?,
                status: r.get(8)?,
                control_epoch: r.get(9)?,
                credential_epoch: r.get(10)?,
                version: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<AccountRow>>>()?;
    Ok(rows)
}
