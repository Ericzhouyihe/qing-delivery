//! 007 US4 在线聊天仓储(T049):会话 upsert/列表、消息落库与游标分页、
//! 出站状态更新、未读清零、隐藏会话(物理清空展示消息)、快捷回复(≤50)、
//! 买家备注与未读汇总。SQL 只在本目录(宪章 II);
//! 未读聚合规则与出站状态机裁决在 domain::chat。
//! chat body 明文存储是 research D15 显式接受的权衡。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::chat::ChatMsgKind;
use crate::domain::time_util::{format_rfc3339, utc_now_ms};

fn now_rfc3339() -> String {
    format_rfc3339(utc_now_ms())
}

// ---------- 行类型 ----------

#[derive(Clone, Debug)]
pub struct ConversationRow {
    pub id: String,
    pub account_id: String,
    pub peer_buyer_id: String,
    pub peer_nickname: Option<String>,
    pub chat_id: Option<String>,
    pub last_item_id: Option<String>,
    pub last_message_at: Option<String>,
    pub last_message_preview: Option<String>,
    pub unread_count: i64,
    pub hidden_at: Option<String>,
    pub created_at: String,
}

const CONVERSATION_COLS: &str = "id, account_id, peer_buyer_id, peer_nickname, chat_id,
         last_item_id, last_message_at, last_message_preview, unread_count, hidden_at, created_at";

fn conversation_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: r.get(0)?,
        account_id: r.get(1)?,
        peer_buyer_id: r.get(2)?,
        peer_nickname: r.get(3)?,
        chat_id: r.get(4)?,
        last_item_id: r.get(5)?,
        last_message_at: r.get(6)?,
        last_message_preview: r.get(7)?,
        unread_count: r.get(8)?,
        hidden_at: r.get(9)?,
        created_at: r.get(10)?,
    })
}

#[derive(Clone, Debug)]
pub struct MessageRow {
    pub id: String,
    pub conversation_id: String,
    pub direction: String,
    pub msg_kind: String,
    pub body_text: Option<String>,
    pub image_path: Option<String>,
    pub status: Option<String>,
    pub platform_message_id: Option<String>,
    pub request_key: Option<String>,
    pub error_hint: Option<String>,
    pub item_snapshot: Option<String>,
    pub created_at: String,
    pub read_at: Option<String>,
}

const MESSAGE_COLS: &str = "id, conversation_id, direction, msg_kind, body_text, image_path,
         status, platform_message_id, request_key, error_hint, item_snapshot, created_at, read_at";

fn message_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        direction: r.get(2)?,
        msg_kind: r.get(3)?,
        body_text: r.get(4)?,
        image_path: r.get(5)?,
        status: r.get(6)?,
        platform_message_id: r.get(7)?,
        request_key: r.get(8)?,
        error_hint: r.get(9)?,
        item_snapshot: r.get(10)?,
        created_at: r.get(11)?,
        read_at: r.get(12)?,
    })
}

// ---------- 会话 ----------

/// 会话 upsert:按 (account_id, peer_buyer_id) 唯一;不存在则建,
/// 已存在时补全 chat_id / peer_nickname(平台携带新值时)。
/// 买家号归一(@goofish 后缀)由调用方经 domain::chat::conversation_key 语义保证——
/// 存储层直接存入参原文,唯一索引按原文判重。
pub fn upsert_conversation(
    conn: &Connection,
    new_id: &str,
    account_id: &str,
    peer_buyer_id: &str,
    chat_id: Option<&str>,
    peer_nickname: Option<&str>,
) -> rusqlite::Result<ConversationRow> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO conversations(id, account_id, peer_buyer_id, peer_nickname, chat_id,
             unread_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)
         ON CONFLICT(account_id, peer_buyer_id) DO UPDATE SET
             chat_id = COALESCE(excluded.chat_id, conversations.chat_id),
             peer_nickname = COALESCE(excluded.peer_nickname, conversations.peer_nickname),
             updated_at = excluded.updated_at",
        params![new_id, account_id, peer_buyer_id, peer_nickname, chat_id, now],
    )?;
    find_conversation_by_peer(conn, account_id, peer_buyer_id)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn find_conversation(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<ConversationRow>> {
    conn.query_row(
        &format!("SELECT {CONVERSATION_COLS} FROM conversations WHERE id = ?1"),
        params![id],
        conversation_of,
    )
    .optional()
}

pub fn find_conversation_by_peer(
    conn: &Connection,
    account_id: &str,
    peer_buyer_id: &str,
) -> rusqlite::Result<Option<ConversationRow>> {
    conn.query_row(
        &format!(
            "SELECT {CONVERSATION_COLS} FROM conversations
             WHERE account_id = ?1 AND peer_buyer_id = ?2"
        ),
        params![account_id, peer_buyer_id],
        conversation_of,
    )
    .optional()
}

/// 入站消息落库后的会话摘要推进:last_* 更新 + 未读(新值由调用方按
/// domain::chat::unread_after 裁决后传入)。与消息插入在同一 DbThread
/// 闭包内调用(单连接串行,无竞态)。
pub fn apply_incoming_summary(
    conn: &Connection,
    conversation_id: &str,
    preview: &str,
    item_id: Option<&str>,
    unread_now: i64,
) -> rusqlite::Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE conversations SET last_message_at = ?2, last_message_preview = ?3,
                last_item_id = COALESCE(?4, last_item_id),
                unread_count = ?5, updated_at = ?2
         WHERE id = ?1",
        params![conversation_id, now, preview, item_id, unread_now],
    )?;
    Ok(())
}

