//! deliveries/content_snapshots/attempts/delivery_proofs/order_execution_guards
//! 仓储:唯一 initial、冻结快照、attempt 先落库、guard 互斥。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::crypto::Envelope;
use crate::domain::time_util::utc_now_ms;

pub struct DeliveryRow {
    pub id: String,
    pub order_id: String,
    pub kind: String,
    pub parent_delivery_id: Option<String>,
    pub rule_id: Option<String>,
    pub content_snapshot_id: Option<String>,
    pub content_state: String,
    pub review_state: String,
    pub confirmation_state: String,
    pub evidence_origin: String,
    pub retry_count: i64,
    pub next_retry_at: Option<i64>,
    pub version: i64,
    pub execution_policy: String,
}

const COLS: &str = "id, order_id, kind, parent_delivery_id, rule_id, content_snapshot_id,
    content_state, review_state, confirmation_state, evidence_origin, retry_count,
    next_retry_at, version, execution_policy";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRow> {
    Ok(DeliveryRow {
        id: r.get(0)?,
        order_id: r.get(1)?,
        kind: r.get(2)?,
        parent_delivery_id: r.get(3)?,
        rule_id: r.get(4)?,
        content_snapshot_id: r.get(5)?,
        content_state: r.get(6)?,
        review_state: r.get(7)?,
        confirmation_state: r.get(8)?,
        evidence_origin: r.get(9)?,
        retry_count: r.get(10)?,
        next_retry_at: r.get(11)?,
        version: r.get(12)?,
        execution_policy: r.get(13)?,
    })
}

/// T1:创建唯一 initial 交付(部分唯一索引拒绝重复);已存在返回 None(FR-014)。
pub fn create_initial(
    conn: &Connection,
    id: &str,
    order_id: &str,
) -> rusqlite::Result<Option<DeliveryRow>> {
    let existing = find_initial(conn, order_id)?;
    if existing.is_some() {
        return Ok(None);
    }
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO deliveries(id, order_id, kind, content_state, created_at, updated_at)
         VALUES (?1, ?2, 'initial', 'pending_verification', ?3, ?3)",
        params![id, order_id, now],
    )?;
    get(conn, id)
}

pub fn find_initial(conn: &Connection, order_id: &str) -> rusqlite::Result<Option<DeliveryRow>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM deliveries WHERE order_id = ?1 AND kind = 'initial'"),
        params![order_id],
        row,
    )
    .optional()
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<DeliveryRow>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM deliveries WHERE id = ?1"),
        params![id],
        row,
    )
    .optional()
}

