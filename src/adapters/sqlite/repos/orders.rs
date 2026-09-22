//! orders/order_facts 表仓储:外部三元唯一、fact_version 单调、终态不倒退。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct OrderRow {
    pub id: String,
    pub platform: String,
    pub account_id: String,
    pub external_order_id: String,
    pub buyer_id: Option<String>,
    pub platform_status: String,
    pub fact_version: i64,
}

/// 状态严重度:退款/取消/完成等终态优先,旧付款事件不能倒退(data-model)。
fn state_rank(state: &str) -> i32 {
    match state {
        "refunded" => 7,
        "refunding" => 6,
        "canceled" => 5,
        "completed" => 4,
        "shipped" => 3,
        "pending_ship" => 2,
        "unpaid" => 1,
        _ => 0, // unknown 永不覆盖已知状态
    }
}

/// 按外部三元查找或创建订单;返回 (订单行, 是否新建)。
pub fn find_or_create(
    conn: &Connection,
    id: &str,
    platform: &str,
    account_id: &str,
    external_order_id: &str,
) -> rusqlite::Result<(OrderRow, bool)> {
    if let Some(row) = find_by_external(conn, platform, account_id, external_order_id)? {
        return Ok((row, false));
    }
    let now = utc_now_ms();
    conn.execute(
        "INSERT INTO orders(id, platform, account_id, external_order_id, trade_type,
             platform_status, history_class, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'unknown', 'unknown', 'unknown', ?5, ?5)",
        params![id, platform, account_id, external_order_id, now],
    )?;
    Ok((
        OrderRow {
            id: id.to_string(),
            platform: platform.to_string(),
            account_id: account_id.to_string(),
            external_order_id: external_order_id.to_string(),
            buyer_id: None,
            platform_status: "unknown".into(),
            fact_version: 0,
        },
        true,
    ))
}

pub fn find_by_external(
    conn: &Connection,
    platform: &str,
    account_id: &str,
    external_order_id: &str,
) -> rusqlite::Result<Option<OrderRow>> {
    conn.query_row(
        "SELECT id, platform, account_id, external_order_id, buyer_id, platform_status, fact_version
         FROM orders WHERE platform = ?1 AND account_id = ?2 AND external_order_id = ?3",
        params![platform, account_id, external_order_id],
        |r| {
            Ok(OrderRow {
                id: r.get(0)?,
                platform: r.get(1)?,
                account_id: r.get(2)?,
                external_order_id: r.get(3)?,
                buyer_id: r.get(4)?,
                platform_status: r.get(5)?,
                fact_version: r.get(6)?,
            })
        },
    )
    .optional()
}