/// 出站消息落库后的会话摘要推进(不改变未读)。
pub fn apply_outgoing_summary(
    conn: &Connection,
    conversation_id: &str,
    preview: &str,
) -> rusqlite::Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE conversations SET last_message_at = ?2, last_message_preview = ?3, updated_at = ?2
         WHERE id = ?1",
        params![conversation_id, now, preview],
    )?;
    Ok(())
}

/// 会话列表分页(新→旧):hidden_at IS NULL;search 匹配买家号/昵称/摘要;
/// unread_only 只看未读。cursor = "last_message_at|created_at|id"。
#[allow(clippy::type_complexity)]
pub fn list_conversations(
    conn: &Connection,
    account_id: &str,
    search: Option<&str>,
    unread_only: bool,
    cursor: Option<(&str, &str, &str)>,
    limit: i64,
) -> rusqlite::Result<(Vec<ConversationRow>, Option<String>)> {
    let mut sql = format!(
        "SELECT {CONVERSATION_COLS} FROM conversations
         WHERE account_id = ? AND hidden_at IS NULL"
    );
    let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&account_id];
    let search_owned;
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" AND (peer_buyer_id LIKE ? OR peer_nickname LIKE ? OR last_message_preview LIKE ?)");
        search_owned = format!("%{}%", q.trim());
        binds.push(&search_owned);
        binds.push(&search_owned);
        binds.push(&search_owned);
    }
    if unread_only {
        sql.push_str(" AND unread_count > 0");
    }
    let (cur_sort, cur_created, cur_id);
    if let Some((s, c, i)) = cursor {
        // last_message_at 可能为 NULL(无消息的新会话):NULL 排在最后,
        // 以 COALESCE(last_message_at, created_at) 为排序键
        sql.push_str(
            " AND (COALESCE(last_message_at, created_at), created_at, id) < (?, ?, ?)",
        );
        cur_sort = s.to_string();
        cur_created = c.to_string();
        cur_id = i.to_string();
        binds.push(&cur_sort);
        binds.push(&cur_created);
        binds.push(&cur_id);
    }
    let fetch_limit = limit + 1;
    sql.push_str(
        " ORDER BY COALESCE(last_message_at, created_at) DESC, created_at DESC, id DESC LIMIT ?",
    );
    binds.push(&fetch_limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(binds.as_slice())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(conversation_of(r)?);
    }
    let next_cursor = if out.len() as i64 > limit {
        out.pop();
        out.last().map(|last| {
            format!(
                "{}|{}|{}",
                last.last_message_at.clone().unwrap_or_else(|| last.created_at.clone()),
                last.created_at,
                last.id
            )
        })
    } else {
        None
    };
    Ok((out, next_cursor))
}

