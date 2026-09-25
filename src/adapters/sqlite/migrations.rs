//! 版本化迁移:schema_version 单向递增、事务化、迁移前受保护备份、失败中止启动。
//! 不支持降级;回退必须走备份恢复流程(data-model)。

use std::path::Path;

use rusqlite::{Connection, params};

use crate::domain::time_util::utc_now_ms;

/// (版本, SQL)按版本升序;新增迁移只追加,不修改历史条目。
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../../migrations/0001_init.sql")),
    (2, include_str!("../../../migrations/0002_verification.sql")),
    (
        3,
        include_str!("../../../migrations/0003_account_profile.sql"),
    ),
    (
        4,
        include_str!("../../../migrations/0004_cards_templates_rules.sql"),
    ),
];

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("迁移前备份失败:{0}")]
    Backup(String),
    #[error("数据库 schema 版本 {found} 高于程序支持的 {supported};不支持降级,请用备份恢复流程")]
    NewerThanSupported { found: i64, supported: i64 },
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
             version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);",
    )?;
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )
}

/// 应用全部待迁移;返回应用后的版本。
/// 已有数据(版本 ≥1)时,先在 backups/pre-migrate-v{target}.db 写一致快照。
pub fn apply(conn: &mut Connection, data_dir: &Path) -> Result<i64, MigrationError> {
    let current = current_version(conn)?;
    let supported = MIGRATIONS.last().map(|(v, _)| *v).unwrap_or(0);
    if current > supported {
        return Err(MigrationError::NewerThanSupported {
            found: current,
            supported,
        });
    }
    let pending: Vec<&(i64, &str)> = MIGRATIONS.iter().filter(|(v, _)| *v > current).collect();
    if pending.is_empty() {
        return Ok(current);
    }
    let target = pending.last().map(|(v, _)| *v).unwrap_or(current);
    if current >= 1 {
        let backup_dir = data_dir.join("backups");
        std::fs::create_dir_all(&backup_dir).map_err(|e| MigrationError::Backup(e.to_string()))?;
        let dst_path = backup_dir.join(format!("pre-migrate-v{target}.db"));
        let mut dst =
            Connection::open(&dst_path).map_err(|e| MigrationError::Backup(e.to_string()))?;
        let bk = rusqlite::backup::Backup::new(&*conn, &mut dst)
            .map_err(|e| MigrationError::Backup(e.to_string()))?;
        bk.run_to_completion(16, std::time::Duration::from_millis(1), None)
            .map_err(|e| MigrationError::Backup(e.to_string()))?;
        drop(bk);
        drop(dst);
    }
    for (version, sql) in pending {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![version, utc_now_ms()],
        )?;
        tx.commit()?;
    }
    // installation 单行(singleton):迁移幂等插入
    conn.execute(
        "INSERT OR IGNORE INTO installation(id, schema_version, created_at, restore_epoch)
         VALUES ('singleton', ?1, ?2, 0)",
        params![target, utc_now_ms()],
    )?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实体表总数(不含 sqlite 内部表与 schema_migrations)。
    /// 随迁移追加更新:T007(0004,+10)→35、T046(0005,+4)→39、T058(0006,+5)→44。
    const EXPECTED_TABLE_COUNT: i64 = 35;

    const ENTITY_TABLES: &[&str] = &[
        "installation",
        "admin",
        "sessions",
        "accounts",
        "account_credentials",
        "auth_flows",
        "items",
        "sync_jobs",
        "rules",
        "rule_contents",
        "orders",
        "order_facts",
        "inbound_events",
        "deliveries",
        "content_snapshots",
        "attempts",
        "delivery_proofs",
        "order_execution_guards",
        "issues",
        "manual_actions",
        "command_receipts",
        "operation_jobs",
        "restore_reviews",
        "backup_manifests",
        "verification_attempts",
        // 007 增量 1(0004,T007)
        "card_pools",
        "card_entries",
        "delivery_templates",
        "delivery_template_messages",
        "rule_variants",
        "reply_rules",
        "reply_rule_items",
        "default_replies",
        "default_reply_log",
        "review_reminder_state",
    ];

    #[test]
    fn fresh_apply_and_idempotence() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("m.db")).unwrap();
        let v1 = apply(&mut conn, dir.path()).unwrap();
        assert_eq!(v1, MIGRATIONS.last().unwrap().0, "应应用到最新版本");
        let v2 = apply(&mut conn, dir.path()).unwrap();
        assert_eq!(v2, v1, "重复应用应为空操作");
        // 已应用版本必须连续 1..=v1(缺号意味着迁移表被手工改动)
        let versions: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT version FROM schema_migrations ORDER BY version")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(
            versions,
            (1..=v1).collect::<Vec<_>>(),
            "schema_migrations 版本必须连续"
        );
        // 实体表清单逐一存在且数量与常量一致(新增迁移时同步两处)
        let placeholders = ENTITY_TABLES
            .iter()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(",");
        let tables: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ({placeholders})"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            tables,
            EXPECTED_TABLE_COUNT,
            "实体表数量应与 EXPECTED_TABLE_COUNT 一致"
        );
        assert_eq!(
            ENTITY_TABLES.len() as i64, EXPECTED_TABLE_COUNT,
            "清单长度与常量必须一致(新增表时同步)"
        );
    }
}
