//! 恢复(T080):仅停机、仅同机同用户;暂存校验 → 原目录保留副本 →
//! 恢复库内写 restore_epoch/全账号暂停/会话撤销/隔离核对记录 → 原子切换。

use std::path::Path;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use super::archive::{ArchiveManifest, read_archive};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("归档读取失败:{0}")]
    Archive(#[from] super::archive::ArchiveReadError),
    #[error("归档 schema 版本 {found} 高于程序支持 {supported};不支持降级")]
    NewerSchema { found: i64, supported: i64 },
    #[error("数据库完整性检查失败:{0}")]
    Integrity(String),
    #[error("恢复发布失败:{0}")]
    Publish(String),
    #[error("目标目录已有数据;需显式确认替换(当前实现要求空目录或使用新目录)")]
    TargetOccupied,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// 程序支持恢复的最高备份 schema 版本;随迁移追加同步(T007:0004 → 4)。
pub const SUPPORTED_SCHEMA: i64 = 4;

pub struct RestoreReport {
    pub manifest: ArchiveManifest,
    pub restore_epoch: i64,
    pub quarantined_tasks: usize,
    pub quarantined_orders: usize,
    pub accounts_paused: usize,
}

/// 执行恢复到 target_dir(必须为空或不存在;原数据保护由调用方管理)。
/// 返回隔离报告;恢复后所有账号暂停、会话撤销、逐单隔离生效。
pub fn restore_archive(
    archive_path: &Path,
    target_dir: &Path,
) -> Result<RestoreReport, RestoreError> {
    let (manifest, plain) = read_archive(archive_path)?;
    if manifest.schema_version > SUPPORTED_SCHEMA {
        return Err(RestoreError::NewerSchema {
            found: manifest.schema_version,
            supported: SUPPORTED_SCHEMA,
        });
    }
    std::fs::create_dir_all(target_dir)?;
    let db_path = target_dir.join("main.db");
    if db_path.exists() {
        return Err(RestoreError::TargetOccupied);
    }

    // 校验和再次确认 + 写入暂存
    if hex::encode(Sha256::digest(&plain)) != manifest.plaintext_sha256 {
        return Err(RestoreError::Archive(
            super::archive::ArchiveReadError::Checksum,
        ));
    }
    std::fs::write(&db_path, &plain)?;
    let conn = Connection::open(&db_path).map_err(|e| RestoreError::Integrity(e.to_string()))?;
    // quick_check + 迁移版本校验
    let ok: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| RestoreError::Integrity(e.to_string()))?;
    if ok != "ok" {
        return Err(RestoreError::Integrity(ok));
    }

    // 恢复库内事务:restore_epoch、全账号暂停、会话撤销、隔离记录
    let current_epoch: i64 = conn
        .query_row(
            "SELECT restore_epoch FROM installation WHERE id='singleton'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| RestoreError::Integrity(e.to_string()))?;
    let restore_epoch = current_epoch + 1;
    let snapshot_finished = manifest.snapshot_finished_at;
    let now = utc_now_ms();
    conn.execute(
        "UPDATE installation SET restore_epoch = ?1, quarantine_started_at = ?2,
                restore_manifest_id = ?3 WHERE id='singleton'",
        rusqlite::params![restore_epoch, now, archive_path.to_string_lossy()],
    )
    .map_err(|e| RestoreError::Publish(e.to_string()))?;
    let accounts_paused = conn
        .execute("UPDATE accounts SET runtime_enabled = 0", [])
        .map_err(|e| RestoreError::Publish(e.to_string()))?;
    conn.execute(
        "UPDATE sessions SET revoked_at = ?1 WHERE revoked_at IS NULL",
        [now],
    )
    .map_err(|e| RestoreError::Publish(e.to_string()))?;

    // 隔离范围 1:快照中未完/未知任务(data-model:即使订单创建早于备份也不能绕过)
    let unfinished: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT d.id, o.id FROM deliveries d
             LEFT JOIN orders o ON o.id = d.order_id
             WHERE d.content_state IN ('pending_verification','queued','dispatching','unknown')
                OR (d.content_state = 'not_sent' AND d.review_state = 'required')",
        )?;

        stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    let mut quarantined_tasks = 0;
    for (delivery, order) in &unfinished {
        conn.execute(
            "INSERT INTO restore_reviews(id, restore_epoch, order_id, account_id, kind, status)
             VALUES (?1, ?2, ?3, (SELECT account_id FROM orders WHERE id = ?3),
                     'unfinished_snapshot', 'unresolved')",
            rusqlite::params![ids::new_id("rrv"), restore_epoch, order],
        )?;
        // 旧自动路径终结资格:manual_only 门槛
        conn.execute(
            "UPDATE deliveries SET execution_policy = 'manual_only' WHERE id = ?1",
            [delivery],
        )?;
        quarantined_tasks += 1;
    }

    // 隔离范围 2:快照完成到恢复完成之间可能发生交付的订单区间
    let uncertain: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT o.id FROM orders o
             WHERE o.paid_at IS NOT NULL AND o.paid_at >= ?1
               AND o.platform_status = 'pending_ship'",
        )?;

        stmt.query_map([snapshot_finished], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut quarantined_orders = 0;
    for order in &uncertain {
        conn.execute(
            "INSERT INTO restore_reviews(id, restore_epoch, order_id, account_id, kind, status)
             VALUES (?1, ?2, ?3, (SELECT account_id FROM orders WHERE id = ?3),
                     'uncertain_interval', 'unresolved')",
            rusqlite::params![ids::new_id("rrv"), restore_epoch, order],
        )?;
        quarantined_orders += 1;
    }

    // 数据密钥:归档携带的 DPAPI 包装密钥写回目录(同机同用户可解)
    std::fs::write(target_dir.join("data-key.bin"), &manifest.data_key_blob)?;
    // profile 标记沿用 live
    std::fs::write(target_dir.join("profile.txt"), "live")?;
    drop(conn);

    Ok(RestoreReport {
        manifest,
        restore_epoch,
        quarantined_tasks,
        quarantined_orders,
        accounts_paused,
    })
}