/// 应用平台状态信号:仅当新状态严重度更高才推进(fact_version 单调)。
/// 返回是否发生状态变化。
pub fn apply_state_signal(
    conn: &Connection,
    order_id: &str,
    new_state: &str,
) -> rusqlite::Result<bool> {
    let current: Option<String> = conn
        .query_row(
            "SELECT platform_status FROM orders WHERE id = ?1",
            params![order_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(false);
    };
    if state_rank(new_state) > state_rank(&current) {
        conn.execute(
            "UPDATE orders SET platform_status = ?1, fact_version = fact_version + 1,
                    observed_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![new_state, utc_now_ms(), order_id],
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// 一条订单事实(脱敏规范证据 + 摘要)。
pub struct NewOrderFact<'a> {
    pub id: &'a str,
    pub order_id: &'a str,
    pub source: &'a str,
    pub source_event_id: Option<&'a str>,
    pub observed_at: i64,
    pub normalized_evidence: &'a str,
    pub evidence_digest: &'a str,
}

/// 追加订单事实。
pub fn append_fact(conn: &Connection, fact: &NewOrderFact<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO order_facts(id, order_id, source, source_event_id, observed_at,
             normalized_evidence, evidence_digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            fact.id,
            fact.order_id,
            fact.source,
            fact.source_event_id,
            fact.observed_at,
            fact.normalized_evidence,
            fact.evidence_digest,
            utc_now_ms()
        ],
    )?;
    Ok(())
}

/// 订单事实字段增量;仅填充 NULL("未知")字段,已核验值不被旧事件覆盖。
#[derive(Default)]
pub struct OrderFactMerge<'a> {
    pub buyer_id: Option<&'a str>,
    pub paid_at: Option<i64>,
    pub quantity: Option<i64>,
    pub amount_minor: Option<i64>,
    pub currency: Option<&'a str>,
    pub sku_pairs: Option<&'a str>,
    pub sku_complete: Option<bool>,
    pub trade_type: Option<&'a str>,
}

pub fn merge_order_facts(
    conn: &Connection,
    order_id: &str,
    m: &OrderFactMerge<'_>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE orders SET
             buyer_id = COALESCE(buyer_id, ?2),
             paid_at = COALESCE(paid_at, ?3),
             quantity = COALESCE(quantity, ?4),
             amount_minor = COALESCE(amount_minor, ?5),
             currency = COALESCE(currency, ?6),
             sku_pairs = COALESCE(sku_pairs, ?7),
             sku_complete = COALESCE(sku_complete, ?8),
             trade_type = CASE WHEN trade_type = 'unknown' THEN COALESCE(?9, trade_type) ELSE trade_type END,
             fact_version = fact_version + 1,
             updated_at = ?10
         WHERE id = ?1",
        params![
            order_id,
            m.buyer_id,
            m.paid_at,
            m.quantity,
            m.amount_minor,
            m.currency,
            m.sku_pairs,
            m.sku_complete.map(|b| b as i64),
            m.trade_type,
            utc_now_ms()
        ],
    )?;
    Ok(())
}

/// 内部订单 ID → 平台订单号(确认发货时需要)。
pub fn get_order_external(conn: &Connection, order_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT external_order_id FROM orders WHERE id = ?1",
        params![order_id],
        |r| r.get(0),
    )
    .optional()
}

pub struct OrderSummaryRow {
    pub id: String,
    pub account_id: String,
    pub platform_order_id: String,
    pub item_id: Option<String>,
    pub paid_at: Option<i64>,
    pub quantity: Option<i64>,
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
    pub trade_type: String,
    pub platform_state: String,
    pub delivery_state: Option<String>,
    pub review_state: Option<String>,
    pub confirmation_state: Option<String>,
    pub version: i64,
    pub updated_at: i64,
}

/// 列表查询(账号筛选 + 稳定排序);US1 最小实现,US5 补游标/时间范围/索引验证。
pub fn list_summary(
    conn: &Connection,
    account_id: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<OrderSummaryRow>> {
    let sql = format!(
        "SELECT o.id, o.account_id, o.external_order_id, o.item_id, o.paid_at, o.quantity,
                o.amount_minor, o.currency, o.trade_type, o.platform_status AS platform_state,
                d.content_state, d.review_state, d.confirmation_state, o.fact_version AS version, o.updated_at
         FROM orders o LEFT JOIN deliveries d ON d.order_id = o.id AND d.kind = 'initial'
         {} ORDER BY o.updated_at DESC, o.id LIMIT {}",
        match account_id {
            Some(_) => "WHERE o.account_id = ?1",
            None => "",
        },
        limit
    );
    let mut stmt = conn.prepare(&sql)?;
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<OrderSummaryRow> {
        Ok(OrderSummaryRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            platform_order_id: r.get(2)?,
            item_id: r.get(3)?,
            paid_at: r.get(4)?,
            quantity: r.get(5)?,
            amount_minor: r.get(6)?,
            currency: r.get(7)?,
            trade_type: r.get(8)?,
            platform_state: r.get(9)?,
            delivery_state: r.get(10)?,
            review_state: r.get(11)?,
            confirmation_state: r.get(12)?,
            version: r.get(13)?,
            updated_at: r.get(14)?,
        })
    };
    match account_id {
        Some(acc) => stmt.query_map(params![acc], map)?.collect(),
        None => stmt.query_map([], map)?.collect(),
    }
}

