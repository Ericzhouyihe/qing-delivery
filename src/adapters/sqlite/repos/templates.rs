//! delivery_templates/delivery_template_messages 仓储(T023):
//! 模板 CRUD(更新=消息整体替换:删旧插新,同事务)、used_by 引用计数
//! (rules.template_id + rule_variants.template_id 两处)、被引用删除拒绝
//! (引用规则标记 needs_reconfiguration=1,同 US1 卡组语义)。
//! 消息体为明文占位符文本(data-model 约定:不含机密,无信封)。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::{format_rfc3339, utc_now_ms};

fn now_rfc3339() -> String {
    format_rfc3339(utc_now_ms())
}

pub struct TemplateRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

const TPL_COLS: &str = "id, name, enabled, version, created_at, updated_at";

fn tpl_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TemplateRow> {
    Ok(TemplateRow {
        id: r.get(0)?,
        name: r.get(1)?,
        enabled: r.get::<_, i64>(2)? != 0,
        version: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn insert_messages(conn: &Connection, template_id: &str, messages: &[String]) -> rusqlite::Result<()> {
    for (i, body) in messages.iter().enumerate() {
        conn.execute(
            "INSERT INTO delivery_template_messages(template_id, position, body)
             VALUES (?1, ?2, ?3)",
            params![template_id, (i + 1) as i64, body],
        )?;
    }
    Ok(())
}

pub fn insert_template(
    conn: &Connection,
    id: &str,
    name: &str,
    enabled: bool,
    messages: &[String],
) -> rusqlite::Result<TemplateRow> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO delivery_templates(id, name, enabled, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?4)",
        params![id, name, enabled as i64, now],
    )?;
    insert_messages(conn, id, messages)?;
    get_template(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_template(conn: &Connection, id: &str) -> rusqlite::Result<Option<TemplateRow>> {
    conn.query_row(
        &format!("SELECT {TPL_COLS} FROM delivery_templates WHERE id = ?1"),
        params![id],
        tpl_row,
    )
    .optional()
}

/// 消息列表(按 position 升序;渲染与 DTO 共用)。
pub fn list_messages(conn: &Connection, template_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT body FROM delivery_template_messages WHERE template_id = ?1 ORDER BY position",
    )?;
    let rows = stmt
        .query_map(params![template_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// LIKE 通配符转义(搜索只按字面包含)。
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn list_templates(
    conn: &Connection,
    search: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<TemplateRow>> {
    let mut sql = format!("SELECT {TPL_COLS} FROM delivery_templates WHERE 1 = 1");
    let mut binds: Vec<&dyn rusqlite::ToSql> = Vec::new();
    let search_owned;
    if let Some(s) = search {
        sql.push_str(" AND name LIKE '%' || ? || '%' ESCAPE '\\'");
        search_owned = like_escape(s);
        binds.push(&search_owned);
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    binds.push(&limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(binds.as_slice())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(tpl_row(r)?);
    }
    Ok(out)
}

/// 版本化更新;expected_version 不匹配返回 None。
/// messages = Some → 整体替换消息列表(删旧插新,与模板行更新同事务)。
pub fn update_template(
    conn: &mut Connection,
    id: &str,
    expected_version: i64,
    name: Option<&str>,
    enabled: Option<bool>,
    messages: Option<&[String]>,
) -> rusqlite::Result<Option<TemplateRow>> {
    let tx = conn.transaction()?;
    let n = tx.execute(
        "UPDATE delivery_templates SET
             name = COALESCE(?2, name),
             enabled = COALESCE(?3, enabled),
             version = version + 1, updated_at = ?4
         WHERE id = ?1 AND version = ?5",
        params![id, name, enabled.map(|b| b as i64), now_rfc3339(), expected_version],
    )?;
    if n == 0 {
        // 版本不匹配:事务无变更,直接放弃
        return Ok(None);
    }
    if let Some(messages) = messages {
        tx.execute(
            "DELETE FROM delivery_template_messages WHERE template_id = ?1",
            params![id],
        )?;
        insert_messages(&tx, id, messages)?;
    }
    let row = get_template(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    tx.commit()?;
    Ok(Some(row))
}

/// 引用该模板的规则 id 清单(去重):rules.template_id 直引 +
/// rule_variants.template_id 变体引(取其 rule_id)。
pub fn referencing_rules(conn: &Connection, template_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut ids: Vec<String> = Vec::new();
    let mut stmt = conn.prepare("SELECT id FROM rules WHERE template_id = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map(params![template_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend(rows);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT rule_id FROM rule_variants WHERE template_id = ?1 ORDER BY rule_id",
    )?;
    let rows = stmt
        .query_map(params![template_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for rule_id in rows {
        if !ids.contains(&rule_id) {
            ids.push(rule_id);
        }
    }
    Ok(ids)
}

/// 删除结果:被规则/变体引用时拒绝(应用层转 409 referenced_resource)。
#[derive(Debug, PartialEq)]
pub enum DeleteTemplateOutcome {
    Deleted,
    Referenced { rule_ids: Vec<String> },
}

/// 删除模板:被 rules.template_id / rule_variants.template_id 引用时拒绝,
/// 并把引用规则置 needs_reconfiguration=1(同 US1 卡组语义:规则页据此警示)。
/// 未被引用时删除(消息行随外键级联清理)。
pub fn delete_template(conn: &Connection, id: &str) -> rusqlite::Result<DeleteTemplateOutcome> {
    let rule_ids = referencing_rules(conn, id)?;
    if !rule_ids.is_empty() {
        let now = now_rfc3339();
        for rule_id in &rule_ids {
            conn.execute(
                "UPDATE rules SET needs_reconfiguration = 1, version = version + 1,
                        updated_at = ?2 WHERE id = ?1",
                params![rule_id, now],
            )?;
        }
        return Ok(DeleteTemplateOutcome::Referenced { rule_ids });
    }
    conn.execute("DELETE FROM delivery_templates WHERE id = ?1", params![id])?;
    Ok(DeleteTemplateOutcome::Deleted)
}