#[cfg(test)]
mod tests {
    use super::super::archive::create_backup;
    use super::*;
    use crate::adapters::sqlite::migrations;
    use crate::adapters::sqlite::repos::accounts;
    use crate::adapters::windows::datadir::DataDir;
    use crate::adapters::windows::keys;

    fn roundtrip_env(tag: &str) -> (tempfile::TempDir, DataDir) {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::resolve(Some(dir.path())).unwrap();
        keys::load_or_create(&data.root).unwrap();
        // 直接同步建库(测试内不开线程)
        let mut conn = Connection::open(data.join("main.db")).unwrap();
        migrations::apply(&mut conn, &data.root).unwrap();
        drop(conn);
        let _ = tag;
        (dir, data)
    }

    #[test]
    fn backup_then_restore_to_fresh_dir_quarantines() {
        let (_guard, data) = roundtrip_env("a");
        // 种子:一个在线账号 + 一条未知交付(应进入隔离)
        let conn = Connection::open(data.join("main.db")).unwrap();
        accounts::upsert_identity(&conn, "acct-1", "xianyu", "s1", "卖家").unwrap();
        accounts::set_control(
            &conn,
            "acct-1",
            "online",
            Some(true),
            Some(true),
            None,
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO orders(id, platform, account_id, external_order_id, paid_at,
                 platform_status, created_at, updated_at)
             VALUES ('ord-1','xianyu','acct-1','EXT-1', 1, 'pending_ship', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO deliveries(id, order_id, kind, content_state, created_at, updated_at)
             VALUES ('dlv-1','ord-1','initial','unknown',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO admin(id, username, password_hash, created_at, password_changed_at)
             VALUES ('adm-1','admin','h',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions(token_hash, admin_id, csrf_hash, created_at, expires_at)
             VALUES ('t','adm-1','c',1,99999999999)",
            [],
        )
        .unwrap();
        drop(conn);

        let out = _guard.path().join("backup.qdbak");
        let manifest = create_backup(&data.root, &out).unwrap();
        assert_eq!(manifest.schema_version, 4);
        assert!(out.exists());

        // 恢复到新目录
        let target = tempfile::tempdir().unwrap();
        let report = restore_archive(&out, target.path()).unwrap();
        assert_eq!(report.accounts_paused, 1, "全部账号暂停");
        assert_eq!(report.quarantined_tasks, 1, "未知任务进入隔离");
        let conn = Connection::open(target.path().join("main.db")).unwrap();
        let epoch: i64 = conn
            .query_row(
                "SELECT restore_epoch FROM installation WHERE id='singleton'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(epoch, report.restore_epoch);
        let reviews: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM restore_reviews WHERE status='unresolved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(reviews >= 1);
        let enabled: i64 = conn
            .query_row(
                "SELECT runtime_enabled FROM accounts WHERE id='acct-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);
        let revoked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE revoked_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revoked, 0, "恢复后全部会话撤销");
        let policy: String = conn
            .query_row(
                "SELECT execution_policy FROM deliveries WHERE id='dlv-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(policy, "manual_only", "旧自动路径被 manual_only 门槛阻断");
    }

    #[test]
    fn occupied_target_refused_and_checksum_tamper_detected() {
        let (_guard, data) = roundtrip_env("b");
        let out = _guard.path().join("b.qdbak");
        create_backup(&data.root, &out).unwrap();
        // 目标已有数据
        let target = tempfile::tempdir().unwrap();
        std::fs::write(target.path().join("main.db"), b"existing").unwrap();
        assert!(matches!(
            restore_archive(&out, target.path()),
            Err(RestoreError::TargetOccupied)
        ));
        // 篡改归档尾部 → 校验/解密失败
        let mut raw = std::fs::read(&out).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        let tampered = target.path().join("tampered.qdbak");
        std::fs::write(&tampered, raw).unwrap();
        assert!(read_archive(&tampered).is_err());
    }
}
