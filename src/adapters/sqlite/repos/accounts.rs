//! accounts 表仓储:账号身份、开关与代次。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct AccountRow {
    pub id: String,
    /// 004:头像外链(空=首字占位)与本地备注(不同步平台)
    pub avatar_url: Option<String>,
    pub remark: Option<String>,
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
    /// 007 US7:账号级 AI 自动回复开关与提示词(默认关;提示词明文,非机密)。
    pub ai_reply_enabled: bool,
    pub ai_prompt: Option<String>,
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
        "SELECT id, platform, external_user_id, display_name, avatar_url, remark, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version, ai_reply_enabled, ai_prompt
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
        "SELECT id, platform, external_user_id, display_name, avatar_url, remark, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version, ai_reply_enabled, ai_prompt
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
        avatar_url: r.get(4)?,
        remark: r.get(5)?,
        runtime_enabled: r.get(6)?,
        auto_delivery_enabled: r.get(7)?,
        auto_confirm_enabled: r.get(8)?,
        monitor_since: r.get(9)?,
        status: r.get(10)?,
        control_epoch: r.get(11)?,
        credential_epoch: r.get(12)?,
        version: r.get(13)?,
        ai_reply_enabled: r.get::<_, i64>(14)? != 0,
        ai_prompt: r.get(15)?,
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
        "SELECT id, platform, external_user_id, display_name, avatar_url, remark, runtime_enabled,
                auto_delivery_enabled, auto_confirm_enabled, monitor_since, status,
                control_epoch, credential_epoch, version, ai_reply_enabled, ai_prompt
         FROM accounts WHERE status != 'deleted' ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(AccountRow {
                id: r.get(0)?,
                platform: r.get(1)?,
                external_user_id: r.get(2)?,
                display_name: r.get(3)?,
                avatar_url: r.get(4)?,
                remark: r.get(5)?,
                runtime_enabled: r.get::<_, i64>(6)? != 0,
                auto_delivery_enabled: r.get::<_, i64>(7)? != 0,
                auto_confirm_enabled: r.get::<_, i64>(8)? != 0,
                monitor_since: r.get(9)?,
                status: r.get(10)?,
                control_epoch: r.get(11)?,
                credential_epoch: r.get(12)?,
                version: r.get(13)?,
                ai_reply_enabled: r.get::<_, i64>(14)? != 0,
                ai_prompt: r.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<AccountRow>>>()?;
    Ok(rows)
}

/// 004:资料落库(空昵称不覆盖旧值;头像可清空)。
pub fn set_profile(
    conn: &Connection,
    id: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE accounts SET
            display_name = COALESCE(?2, display_name),
            avatar_url = ?3,
            version = version + 1,
            updated_at = ?4
         WHERE id = ?1",
        rusqlite::params![
            id,
            display_name,
            avatar_url,
            crate::domain::time_util::utc_now_ms()
        ],
    )?;
    Ok(n == 1)
}

/// 004:本地备注(≤64 字符由调用方校验;NULL=清空)。
pub fn set_remark(conn: &Connection, id: &str, remark: Option<&str>) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE accounts SET remark = ?2, version = version + 1, updated_at = ?3
         WHERE id = ?1",
        rusqlite::params![id, remark, crate::domain::time_util::utc_now_ms()],
    )?;
    Ok(n == 1)
}

// ---------- 007 US7:账号级 AI 自动回复(FR-071) ----------

/// 账号 AI 设置(0006 迁移列;None=账号不存在)。
pub fn get_ai_settings(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<(bool, Option<String>)>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT ai_reply_enabled, ai_prompt FROM accounts WHERE id = ?1",
        rusqlite::params![id],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
    )
    .optional()
}

/// 写账号 AI 开关/提示词(prompt 由调用方校验长度;NULL=清空)。
/// 返回 false=账号不存在。
pub fn set_ai_settings(
    conn: &Connection,
    id: &str,
    enabled: bool,
    prompt: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE accounts SET ai_reply_enabled = ?2, ai_prompt = ?3, version = version + 1,
             updated_at = ?4
         WHERE id = ?1",
        rusqlite::params![id, enabled as i64, prompt, crate::domain::time_util::utc_now_ms()],
    )?;
    Ok(n == 1)
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn 资料与备注_空昵称不覆盖() {
        let mut conn = Connection::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        crate::adapters::sqlite::migrations::apply(&mut conn, dir.path()).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, platform, external_user_id, display_name, created_at, updated_at)
             VALUES ('acct-1','xianyu','u1','旧昵称',0,0)", [],
        ).unwrap();
        // 空昵称保留旧值,头像写入
        assert!(set_profile(&conn, "acct-1", None, Some("https://img/a.png")).unwrap());
        let row = get(&conn, "acct-1").unwrap().unwrap();
        assert_eq!(row.display_name, "旧昵称");
        assert_eq!(row.avatar_url.as_deref(), Some("https://img/a.png"));
        // 非空昵称覆盖
        assert!(set_profile(&conn, "acct-1", Some("新昵称"), Some("https://img/b.png")).unwrap());
        let row = get(&conn, "acct-1").unwrap().unwrap();
        assert_eq!(row.display_name, "新昵称");
        // 备注:写入/清空
        assert!(set_remark(&conn, "acct-1", Some("主号")).unwrap());
        assert_eq!(
            get(&conn, "acct-1").unwrap().unwrap().remark.as_deref(),
            Some("主号")
        );
        assert!(set_remark(&conn, "acct-1", None).unwrap());
        assert_eq!(get(&conn, "acct-1").unwrap().unwrap().remark, None);
    }
}