/// 版本化状态更新;迁移合法性由 domain::delivery 校验后再调用。
pub fn set_state(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    content_state: Option<&str>,
    review_state: Option<&str>,
    confirmation_state: Option<&str>,
    retry_count: Option<i64>,
    next_retry_at: Option<Option<i64>>,
) -> rusqlite::Result<Option<DeliveryRow>> {
    let n = conn.execute(
        "UPDATE deliveries SET
             content_state = COALESCE(?2, content_state),
             review_state = COALESCE(?3, review_state),
             confirmation_state = COALESCE(?4, confirmation_state),
             retry_count = COALESCE(?5, retry_count),
             next_retry_at = COALESCE(?6, next_retry_at),
             version = version + 1, updated_at = ?7
         WHERE id = ?1 AND version = ?8",
        params![
            id,
            content_state,
            review_state,
            confirmation_state,
            retry_count,
            next_retry_at,
            utc_now_ms(),
            expected_version
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get(conn, id)
}

/// 关联规则与冻结快照(T2):把快照 ID 绑定到交付并进入 queued。
pub fn bind_snapshot(
    conn: &Connection,
    id: &str,
    rule_id: &str,
    snapshot_id: &str,
    expected_version: i64,
) -> rusqlite::Result<Option<DeliveryRow>> {
    let n = conn.execute(
        "UPDATE deliveries SET rule_id = ?2, content_snapshot_id = ?3,
                content_state = 'queued', version = version + 1, updated_at = ?4
         WHERE id = ?1 AND version = ?5 AND content_state = 'pending_verification'",
        params![id, rule_id, snapshot_id, utc_now_ms(), expected_version],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get(conn, id)
}

/// T2:冻结内容快照(从规则内容版本复制密文,独立 AAD 由应用层构造)。
pub struct NewSnapshot<'a> {
    pub id: &'a str,
    pub order_id: &'a str,
    pub source_content_id: &'a str,
    pub envelope: &'a Envelope,
    pub digest: &'a str,
}

pub fn insert_snapshot(conn: &Connection, s: &NewSnapshot<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO content_snapshots(id, order_id, source_content_id, ciphertext, nonce,
             key_id, format_version, digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.id,
            s.order_id,
            s.source_content_id,
            s.envelope.ciphertext,
            s.envelope.nonce.as_slice(),
            s.envelope.key_id,
            s.envelope.format_version as i64,
            s.digest,
            utc_now_ms()
        ],
    )?;
    Ok(())
}

pub struct SnapshotRow {
    pub id: String,
    pub order_id: String,
    pub envelope: Envelope,
    pub digest: String,
}

pub fn get_snapshot(conn: &Connection, id: &str) -> rusqlite::Result<Option<SnapshotRow>> {
    conn.query_row(
        "SELECT id, order_id, ciphertext, nonce, key_id, format_version, digest
         FROM content_snapshots WHERE id = ?1",
        params![id],
        |r| {
            let nonce_slice: Vec<u8> = r.get(3)?;
            let mut nonce = [0u8; crate::domain::crypto::NONCE_LEN];
            if nonce_slice.len() == nonce.len() {
                nonce.copy_from_slice(&nonce_slice);
            }
            Ok(SnapshotRow {
                id: r.get(0)?,
                order_id: r.get(1)?,
                envelope: Envelope {
                    ciphertext: r.get(2)?,
                    nonce,
                    key_id: r.get(4)?,
                    format_version: r.get::<_, i64>(5)? as u16,
                },
                digest: r.get(6)?,
            })
        },
    )
    .optional()
}

pub struct AttemptRow {
    pub id: String,
    pub delivery_id: String,
    pub action_kind: String,
    pub sequence: i64,
    pub request_id: String,
    pub state: String,
    pub prepared_at: i64,
    pub handoff_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub result_code: Option<String>,
    pub proof_ref: Option<String>,
}

/// T3:持久化 dispatching 尝试;request_id(关联 mid)先于 handoff 落库。
/// 返回 None 表示版本冲突或已有未决 attempt。
/// 动作前提:initial/retry 要求 queued|not_sent;confirmation 要求内容 accepted。
pub fn prepare_attempt(
    conn: &Connection,
    id: &str,
    delivery_id: &str,
    action_kind: &str,
    request_id: &str,
    expected_delivery_version: Option<i64>,
) -> rusqlite::Result<Option<AttemptRow>> {
    // 同动作存在未决 attempt 时不重复准备(互斥第一道防线)
    let pending: Option<String> = conn
        .query_row(
            "SELECT id FROM attempts WHERE delivery_id = ?1 AND action_kind = ?2
             AND state IN ('prepared','dispatching')",
            params![delivery_id, action_kind],
            |r| r.get(0),
        )
        .optional()?;
    if pending.is_some() {
        return Ok(None);
    }
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM attempts WHERE delivery_id = ?1 AND action_kind = ?2",
        params![delivery_id, action_kind],
        |r| r.get(0),
    )?;
    let allowed_states = match action_kind {
        "confirmation" => "('accepted')",
        // 人工补发允许:已交付(买家要求重发)/未知(核对后)/确定未发送(FR-022)
        "manual_resend" => "('accepted','unknown','not_sent')",
        // 007 US6:人工确认平台已发货,前提内容交付 accepted(FR-062)
        "manual_confirm" => "('accepted')",
        _ => "('queued','not_sent')",
    };
    let n = conn.execute(
        &format!(
            "INSERT INTO attempts(id, delivery_id, action_kind, sequence, request_id, state, prepared_at)
             SELECT ?1, ?2, ?3, ?4, ?5, 'dispatching', ?6
             WHERE EXISTS (SELECT 1 FROM deliveries WHERE id = ?2
                           AND content_state IN {allowed_states}
                           AND (?7 IS NULL OR version = ?7))"
        ),
        params![
            id,
            delivery_id,
            action_kind,
            seq,
            request_id,
            utc_now_ms(),
            expected_delivery_version
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get_attempt(conn, id)
}

pub fn get_attempt(conn: &Connection, id: &str) -> rusqlite::Result<Option<AttemptRow>> {
    conn.query_row(
        "SELECT id, delivery_id, action_kind, sequence, request_id, state, prepared_at,
                handoff_at, finished_at, result_code, proof_ref
         FROM attempts WHERE id = ?1",
        params![id],
        |r| {
            Ok(AttemptRow {
                id: r.get(0)?,
                delivery_id: r.get(1)?,
                action_kind: r.get(2)?,
                sequence: r.get(3)?,
                request_id: r.get(4)?,
                state: r.get(5)?,
                prepared_at: r.get(6)?,
                handoff_at: r.get(7)?,
                finished_at: r.get(8)?,
                result_code: r.get(9)?,
                proof_ref: r.get(10)?,
            })
        },
    )
    .optional()
}

pub fn find_attempt_by_request(
    conn: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<AttemptRow>> {
    conn.query_row(
        "SELECT id, delivery_id, action_kind, sequence, request_id, state, prepared_at,
                handoff_at, finished_at, result_code, proof_ref
         FROM attempts WHERE request_id = ?1 ORDER BY prepared_at DESC LIMIT 1",
        params![request_id],
        |r| {
            Ok(AttemptRow {
                id: r.get(0)?,
                delivery_id: r.get(1)?,
                action_kind: r.get(2)?,
                sequence: r.get(3)?,
                request_id: r.get(4)?,
                state: r.get(5)?,
                prepared_at: r.get(6)?,
                handoff_at: r.get(7)?,
                finished_at: r.get(8)?,
                result_code: r.get(9)?,
                proof_ref: r.get(10)?,
            })
        },
    )
    .optional()
}

pub fn mark_handoff(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE attempts SET handoff_at = ?1 WHERE id = ?2",
        params![utc_now_ms(), id],
    )?;
    Ok(())
}

pub fn finish_attempt(
    conn: &Connection,
    id: &str,
    state: &str,
    result_code: Option<&str>,
    proof_ref: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE attempts SET state = ?1, finished_at = ?2, result_code = ?3, proof_ref = ?4
         WHERE id = ?5 AND state = 'dispatching'",
        params![state, utc_now_ms(), result_code, proof_ref, id],
    )?;
    Ok(())
}

/// 记录平台/人工证明;同一 attempt 的平台证明只消费一次。
pub fn insert_proof(
    conn: &Connection,
    id: &str,
    attempt_id: &str,
    origin: &str,
    platform_message_id: Option<&str>,
    request_id: &str,
    buyer_id: Option<&str>,
    content_digest: &str,
    manual_action_id: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO delivery_proofs(id, attempt_id, origin, platform_message_id, request_id,
             buyer_id, content_digest, accepted_at, manual_action_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            attempt_id,
            origin,
            platform_message_id,
            request_id,
            buyer_id,
            content_digest,
            utc_now_ms(),
            manual_action_id
        ],
    )?;
    Ok(())
}

