//! operation_jobs 表仓储:HTTP 的 202 任务句柄(data-model)。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

#[derive(Debug)]
pub struct JobRow {
    pub id: String,
    pub kind: String,
    pub target_id: Option<String>,
    pub state: String,
    pub stage: String,
    pub version: i64,
    pub cancel_requested: bool,
    pub result_ref: Option<String>,
    pub safe_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        target_id: r.get(2)?,
        state: r.get(3)?,
        stage: r.get(4)?,
        version: r.get(5)?,
        cancel_requested: r.get(6)?,
        result_ref: r.get(7)?,
        safe_error: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

const COLS: &str = "id, kind, target_id, state, stage, version, cancel_requested, result_ref, safe_error, created_at, updated_at";

pub fn insert(
    conn: &Connection,
    id: &str,
    kind: &str,
    target_id: Option<&str>,
) -> rusqlite::Result<JobRow> {
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO operation_jobs(id, kind, target_id, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', ?4, ?4)",
        params![id, kind, target_id, now],
    )?;
    get(conn, id)?.ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<JobRow>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM operation_jobs WHERE id = ?1"),
        params![id],
        row,
    )
    .optional()
}

/// CAS 状态迁移:仅当版本匹配时更新状态并递增版本。
pub fn compare_and_set(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    new_state: &str,
    stage: &str,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE operation_jobs SET state = ?1, stage = ?2, version = version + 1, updated_at = ?3
         WHERE id = ?4 AND version = ?5",
        params![new_state, stage, utc_now_ms(), id, expected_version],
    )?;
    Ok(n == 1)
}

/// 标记请求取消(不改变状态机;取消语义由执行器解释,契约 §4)。
pub fn request_cancel(
    conn: &Connection,
    id: &str,
    expected_version: i64,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE operation_jobs SET cancel_requested = 1, version = version + 1, updated_at = ?1
         WHERE id = ?2 AND version = ?3",
        params![utc_now_ms(), id, expected_version],
    )?;
    Ok(n == 1)
}

pub fn finish(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    state: &str,
    result_ref: Option<&str>,
    safe_error: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE operation_jobs SET state = ?1, version = version + 1, result_ref = ?2,
                safe_error = ?3, updated_at = ?4
         WHERE id = ?5 AND version = ?6",
        params![
            state,
            result_ref,
            safe_error,
            utc_now_ms(),
            id,
            expected_version
        ],
    )?;
    Ok(n == 1)
}