/// 未读清零:会话 unread_count=0,入站未读消息补 read_at(FR-043 打开会话后清零)。
pub fn mark_conversation_read(conn: &Connection, conversation_id: &str) -> rusqlite::Result<i64> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE chat_messages SET read_at = ?2
         WHERE conversation_id = ?1 AND direction = 'in' AND read_at IS NULL",
        params![conversation_id, now],
    )?;
    let cleared = conn.execute(
        "UPDATE conversations SET unread_count = 0, updated_at = ?2 WHERE id = ?1",
        params![conversation_id, now],
    )?;
    Ok(cleared as i64)
}

/// 删除会话 = 本机隐藏 + 物理清空该会话展示消息(FR-045;平台侧不受影响)。
/// 未读清零、摘要清空;hidden_at 置位后列表不再可见。
pub fn hide_conversation(conn: &Connection, conversation_id: &str) -> rusqlite::Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "DELETE FROM chat_messages WHERE conversation_id = ?1",
        params![conversation_id],
    )?;
    conn.execute(
        "UPDATE conversations SET hidden_at = ?2, unread_count = 0,
                last_message_at = NULL, last_message_preview = NULL, updated_at = ?2
         WHERE id = ?1",
        params![conversation_id, now],
    )?;
    Ok(())
}

// ---------- 消息 ----------

/// 入站消息(platform_message_id 去重;重复返回 None)。
/// 返回已插入行与未读新值(未读聚合按 domain::chat::unread_after)。
pub struct IncomingMessage<'a> {
    pub id: &'a str,
    pub conversation_id: &'a str,
    pub platform_message_id: &'a str,
    pub kind: ChatMsgKind,
    pub body_text: Option<&'a str>,
    pub image_url: Option<&'a str>,
    pub item_snapshot: Option<&'a str>,
    pub created_at: &'a str,
}

/// 入站去重:同一 platform_message_id 已存在则不重复落库(FR-046 双路径防重)。
pub fn platform_message_seen(
    conn: &Connection,
    platform_message_id: &str,
) -> rusqlite::Result<bool> {
    let seen: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM chat_messages WHERE platform_message_id = ?1 LIMIT 1",
            params![platform_message_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(seen.is_some())
}

/// 插入入站消息行(调用方先经 platform_message_seen 判重;同闭包内串行安全)。
pub fn insert_incoming(conn: &Connection, m: &IncomingMessage<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO chat_messages(id, conversation_id, direction, msg_kind, body_text,
             image_path, platform_message_id, item_snapshot, created_at)
         VALUES (?1, ?2, 'in', ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            m.id,
            m.conversation_id,
            m.kind.as_str(),
            m.body_text,
            m.image_url,
            m.platform_message_id,
            m.item_snapshot,
            m.created_at,
        ],
    )?;
    Ok(())
}

/// 插入出站 sending 行(发送前持久化;幂等键 request_key 关联回执回填)。
pub fn insert_outgoing(
    conn: &Connection,
    id: &str,
    conversation_id: &str,
    kind: ChatMsgKind,
    body_text: Option<&str>,
    image_path: Option<&str>,
    request_key: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO chat_messages(id, conversation_id, direction, msg_kind, body_text,
             image_path, status, request_key, created_at)
         VALUES (?1, ?2, 'out', ?3, ?4, ?5, 'sending', ?6, ?7)",
        params![
            id,
            conversation_id,
            kind.as_str(),
            body_text,
            image_path,
            request_key,
            now_rfc3339(),
        ],
    )?;
    Ok(())
}

/// 出站结果落库:状态迁移(sending→sent|failed|uncertain;域层已裁决)、
/// platform_message_id 回填、request_key 换绑适配器请求 ID(Accepted 后,
/// 后续 OutgoingMessageEvidence 以该 ID 关联)。
pub fn update_outgoing_result(
    conn: &Connection,
    id: &str,
    status: &str,
    platform_message_id: Option<&str>,
    error_hint: Option<&str>,
    request_key: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE chat_messages SET status = ?2, platform_message_id = COALESCE(?3, platform_message_id),
                error_hint = ?4,
                request_key = COALESCE(?5, request_key)
         WHERE id = ?1",
        params![id, status, platform_message_id, error_hint, request_key],
    )?;
    Ok(())
}

