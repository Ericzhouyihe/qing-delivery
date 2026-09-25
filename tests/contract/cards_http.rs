//! US1 卡密库存 HTTP 契约测试(T013,contracts §1 端点子集):
//! 鉴权、创建/列表/详情(不回明文)、追加、条目分页、版本冲突、
//! 批量导入(multipart,逐行报告/413)、test-api 校验拒绝、被引用删除 409。

use crate::support::app::spawn_app;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";

struct Session {
    cookie: String,
    csrf: String,
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn login(app: &axum::Router) -> Session {
    let anon = app
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
    let csrf1 = body_json(anon).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    let init = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/initialize")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf1)
                .body(Body::from(
                    r#"{"password":"a-long-password-123","password_confirmation":"a-long-password-123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(init.status(), StatusCode::CREATED);
    let set_cookie = init
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let cookie = set_cookie["qing_session=".len()..]
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = body_json(init).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    Session { cookie, csrf }
}

fn authed(builder: axum::http::request::Builder, s: &Session) -> axum::http::request::Builder {
    builder
        .header(header::COOKIE, format!("qing_session={}", s.cookie))
        .header("x-csrf-token", &s.csrf)
}

#[tokio::test]
async fn 未认证访问卡密端点被拒() {
    let app = spawn_app().await;
    let resp = app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/card-pools")
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 卡密组全生命周期与引用保护契约() {
    let app = spawn_app().await;
    let s = login(&app.router).await;

    // 创建 data 组(3 条,含 1 空行)→ 201,stock.available=2,无明文字段
    let create = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/card-pools")
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                &s,
            )
            .body(Body::from(
                r#"{"name":"网课卡池","kind":"data","enabled":true,"delay_seconds":0,
                    "description":"导入","entries":["K-1","K-2","  "]}"#,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED, "创建应 201");
    let created = body_json(create).await;
    assert_eq!(created["stock"]["available"], 2, "空行不入库");
    assert!(created.get("entries").is_none(), "响应不得携带明文条目");
    let pool_id = created["id"].as_str().unwrap().to_string();

    // 列表 kind/search 过滤(查询参数需百分号编码)
    let list = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .uri("/api/v1/card-pools?kind=data&search=%E7%BD%91%E8%AF%BE")
                    .header(header::HOST, HOST),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = body_json(list).await;
    assert_eq!(list_body["items"].as_array().unwrap().len(), 1);
    // 非法 kind → invalid_request(本项目错误表:400)
    let bad = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .uri("/api/v1/card-pools?kind=voice")
                    .header(header::HOST, HOST),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(bad).await["error"]["code"], "invalid_request");

    // 追加:1 新卡 + 1 重复 + 1 空行
    let append = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/card-pools/{pool_id}/append-data"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                &s,
            )
            .body(Body::from(r#"{"lines":["K-3","K-1",""]}"#))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(append.status(), StatusCode::OK);
    let appended = body_json(append).await;
    assert_eq!(appended["appended"], 1);
    assert_eq!(appended["skipped_empty"], 1);
    assert_eq!(appended["skipped_duplicate"], 1);

    // 详情:分状态计数
    let detail = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .uri(format!("/api/v1/card-pools/{pool_id}"))
                    .header(header::HOST, HOST),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let detail_body = body_json(detail).await;
    assert_eq!(detail_body["stock"]["available"], 3);

    // 条目分页:state 过滤;永不携带密文/完整摘要
    let entries = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .uri(format!("/api/v1/card-pools/{pool_id}/entries?state=available&limit=2"))
                    .header(header::HOST, HOST),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let entries_body = body_json(entries).await;
    let items = entries_body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "limit=2 应返回 2 条");
    assert!(entries_body["next_cursor"].is_string());
    assert!(items[0]["content_digest_prefix"].as_str().unwrap().len() == 12);
    assert!(items[0].get("content_ciphertext").is_none());

    // 版本冲突:错误 expected_version → 409
    let stale = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/card-pools/{pool_id}"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                &s,
            )
            .body(Body::from(
                r#"{"name":"网课卡池","kind":"data","enabled":false,"delay_seconds":0,
                    "description":"","expected_version":999}"#,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(stale).await["error"]["code"], "version_conflict");

    // 被规则引用 → 删除 409 referenced_resource
    {
        let pid = pool_id.clone();
        app.db
            .call(move |conn| {
                use qing_delivery::adapters::sqlite::repos::{accounts, items, rules};
                let acct =
                    accounts::upsert_identity(conn, "acct-c", "xianyu", "seller-c", "卖家")?;
                let item = items::upsert(
                    conn, "item-c", &acct.id, "EXT-C", "商品", "on_sale", "[]", "single",
                )?;
                let rule = rules::insert(conn, "rule-c", &acct.id, &item.id, "single", true)?;
                conn.execute(
                    "UPDATE rules SET card_pool_id = ?1 WHERE id = ?2",
                    rusqlite::params![pid, rule.id],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap()
            .unwrap();
    }
    let refused = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/card-pools/{pool_id}"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(refused).await["error"]["code"],
        "referenced_resource"
    );
    // 解除引用后删除 → 204
    app.db
        .call(|conn| {
            conn.execute("UPDATE rules SET card_pool_id = NULL WHERE id = 'rule-c'", [])
        })
        .await
        .unwrap()
        .unwrap();
    let deleted = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/card-pools/{pool_id}"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn 批量导入与test_api契约() {
    let app = spawn_app().await;
    let s = login(&app.router).await;

    // multipart:csv 两好一坏
    let boundary = "----qdtest";
    let csv = "名称,类型,内容,描述,启用,延迟秒\n契约组A,批量,HTTP-1,,是,0\n坏行,voice,x,,,\n契约组B,文本,固定内容,,否,\n";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"import.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--{b}--\r\n",
        b = boundary
    );
    let import = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/card-pools/batch-import")
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    ),
                &s,
            )
            .body(Body::from(body))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(import.status(), StatusCode::OK);
    let report = body_json(import).await;
    assert_eq!(report["total"], 3);
    assert_eq!(report["succeeded"], 2);
    assert_eq!(report["failed"].as_array().unwrap().len(), 1);
    assert_eq!(report["failed"][0]["row"], 3);

    // 超限文件 → 413 payload_too_large
    let big_payload = vec![b'a'; 3 * 1024 * 1024];
    let big = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.csv\"\r\n\r\n",
        b = boundary
    );
    let mut big_body = big.into_bytes();
    big_body.extend_from_slice(&big_payload);
    big_body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let oversized = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/card-pools/batch-import")
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    ),
                &s,
            )
            .body(Body::from(big_body))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body_json(oversized).await["error"]["code"],
        "payload_too_large"
    );

    // test-api:非法配置 invalid_request(校验先行,不发起网络请求;错误表 400)
    let invalid = app.router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/card-pools/test-api")
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                &s,
            )
            .body(Body::from(
                r#"{"url":"not a url","method":"PUT","timeout_ms":999,"response_path":""}"#,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(invalid).await["error"]["code"], "invalid_request");
}
