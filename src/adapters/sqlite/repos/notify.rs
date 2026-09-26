//! 007 US5 通知仓储(T061):渠道 CRUD(config 明文 JSON/secrets 信封四件套/
//! 乐观锁 version)、账号绑定覆盖式 PUT(FK CASCADE)、投递留痕(channel_id
//! 可 NULL 保留审计)、system_settings 明文键与 system_secrets 信封键。
//! SQL 只在本目录(宪章 II);订阅过滤/绑定覆盖裁决在 domain/application;
//! 网络发送绝不进入本模块的调用事务。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::crypto::{self, Envelope};
use crate::domain::time_util::{format_rfc3339, utc_now_ms};

fn now_rfc3339() -> String {
    format_rfc3339(utc_now_ms())
}

// ---------- 渠道 ----------

#[derive(Clone, Debug)]
pub struct ChannelRow {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub config_json: String,
    pub secrets_ciphertext: Option<Vec<u8>>,
    pub secrets_nonce: Option<Vec<u8>>,
    pub secrets_key_id: Option<String>,
    pub secrets_format_version: Option<i64>,
    pub event_types_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

const CHANNEL_COLS: &str = "id, kind, name, enabled, config, secrets_ciphertext, secrets_nonce,
         secrets_key_id, secrets_format_version, event_types, version, created_at, updated_at";

fn channel_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChannelRow> {
    Ok(ChannelRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        config_json: r.get(4)?,
        secrets_ciphertext: r.get(5)?,
        secrets_nonce: r.get(6)?,
        secrets_key_id: r.get(7)?,
        secrets_format_version: r.get(8)?,
        event_types_json: r.get(9)?,
        version: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

pub fn insert_channel(
    conn: &Connection,
    id: &str,
    kind: &str,
    name: &str,
    enabled: bool,
    config_json: &str,
    secrets: Option<&Envelope>,
    event_types_json: &str,
) -> rusqlite::Result<ChannelRow> {
    let now = now_rfc3339();
    let (cipher, nonce, key_id, version) = envelope_parts(secrets);
    conn.execute(
        "INSERT INTO notification_channels(id, kind, name, enabled, config, secrets_ciphertext,
             secrets_nonce, secrets_key_id, secrets_format_version, event_types, version,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?11)",
        params![
            id,
            kind,
            name,
            enabled as i64,
            config_json,
            cipher,
            nonce,
            key_id,
            version,
            event_types_json,
            now
        ],
    )?;
    get_channel(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_channel(conn: &Connection, id: &str) -> rusqlite::Result<Option<ChannelRow>> {
    conn.query_row(
        &format!("SELECT {CHANNEL_COLS} FROM notification_channels WHERE id = ?1"),
        params![id],
        channel_of,
    )
    .optional()
}

pub fn list_channels(conn: &Connection) -> rusqlite::Result<Vec<ChannelRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHANNEL_COLS} FROM notification_channels ORDER BY created_at, id"
    ))?;
    let rows = stmt
        .query_map([], channel_of)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 启用渠道(投递扇出候选)。
pub fn list_enabled_channels(conn: &Connection) -> rusqlite::Result<Vec<ChannelRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHANNEL_COLS} FROM notification_channels
         WHERE enabled = 1 ORDER BY created_at, id"
    ))?;
    let rows = stmt
        .query_map([], channel_of)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 更新结果:未命中按当前版本区分 404 与乐观锁冲突。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelUpdate {
    Updated,
    NotFound,
    VersionConflict,
}

/// 更新渠道:version 乐观锁;secrets=None 表示保留既有信封(编辑留空=不修改)。
#[allow(clippy::too_many_arguments)]
pub fn update_channel(
    conn: &Connection,
    id: &str,
    name: &str,
    enabled: bool,
    config_json: &str,
    secrets: Option<&Envelope>,
    event_types_json: &str,
    expected_version: i64,
) -> rusqlite::Result<ChannelUpdate> {
    let Some(current) = get_channel(conn, id)? else {
        return Ok(ChannelUpdate::NotFound);
    };
    if current.version != expected_version {
        return Ok(ChannelUpdate::VersionConflict);
    }
    let (cipher, nonce, key_id, fmt_version) = match secrets {
        Some(env) => (
            Some(env.ciphertext.clone()),
            Some(env.nonce.as_slice().to_vec()),
            Some(env.key_id.clone()),
            Some(env.format_version as i64),
        ),
        // 留空保存:沿用既有信封四件套(整包不覆盖)
        None => (
            current.secrets_ciphertext.clone(),
            current.secrets_nonce.clone(),
            current.secrets_key_id.clone(),
            current.secrets_format_version,
        ),
    };
    let now = now_rfc3339();
    let updated = conn.execute(
        "UPDATE notification_channels SET name = ?2, enabled = ?3, config = ?4,
                secrets_ciphertext = ?5, secrets_nonce = ?6, secrets_key_id = ?7,
                secrets_format_version = ?8, event_types = ?9,
                version = version + 1, updated_at = ?10
         WHERE id = ?1 AND version = ?11",
        params![
            id,
            name,
            enabled as i64,
            config_json,
            cipher,
            nonce,
            key_id,
            fmt_version,
            event_types_json,
            now,
            expected_version
        ],
    )?;
    Ok(if updated == 1 {
        ChannelUpdate::Updated
    } else {
        ChannelUpdate::VersionConflict
    })
}