/// 消息游标分页(新→旧):before_id 起往更旧方向;cursor 取首行 id。
pub fn list_messages(
    conn: &Connection,
    conversation_id: &str,
    before_id: Option<&str>,
    limit: i64,
) -> rusqlite::Result<(Vec<MessageRow>, Option<String>)> {
    let mut sql = format!(
        "SELECT {MESSAGE_COLS} FROM chat_messages WHERE conversation_id = ?"
    );
    let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&conversation_id];
    // chat_messages.id 为时间有序 UUID:字典序即时间序,before_id 直接比较
    let before_owned;
    if let Some(b) = before_id.filter(|s| !s.is_empty()) {
        sql.push_str(" AND id < ?");
        before_owned = b.to_string();
        binds.push(&before_owned);
    }
    let fetch_limit = limit + 1;
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    binds.push(&fetch_limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(binds.as_slice())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(message_of(r)?);
    }
    let next_before = if out.len() as i64 > limit {
        out.pop();
        out.last().map(|last| last.id.clone())
    } else {
        None
    };
    Ok((out, next_before))
}

pub fn get_message(conn: &Connection, id: &str) -> rusqlite::Result<Option<MessageRow>> {
    conn.query_row(
        &format!("SELECT {MESSAGE_COLS} FROM chat_messages WHERE id = ?1"),
        params![id],
        message_of,
    )
    .optional()
}

/// OutgoingMessageEvidence 回填(T050):按 request_key 找出站行,
/// 回填 platform_message_id;sending/uncertain 行按接纳证据补 sent。
/// 返回 (message_id, 是否补了 sent) 供留痕;无匹配返回 None
/// (交付类回执没有聊天行,是正常路径)。
pub fn backfill_by_request_key(
    conn: &Connection,
    request_key: &str,
    platform_message_id: &str,
) -> rusqlite::Result<Option<(String, bool)>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, status FROM chat_messages
             WHERE request_key = ?1 AND direction = 'out'
             ORDER BY created_at DESC LIMIT 1",
            params![request_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, status)) = row else {
        return Ok(None);
    };
    let promoted = matches!(status.as_str(), "sending" | "uncertain");
    let next_status = if promoted { "sent" } else { status.as_str() };
    conn.execute(
        "UPDATE chat_messages SET platform_message_id = ?2, status = ?3 WHERE id = ?1",
        params![id, platform_message_id, next_status],
    )?;
    Ok(Some((id, promoted)))
}

// ---------- 快捷回复 ----------

#[derive(Clone, Debug)]
pub struct QuickReplyRow {
    pub id: String,
    pub body: String,
    pub created_at: String,
}

pub fn list_quick_replies(conn: &Connection) -> rusqlite::Result<Vec<QuickReplyRow>> {
    let mut stmt = conn.prepare("SELECT id, body, created_at FROM quick_replies ORDER BY created_at, id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(QuickReplyRow {
                id: r.get(0)?,
                body: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn count_quick_replies(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM quick_replies", [], |r| r.get(0))
}

pub fn insert_quick_reply(conn: &Connection, id: &str, body: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO quick_replies(id, body, created_at) VALUES (?1, ?2, ?3)",
        params![id, body, now_rfc3339()],
    )?;
    Ok(())
}

pub fn delete_quick_reply(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("DELETE FROM quick_replies WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

// ---------- 买家备注 ----------

pub fn get_buyer_note(conn: &Connection, account_id: &str, buyer_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT note FROM buyer_notes WHERE account_id = ?1 AND buyer_id = ?2",
        params![account_id, buyer_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn upsert_buyer_note(
    conn: &Connection,
    account_id: &str,
    buyer_id: &str,
    note: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO buyer_notes(account_id, buyer_id, note, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id, buyer_id) DO UPDATE SET note = excluded.note,
             updated_at = excluded.updated_at",
        params![account_id, buyer_id, note, now_rfc3339()],
    )?;
    Ok(())
}

// ---------- 未读汇总 ----------

/// 全局未读汇总(侧边栏徽标):SUM(unread_count) 与按账号分组(账号 Tab 徽标)。
/// 隐藏会话不计入(hidden_at IS NULL)。
pub fn unread_summary(conn: &Connection) -> rusqlite::Result<(i64, Vec<(String, i64)>)> {
    let total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(unread_count), 0) FROM conversations
         WHERE hidden_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT account_id, SUM(unread_count) FROM conversations
         WHERE hidden_at IS NULL AND unread_count > 0
         GROUP BY account_id ORDER BY account_id",
    )?;
    let by_account = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((total, by_account))
}
