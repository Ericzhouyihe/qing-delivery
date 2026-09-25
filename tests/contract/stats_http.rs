//! 运营统计契约测试(005,T004/T012):GET /api/v1/stats/overview 的鉴权、
//! 参数校验、统计口径(付款事实/CNY/退款不回冲)、只读不变式与趋势分桶
//! (contracts/stats-api.md;spec FR-003/FR-009/FR-010/FR-012/FR-016)。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use crate::support::app::{TestApp, spawn_app};

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn cookie_of(resp: &axum::response::Response) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.starts_with("qing_session=").then(|| {
                s["qing_session=".len()..]
                    .split(';')
                    .next()
                    .unwrap()
                    .to_string()
            })
        })
        .unwrap()
}

async fn login_session(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/session")
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/initialize")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    r#"{"password":"a-long-password-123","password_confirmation":"a-long-password-123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    cookie_of(&resp)
}

async fn get_stats_raw(
    app: &TestApp,
    cookie: &str,
    query: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/stats/overview{query}"))
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

/// 口径矩阵种子行:(订单号, paid_at(None=未付款), 金额, 币种, 平台状态)。
type SeedRow<'a> = (&'a str, Option<i64>, Option<i64>, Option<&'a str>, &'a str);

/// 播种一个账号 + 若干订单;返回函数便于断言各口径。
async fn seed_orders(app: &TestApp, now: i64) {
    app.db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, status, created_at, updated_at)
                 VALUES ('acct-1', 'xianyu', 'u1', '统计账号', 'online', 0, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, status, created_at, updated_at)
                 VALUES ('acct-2', 'xianyu', 'u2', '离线账号', 'offline', 0, 0)",
                [],
            )?;
            // 口径矩阵:o1 CNY 计入;o2 非 CNY 排除合计计入订单数;o3 金额缺失排除合计
            // 计入订单数;o4 退款单仍计入(不回冲,澄清 Q1);o5 小写币种不入合计
            let rows: Vec<SeedRow> = vec![
                ("o1", Some(now - 60_000), Some(10_00), Some("CNY"), "paid"),
                ("o2", Some(now - 50_000), Some(25_50), Some("USD"), "paid"),
                ("o3", Some(now - 40_000), None, Some("CNY"), "paid"),
                ("o4", Some(now - 30_000), Some(8_00), Some("CNY"), "refund_success"),
                ("o5", Some(now - 20_000), Some(5_00), Some("cny"), "paid"),
                // 未付款:paid_at 缺失,不构成付款事实(宪章 IV 关键测试清单)
                ("o9", None, Some(9_00), Some("CNY"), "pending_payment"),
                // 区间外:昨天(不落入所选区间,但落入前一等长区间)
                ("o6", Some(now - 24 * 3600 * 1000), Some(6_00), Some("CNY"), "paid"),
            ];
            for (id, paid_at, amount, currency, status) in rows {
                conn.execute(
                    "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at,
                         amount_minor, currency, platform_status, created_at, updated_at)
                     VALUES (?1, 'xianyu', 'acct-1', ?1, ?2, ?3, ?4, ?5, 0, 0)",
                    rusqlite::params![id, paid_at, amount, currency, status],
                )?;
            }
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn stats_未登录返回401() {
    let app = spawn_app().await;
    let (status, _) = get_stats_raw(&app, "no-such-session", "?from=0&to=1").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stats_参数非法三态返回400() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    // from/to 非数字
    let (status, _) = get_stats_raw(&app, &cookie, "?from=abc&to=100").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // to <= from
    let (status, _) = get_stats_raw(&app, &cookie, "?from=100&to=100").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 跨度超过 92 天
    let over = 93 * 24 * 3600 * 1000i64;
    let (status, _) = get_stats_raw(&app, &cookie, format!("?from=0&to={over}").as_str()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 缺参
    let (status, _) = get_stats_raw(&app, &cookie, "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_空区间为零且无对比徽标() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    // 久远空区间(1970 年头一小时,无任何订单)
    let hour = 3600 * 1000i64;
    let (status, body) = get_stats_raw(&app, &cookie, format!("?from=0&to={hour}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revenue"]["minor_units"], 0);
    assert_eq!(body["revenue"]["order_count"], 0);
    assert_eq!(body["revenue"]["currency"], "CNY");
    assert!(
        body["revenue"]["change_percent"].is_null(),
        "前区间无数据不得显示百分比(FR-003)"
    );
    assert_eq!(body["accounts"]["total"], 0);
    assert_eq!(body["pending_issues"], 0);
    assert_eq!(body["range"]["granularity"], "hourly");
    // 横幅状态随统计响应返回(研究 R7 单请求装配)
    assert_eq!(body["stopping"], false);
    assert!(body["restore"].is_null(), "新装系统无恢复隔离");
}

#[tokio::test]
async fn stats_口径_付款事实cny退款不回冲() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;

    // 2 小时窗口覆盖 o1~o5(o6 在 24h 前,区间外)
    let from = now - 3600 * 1000;
    let to = now + 3600 * 1000;
    let (status, body) =
        get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    // 合计 = o1(1000) + o4(800,退款不回冲);o2 USD/o3 缺失/o5 小写/o9 未付款不入
    assert_eq!(body["revenue"]["minor_units"], 1_800);
    // 订单数 = o1~o5 全部(付款事实口径);o9 未付款不计入
    assert_eq!(
        body["revenue"]["order_count"], 5,
        "未付款订单不得计入订单数"
    );
    assert_eq!(body["revenue"]["currency"], "CNY");

    // 账号快照:online 1 / total 2
    assert_eq!(body["accounts"]["online"], 1);
    assert_eq!(body["accounts"]["total"], 2);
    // 无事项
    assert_eq!(body["pending_issues"], 0);
}

#[tokio::test]
async fn stats_对比徽标按前一等长区间计算() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;
    // 前区间 [now-3h, now-1h) 一笔:o7
    app.db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at,
                     amount_minor, currency, platform_status, created_at, updated_at)
                 VALUES ('o7', 'xianyu', 'acct-1', 'o7', ?1, 6_00, 'CNY', 'paid', 0, 0)",
                rusqlite::params![now - 2 * 3600 * 1000],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();

    // 本区间 = [now-1h, now+1h):o1~o5,合计 1800;前区间含 o7(600)
    let from = now - 3600 * 1000;
    let to = now + 3600 * 1000;
    let (status, body) =
        get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revenue"]["previous_minor_units"], 600);
    // (1800-600)/600 = +200%
    assert_eq!(body["revenue"]["change_percent"], 200);
}

