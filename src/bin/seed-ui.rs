//! 002 界面走查播种(一次性开发工具,不入发行):为 mock 数据目录写入
//! 账号/商品/订单/交付/事项样本。规则与加密内容走 UI 正常创建路径,不在此伪造。
//! 用法:qing-delivery-seed-ui <data-dir>
use rusqlite::Connection;

fn main() {
    let dir = std::env::args().nth(1).expect("用法:seed-ui <data-dir>");
    let db_path = std::path::Path::new(&dir).join("main.db");
    let mut conn = Connection::open(&db_path).expect("打开数据库");
    let mig_dir = std::path::Path::new(&dir).join(".seed-tmp");
    std::fs::create_dir_all(&mig_dir).expect("临时目录");
    let applied: Result<i64, _> =
        qing_delivery::adapters::sqlite::migrations::apply(&mut conn, &mig_dir);
    applied.expect("迁移应用");

    let now = qing_delivery::domain::time_util::utc_now_ms();
    let today = now - 3_600_000;

    conn.execute("DELETE FROM issues", []).unwrap();
    conn.execute("DELETE FROM attempts", []).unwrap();
    conn.execute("DELETE FROM deliveries", []).unwrap();
    conn.execute("DELETE FROM orders", []).unwrap();
    conn.execute("DELETE FROM items", []).unwrap();
    conn.execute("DELETE FROM accounts", []).unwrap();

    conn.execute(
        "INSERT INTO accounts (id, platform, external_user_id, display_name, runtime_enabled,
         auto_delivery_enabled, auto_confirm_enabled, monitor_since, status, version, created_at, updated_at)
         VALUES ('acct-1','xianyu','9876543210abcdef','闲鱼小店',1,1,0,?1,'online',3,?1,?1)",
        [today],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO accounts (id, platform, external_user_id, display_name, runtime_enabled,
         auto_delivery_enabled, auto_confirm_enabled, monitor_since, status, version, created_at, updated_at)
         VALUES ('acct-2','xianyu','5551234444eeee','二号闲置店',0,0,0,NULL,'offline',1,?1,?1)",
        [today],
    )
    .unwrap();

    for (id, ext, title, state) in [
        ("item-1", "7011001", "网盘资料大合集(持续更新)", "on_sale"),
        ("item-2", "7011002", "设计素材包 500GB", "on_sale"),
        ("item-3", "7011003", "教程视频全集", "off_sale"),
    ] {
        conn.execute(
            "INSERT INTO items (id, account_id, external_item_id, title, listing_state, sku_definition, version, created_at, updated_at)
             VALUES (?1,'acct-1',?2,?3,?4,'[]',1,?5,?5)",
            rusqlite::params![id, ext, title, state, today],
        )
        .unwrap();
    }

    // 订单:o1/o2 已交付,o3 结果未知+人工事项,o4 待发送
    for (id, ext, paid, amount, item) in [
        ("ord-1", "XY2026092201", today, 1280i64, "item-1"),
        ("ord-2", "XY2026092202", today + 60_000, 2560, "item-2"),
        ("ord-3", "XY2026092203", today + 120_000, 990, "item-1"),
        ("ord-4", "XY2026092204", today + 180_000, 4990, "item-2"),
    ] {
        conn.execute(
            "INSERT INTO orders (id, platform, account_id, external_order_id, item_id, sku_complete,
             amount_minor, currency, quantity, paid_at, trade_type, platform_status, fact_version, created_at, updated_at)
             VALUES (?1,'xianyu','acct-1',?2,?3,1,?4,'CNY',1,?5,'standard','pending_ship',1,?5,?5)",
            rusqlite::params![id, ext, item, amount, paid],
        )
        .unwrap();
    }

    for (id, order, content, confirm, retry) in [
        ("dlv-1", "ord-1", "accepted", "accepted", 0i64),
        ("dlv-2", "ord-2", "accepted", "accepted", 0),
        ("dlv-3", "ord-3", "unknown", "unknown", 3),
        ("dlv-4", "ord-4", "queued", "disabled", 0),
    ] {
        conn.execute(
            "INSERT INTO deliveries (id, order_id, kind, content_state, review_state, confirmation_state,
             evidence_origin, retry_count, version, restore_epoch, execution_policy, created_at, updated_at)
             VALUES (?1,?2,'initial',?3,'none',?4,'platform_event',?5,1,0,'auto',?6,?6)",
            rusqlite::params![id, order, content, confirm, retry, today + 300_000],
        )
        .unwrap();
    }

    for (id, order, kind, seq, state) in [
        ("att-1", "ord-1", "send_message", 1i64, "succeeded"),
        ("att-2", "ord-3", "send_message", 1, "unknown"),
        ("att-3", "ord-3", "send_message", 2, "unknown"),
        ("att-4", "ord-3", "send_message", 3, "unknown"),
    ] {
        conn.execute(
            "INSERT INTO attempts (id, delivery_id, action_kind, sequence, request_id, state, prepared_at)
             VALUES (?1, (SELECT id FROM deliveries WHERE order_id=?2 AND kind='initial'), ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, order, kind, seq, format!("req-{id}"), state, today + 240_000],
        )
        .unwrap();
    }

    conn.execute(
        "INSERT INTO issues (id, order_id, account_id, delivery_id, kind, reason_code, allowed_actions, state, created_at)
         VALUES ('iss-1','ord-3','acct-1',(SELECT id FROM deliveries WHERE order_id='ord-3'),'delivery_unknown',
         'ws_ack_timeout','[\"resend\",\"terminate\"]','open',?1)",
        [today + 600_000],
    )
    .unwrap();

    println!("播种完成:2 账号 / 3 商品 / 4 订单(2 交付 1 未知 1 待发)/ 1 待处理事项");
}