/// 该交付全部 attempt 已确认的 content_digest 清单(007 US2:多消息续发
/// 以 proof 判定已确认条目,research D4;单条路径不使用)。
pub fn confirmed_digests(conn: &Connection, delivery_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT dp.content_digest FROM delivery_proofs dp
         JOIN attempts a ON a.id = dp.attempt_id
         WHERE a.delivery_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![delivery_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 证明行(多消息全部已确认时的防御性收尾材料)。
pub struct ProofRow {
    pub id: String,
    pub platform_message_id: Option<String>,
    pub request_id: String,
    pub buyer_id: Option<String>,
    pub content_digest: String,
}

/// 交付最近一条证明(按落库顺序)。
pub fn latest_proof_for_delivery(
    conn: &Connection,
    delivery_id: &str,
) -> rusqlite::Result<Option<ProofRow>> {
    conn.query_row(
        "SELECT dp.id, dp.platform_message_id, dp.request_id, dp.buyer_id, dp.content_digest
         FROM delivery_proofs dp JOIN attempts a ON a.id = dp.attempt_id
         WHERE a.delivery_id = ?1 ORDER BY dp.rowid DESC LIMIT 1",
        params![delivery_id],
        |r| {
            Ok(ProofRow {
                id: r.get(0)?,
                platform_message_id: r.get(1)?,
                request_id: r.get(2)?,
                buyer_id: r.get(3)?,
                content_digest: r.get(4)?,
            })
        },
    )
    .optional()
}

/// T3:获取 order guard;已有活跃 guard 返回 None(自动/人工互斥)。
pub fn acquire_guard(    conn: &Connection,
    order_id: &str,
    delivery_id: &str,
    attempt_id: &str,
) -> rusqlite::Result<Option<()>> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT state FROM order_execution_guards WHERE order_id = ?1",
            params![order_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(state) = existing {
        // 活跃执行互斥;unknown_manual 是人工显式解决的唯一出口(数据模型),
        // 由人工动作接管并递增执行代次
        if state == "active" {
            return Ok(None);
        }
        conn.execute(
            "UPDATE order_execution_guards SET delivery_id = ?2, attempt_id = ?3,
                    execution_generation = execution_generation + 1, state = 'active'
             WHERE order_id = ?1",
            params![order_id, delivery_id, attempt_id],
        )?;
        return Ok(Some(()));
    }
    conn.execute(
        "INSERT INTO order_execution_guards(order_id, delivery_id, attempt_id,
             execution_generation, state) VALUES (?1, ?2, ?3, 0, 'active')",
        params![order_id, delivery_id, attempt_id],
    )?;
    Ok(Some(()))
}

/// guard 只在确定终态释放;未知时保持 active(data-model 并发约束 6)。
pub fn release_guard(conn: &Connection, order_id: &str, state: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE order_execution_guards SET state = ?2 WHERE order_id = ?1",
        params![order_id, state],
    )?;
    Ok(())
}

pub fn get_guard_state(conn: &Connection, order_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT state FROM order_execution_guards WHERE order_id = ?1",
        params![order_id],
        |r| r.get(0),
    )
    .optional()
}

/// 平台订单号 → 初始交付(测试/查询辅助)。
pub fn find_initial_by_external(
    conn: &Connection,
    external_order_id: &str,
) -> rusqlite::Result<Option<DeliveryRow>> {
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM deliveries WHERE kind = 'initial' AND order_id IN
             (SELECT id FROM orders WHERE external_order_id = ?1)"
        ),
        params![external_order_id],
        row,
    )
    .optional()
}