/// 详情:订单 + initial 交付 + 尝试时间线。
pub struct OrderDetailRow {
    pub summary: OrderSummaryRow,
    pub delivery_id: Option<String>,
    pub delivery_version: Option<i64>,
    pub retry_count: Option<i64>,
    pub evidence_origin: Option<String>,
    pub attempts_json: String,
}

pub fn get_detail(conn: &Connection, order_id: &str) -> rusqlite::Result<Option<OrderDetailRow>> {
    let Some(summary) = list_one(conn, order_id)? else {
        return Ok(None);
    };
    let delivery = {
        let mut stmt = conn.prepare(
            "SELECT id, version, retry_count, evidence_origin FROM deliveries
             WHERE order_id = ?1 AND kind = 'initial'",
        )?;
        let mut rows = stmt.query(params![order_id])?;
        match rows.next()? {
            Some(r) => Some((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
            )),
            None => None,
        }
    };
    let attempts = {
        let mut stmt = conn.prepare(
            "SELECT id, action_kind, sequence, state, prepared_at, finished_at, result_code
             FROM attempts WHERE delivery_id = ?1 ORDER BY prepared_at",
        )?;
        let mut arr = Vec::new();
        let delivery_id = delivery.as_ref().map(|d| d.0.clone());
        if let Some(did) = delivery_id {
            let rows = stmt.query_map(params![did], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "kind": r.get::<_, String>(1)?,
                    "sequence": r.get::<_, i64>(2)?,
                    "state": r.get::<_, String>(3)?,
                    "started_at": r.get::<_, i64>(4)?,
                    "finished_at": r.get::<_, Option<i64>>(5)?,
                    "reason": r.get::<_, Option<String>>(6)?,
                }))
            })?;
            for row in rows {
                arr.push(row?);
            }
        }
        serde_json::Value::Array(arr).to_string()
    };
    Ok(Some(OrderDetailRow {
        summary,
        delivery_id: delivery.as_ref().map(|d| d.0.clone()),
        delivery_version: delivery.as_ref().map(|d| d.1),
        retry_count: delivery.as_ref().map(|d| d.2),
        evidence_origin: delivery.as_ref().and_then(|d| d.3.clone()),
        attempts_json: attempts,
    }))
}

fn list_one(conn: &Connection, order_id: &str) -> rusqlite::Result<Option<OrderSummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT o.id, o.account_id, o.external_order_id, o.item_id, o.paid_at, o.quantity,
                o.amount_minor, o.currency, o.trade_type, o.platform_status AS platform_state,
                d.content_state, d.review_state, d.confirmation_state, o.fact_version AS version, o.updated_at
         FROM orders o LEFT JOIN deliveries d ON d.order_id = o.id AND d.kind = 'initial'
         WHERE o.id = ?1",
    )?;
    stmt.query_row(params![order_id], |r| {
        Ok(OrderSummaryRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            platform_order_id: r.get(2)?,
            item_id: r.get(3)?,
            paid_at: r.get(4)?,
            quantity: r.get(5)?,
            amount_minor: r.get(6)?,
            currency: r.get(7)?,
            trade_type: r.get(8)?,
            platform_state: r.get(9)?,
            delivery_state: r.get(10)?,
            review_state: r.get(11)?,
            confirmation_state: r.get(12)?,
            version: r.get(13)?,
            updated_at: r.get(14)?,
        })
    })
    .optional()
}

/// 人工动作所需的订单上下文(单次窄读)。
pub struct OrderManualCtx {
    pub id: String,
    pub account_id: String,
    pub external_order_id: String,
    pub platform_status: String,
    pub buyer_id: Option<String>,
}

pub fn get_manual_ctx(
    conn: &Connection,
    order_db_id: &str,
) -> rusqlite::Result<Option<OrderManualCtx>> {
    conn.query_row(
        "SELECT id, account_id, external_order_id, platform_status, buyer_id
         FROM orders WHERE id = ?1",
        params![order_db_id],
        |r| {
            Ok(OrderManualCtx {
                id: r.get(0)?,
                account_id: r.get(1)?,
                external_order_id: r.get(2)?,
                platform_status: r.get(3)?,
                buyer_id: r.get(4)?,
            })
        },
    )
    .optional()
}

