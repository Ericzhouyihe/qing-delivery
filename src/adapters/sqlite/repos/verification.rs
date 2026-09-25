//! 003 验证审计仓储:只插入与回填终态,不删改历史(FR-010)。

use crate::domain::time_util::utc_now_ms;
use rusqlite::{Connection, params};

pub struct AttemptRow {
    pub id: String,
    pub account_id: String,
    pub trigger_source: String,
    pub trigger_reason: String,
    pub verification_url: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub outcome: String,
    pub duration_ms: Option<i64>,
    pub credential_updated: bool,
    pub failure_reason: Option<String>,
}

pub fn insert(
    conn: &Connection,
    id: &str,
    account_id: &str,
    trigger_source: &str,
    trigger_reason: &str,
    verification_url: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO verification_attempts
             (id, account_id, trigger_source, trigger_reason, verification_url, started_at, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'in_progress')",
        params![
            id,
            account_id,
            trigger_source,
            trigger_reason,
            verification_url,
            utc_now_ms()
        ],
    )?;
    Ok(())
}

/// 回填终态;只允许 in_progress → succeeded|failed_manual,终态行不可再改。
pub fn finish(
    conn: &Connection,
    id: &str,
    outcome: &str,
    credential_updated: bool,
    failure_reason: Option<&str>,
) -> rusqlite::Result<bool> {
    let now = utc_now_ms();
    let n = conn.execute(
        "UPDATE verification_attempts
         SET finished_at = ?2,
             duration_ms = ?2 - started_at,
             outcome = ?3,
             credential_updated = ?4,
             failure_reason = ?5
         WHERE id = ?1 AND outcome = 'in_progress'",
        params![id, now, outcome, credential_updated as i64, failure_reason],
    )?;
    Ok(n == 1)
}

pub fn list_recent(
    conn: &Connection,
    account_id: &str,
    limit: i64,
) -> rusqlite::Result<Vec<AttemptRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, trigger_source, trigger_reason, verification_url,
                started_at, finished_at, outcome, duration_ms, credential_updated, failure_reason
         FROM verification_attempts WHERE account_id = ?1
         ORDER BY started_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![account_id, limit], |r| {
            Ok(AttemptRow {
                id: r.get(0)?,
                account_id: r.get(1)?,
                trigger_source: r.get(2)?,
                trigger_reason: r.get(3)?,
                verification_url: r.get(4)?,
                started_at: r.get(5)?,
                finished_at: r.get(6)?,
                outcome: r.get(7)?,
                duration_ms: r.get(8)?,
                credential_updated: r.get::<_, i64>(9)? != 0,
                failure_reason: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        crate::adapters::sqlite::migrations::apply(&mut c, dir.path()).unwrap();
        c.execute(
            "INSERT INTO accounts (id, platform, external_user_id, display_name, created_at, updated_at)
             VALUES ('acct-1','xianyu','u1','测试',0,0)", [],
        ).unwrap();
        c
    }

    #[test]
    fn 只增不变量_终态不可改() {
        let c = conn();
        insert(&c, "vat-1", "acct-1", "mtop", "punish", Some("https://v")).unwrap();
        assert!(finish(&c, "vat-1", "succeeded", true, None).unwrap());
        // 终态再 finish 被拒
        assert!(!finish(&c, "vat-1", "failed_manual", false, Some("又失败")).unwrap());
        let rows = list_recent(&c, "acct-1", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "succeeded");
        assert!(rows[0].credential_updated);
        assert!(rows[0].duration_ms.is_some());
    }

    #[test]
    fn issues_metadata_增列可写读() {
        let c = conn();
        c.execute(
            "INSERT INTO issues (id, account_id, kind, reason_code, allowed_actions, state, created_at, metadata)
             VALUES ('iss-1','acct-1','security_verification','滑块未通过','[\"open_verification\"]','open',0,
                     '{\"verification_url\":\"https://x\"}')", [],
        ).unwrap();
        let meta: Option<String> = c
            .query_row("SELECT metadata FROM issues WHERE id='iss-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(meta.unwrap().contains("verification_url"));
    }
}