pub fn delete_channel(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM notification_channels WHERE id = ?1",
        params![id],
    )?;
    Ok(n > 0)
}

#[allow(clippy::type_complexity)]
fn envelope_parts(env: Option<&Envelope>) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>, Option<i64>) {
    match env {
        Some(e) => (
            Some(e.ciphertext.clone()),
            Some(e.nonce.as_slice().to_vec()),
            Some(e.key_id.clone()),
            Some(e.format_version as i64),
        ),
        None => (None, None, None, None),
    }
}

/// 行内信封 → 域信封(无秘密时 None)。
pub fn row_envelope(row: &ChannelRow) -> Option<Envelope> {
    let cipher = row.secrets_ciphertext.clone()?;
    let nonce = row.secrets_nonce.clone()?;
    let mut nonce_arr = [0u8; crypto::NONCE_LEN];
    nonce_arr.copy_from_slice(&nonce);
    Some(Envelope {
        ciphertext: cipher,
        nonce: nonce_arr,
        key_id: row.secrets_key_id.clone().unwrap_or_default(),
        format_version: row.secrets_format_version.unwrap_or(1) as u16,
    })
}

// ---------- 绑定(FR-052 覆盖式) ----------

/// 覆盖式绑定 PUT:先清空该账号全部绑定再写入新集合(单事务内,原子)。
pub fn put_bindings(
    conn: &Connection,
    account_id: &str,
    channel_ids: &[String],
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM notification_bindings WHERE account_id = ?1",
        params![account_id],
    )?;
    for cid in channel_ids {
        conn.execute(
            "INSERT INTO notification_bindings(account_id, channel_id) VALUES (?1, ?2)",
            params![account_id, cid],
        )?;
    }
    Ok(())
}

pub fn list_bindings(conn: &Connection, account_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT channel_id FROM notification_bindings WHERE account_id = ?1 ORDER BY channel_id",
    )?;
    let rows = stmt
        .query_map(params![account_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- 投递留痕(FR-054) ----------

#[derive(Clone, Debug)]
pub struct DeliveryRow {
    pub id: String,
    pub channel_id: Option<String>,
    pub event_kind: String,
    pub subject: String,
    pub state: String,
    pub error_hint: Option<String>,
    pub created_at: String,
}

pub fn insert_delivery(
    conn: &Connection,
    id: &str,
    channel_id: Option<&str>,
    event_kind: &str,
    subject: &str,
    state: &str,
    error_hint: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO notification_deliveries(id, channel_id, event_kind, subject, state,
             error_hint, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, channel_id, event_kind, subject, state, error_hint, now_rfc3339()],
    )?;
    Ok(())
}

/// 最近投递记录(新→旧;审计/前端展示)。
pub fn list_deliveries(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<DeliveryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel_id, event_kind, subject, state, error_hint, created_at
         FROM notification_deliveries ORDER BY created_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(DeliveryRow {
                id: r.get(0)?,
                channel_id: r.get(1)?,
                event_kind: r.get(2)?,
                subject: r.get(3)?,
                state: r.get(4)?,
                error_hint: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- 系统设置(明文)与系统秘密(信封) ----------

pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM system_settings WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
}

pub fn put_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO system_settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn list_settings(conn: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT key, value FROM system_settings ORDER BY key")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_secret_envelope(conn: &Connection, key: &str) -> rusqlite::Result<Option<Envelope>> {
    let row: Option<(Vec<u8>, Vec<u8>, String, i64)> = conn
        .query_row(
            "SELECT ciphertext, nonce, key_id, format_version FROM system_secrets WHERE key = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((ciphertext, nonce, key_id, format_version)) = row else {
        return Ok(None);
    };
    let mut nonce_arr = [0u8; crypto::NONCE_LEN];
    nonce_arr.copy_from_slice(&nonce);
    Ok(Some(Envelope {
        ciphertext,
        nonce: nonce_arr,
        key_id,
        format_version: format_version as u16,
    }))
}

pub fn put_secret_envelope(conn: &Connection, key: &str, envelope: &Envelope) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO system_secrets(key, ciphertext, nonce, key_id, format_version)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(key) DO UPDATE SET ciphertext = excluded.ciphertext,
             nonce = excluded.nonce, key_id = excluded.key_id,
             format_version = excluded.format_version",
        params![
            key,
            envelope.ciphertext,
            envelope.nonce.as_slice(),
            envelope.key_id,
            envelope.format_version as i64
        ],
    )?;
    Ok(())
}

pub fn list_secret_keys(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT key FROM system_secrets ORDER BY key")?;
    let rows = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