/// 交付 ID → 订单内部 ID(人工动作按订单寻址)。
pub fn get_order_external_reverse(
    conn: &Connection,
    delivery_id: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT order_id FROM deliveries WHERE id = ?1",
        params![delivery_id],
        |r| r.get(0),
    )
    .optional()
}

/// 列表筛选条件(T074);游标为 (updated_at,id) 稳定键集分页。
pub struct OrderFilter<'a> {
    pub account_id: Option<&'a str>,
    pub platform_order_id: Option<&'a str>,
    pub platform_state: Option<&'a str>,
    /// 付款时间范围(UTC 毫秒)
    pub paid_from: Option<i64>,
    pub paid_to: Option<i64>,
    /// 游标:上次页尾 (updated_at,id)
    pub cursor: Option<(&'a i64, &'a str)>,
    pub limit: i64,
}

pub fn list_filtered(
    conn: &Connection,
    f: &OrderFilter<'_>,
) -> rusqlite::Result<Vec<OrderSummaryRow>> {
    let mut sql = String::from(
        "SELECT o.id, o.account_id, o.external_order_id, o.item_id, o.paid_at, o.quantity,
                o.amount_minor, o.currency, o.trade_type,
                o.platform_status AS platform_state,
                d.content_state, d.review_state, d.confirmation_state,
                o.fact_version AS version, o.updated_at
         FROM orders o LEFT JOIN deliveries d ON d.order_id = o.id AND d.kind = 'initial'
         WHERE 1=1",
    );
    let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(a) = f.account_id {
        sql.push_str(&format!(" AND o.account_id = ?{}", bind.len() + 1));
        bind.push(Box::new(a.to_string()));
    }
    if let Some(ext) = f.platform_order_id {
        sql.push_str(&format!(
            " AND o.external_order_id LIKE ?{}",
            bind.len() + 1
        ));
        bind.push(Box::new(format!("%{ext}%")));
    }
    if let Some(s) = f.platform_state {
        sql.push_str(&format!(" AND o.platform_status = ?{}", bind.len() + 1));
        bind.push(Box::new(s.to_string()));
    }
    if let Some(from) = f.paid_from {
        sql.push_str(&format!(" AND o.paid_at >= ?{}", bind.len() + 1));
        bind.push(Box::new(from));
    }
    if let Some(to) = f.paid_to {
        sql.push_str(&format!(" AND o.paid_at <= ?{}", bind.len() + 1));
        bind.push(Box::new(to));
    }
    if let Some((ts, id)) = f.cursor {
        sql.push_str(&format!(
            " AND (o.updated_at < ?{} OR (o.updated_at = ?{} AND o.id < ?{}))",
            bind.len() + 1,
            bind.len() + 2,
            bind.len() + 3
        ));
        bind.push(Box::new(*ts));
        bind.push(Box::new(*ts));
        bind.push(Box::new(id.to_string()));
    }
    sql.push_str(&format!(
        " ORDER BY o.updated_at DESC, o.id DESC LIMIT {}",
        f.limit.max(1)
    ));
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::ToSql> = bind.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(refs.as_slice(), |r| {
            Ok(OrderSummaryRow {
                id: r.get(0)?,
                account_id: r.get(1)?,
                platform_order_id: r.get(2)?,
                item_id: r.get(3)?,
                paid_at: r.get(4)?,
                quantity: r.get(5)?,
                amount_minor: r.get(6)?,
                currency: r.get(7)?,
                trade_type: r.get(8)?,
                platform_state: r.get(9)?,
                delivery_state: r.get(10)?,
                review_state: r.get(11)?,
                confirmation_state: r.get(12)?,
                version: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<OrderSummaryRow>>>()?;
    Ok(rows)
}

/// 数据库健康探测(dashboard persistence 状态)。
pub fn ping(conn: &Connection) -> rusqlite::Result<()> {
    conn.query_row("SELECT 1", [], |_| Ok(()))
}