#[tokio::test]
async fn stats_开放事项计数() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;
    app.db
        .call(move |conn| {
            for (id, state) in [("i1", "open"), ("i2", "open"), ("i3", "resolved")] {
                conn.execute(
                    "INSERT INTO issues (id, account_id, kind, reason_code, state, created_at)
                     VALUES (?1, 'acct-1', 'delivery_unknown', 'test', ?2, 0)",
                    rusqlite::params![id, state],
                )?;
            }
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();

    let from = now - 3600 * 1000;
    let to = now + 3600 * 1000;
    let (status, body) =
        get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pending_issues"], 2, "只计 open 事项");
}

async fn db_snapshot(app: &TestApp) -> (i64, i64, i64, String) {
    app.db
        .call(|conn| {
            let orders: i64 = conn.query_row("SELECT COUNT(*) FROM orders", [], |r| r.get(0))?;
            let issues: i64 = conn.query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))?;
            let accounts: i64 =
                conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
            let mut stmt = conn.prepare("SELECT id, platform_status FROM orders ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let statuses = format!("{rows:?}");
            Ok::<_, rusqlite::Error>((orders, issues, accounts, statuses))
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn stats_趋势分桶_小时粒度空桶补零且与卡值一致() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;

    // 2 小时窗口 → hourly;o1~o5 都在最近 1 小时 → 同一桶
    let from = now - 3600 * 1000;
    let to = now + 3600 * 1000;
    let (status, body) =
        get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["range"]["granularity"], "hourly");
    let trend = body["trend"].as_array().unwrap();
    assert!(
        (2..=3).contains(&trend.len()),
        "2 小时窗口应有 2~3 个小时桶:实际 {trend:?}"
    );
    // 所有桶都有 bucket_start 字段(空桶补零仍存在)
    for p in trend {
        assert!(p["bucket_start"].is_number());
        assert!(p["minor_units"].is_number());
        assert!(p["order_count"].is_number());
    }
    let total_minor: i64 = trend
        .iter()
        .map(|p| p["minor_units"].as_i64().unwrap())
        .sum();
    let total_count: i64 = trend
        .iter()
        .map(|p| p["order_count"].as_i64().unwrap())
        .sum();
    assert_eq!(
        total_minor, body["revenue"]["minor_units"],
        "Σ桶营收 = 卡值"
    );
    assert_eq!(
        total_count, body["revenue"]["order_count"],
        "Σ桶订单数 = 卡值"
    );
}

#[tokio::test]
async fn stats_趋势分桶_日粒度自然日边界() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;

    // 7 自然日窗口 → daily;起点为本地自然日 00:00
    let from = qing_delivery::domain::time_util::local_day_start_ms(now, 6);
    let to = from + 7 * 24 * 3600 * 1000;
    let (status, body) =
        get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["range"]["granularity"], "daily");
    let trend = body["trend"].as_array().unwrap();
    assert_eq!(trend.len(), 7, "回退 6 天+今天共 7 个日桶");

    // 桶边界必须为本地自然日 00:00:回读再对齐应不变
    let first = trend[0]["bucket_start"].as_i64().unwrap();
    assert_eq!(first, from, "首桶起点 = 区间起点(已是本地零点)");
    for pair in trend.windows(2) {
        let s0 = pair[0]["bucket_start"].as_i64().unwrap();
        let s1 = pair[1]["bucket_start"].as_i64().unwrap();
        assert!(s1 > s0, "桶起点严格递增");
    }

    // Σ桶 = 卡值(o1~o5 今日 1800/5 单 + o6 昨日 600/1 单)
    let total_minor: i64 = trend
        .iter()
        .map(|p| p["minor_units"].as_i64().unwrap())
        .sum();
    let total_count: i64 = trend
        .iter()
        .map(|p| p["order_count"].as_i64().unwrap())
        .sum();
    assert_eq!(total_minor, 2_400);
    assert_eq!(total_count, 6);
    assert_eq!(total_minor, body["revenue"]["minor_units"]);
    assert_eq!(total_count, body["revenue"]["order_count"]);
}

#[tokio::test]
async fn stats_只读不变式_查询前后库零变化() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    seed_orders(&app, now).await;

    let before = db_snapshot(&app).await;

    let from = now - 3600 * 1000;
    let to = now + 3600 * 1000;
    for _ in 0..3 {
        let (status, _) =
            get_stats_raw(&app, &cookie, format!("?from={from}&to={to}").as_str()).await;
        assert_eq!(status, StatusCode::OK);
    }
    let after = db_snapshot(&app).await;
    assert_eq!(before, after, "统计必须严格只读(FR-012)");
}
