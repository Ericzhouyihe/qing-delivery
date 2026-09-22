//! research R8 依赖最小实验:在真实 Windows 环境验证关键依赖组合可用。
//! 结果(含版本)记录于 docs/deps-validation.md;本文件是可重复的实验本体。

mod support;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// E1: tokio-tungstenite 客户端 × 假平台 WS 服务,帧往返 + msgpack/base64 夹具编解码。
#[tokio::test]
async fn ws_client_against_fake_platform_roundtrip() {
    let platform = support::fake_platform::FakePlatform::spawn()
        .await
        .expect("假平台启动");
    let frame =
        support::fixtures::encode_sync_frame("sync", &support::fixtures::batch_mixed_sample());
    platform.script_ws_push(frame.clone());

    let (mut ws, _resp) = tokio_tungstenite::connect_async(platform.ws_url())
        .await
        .expect("WS 连接");
    let pushed = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("等待下发帧超时")
        .expect("流结束")
        .expect("读取失败");
    let Message::Text(text) = pushed else {
        panic!("应为文本帧");
    };

    let entries = support::fixtures::decode_sync_frame(&text);
    assert_eq!(entries.len(), 5, "同批全部条目应可解码");
    assert_eq!(
        entries.first().expect("首条目").kind,
        "order_payment_signal"
    );

    ws.send(Message::Text("client-ping".into()))
        .await
        .expect("发送");
    let ack = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("等待 ACK 超时")
        .expect("流结束")
        .expect("读取失败");
    assert!(matches!(ack, Message::Text(_)), "应收到 ACK");
    assert_eq!(platform.received().len(), 1);
}

/// E2: reqwest(rustls)× 假平台 HTTP 脚本化响应。
#[tokio::test]
async fn reqwest_against_fake_platform_http() {
    let platform = support::fake_platform::FakePlatform::spawn()
        .await
        .expect("假平台启动");
    platform.script_http(
        "/mtop/item/list",
        support::fake_platform::ScriptedResponse::json(r#"{"data":{"cardList":[]}}"#),
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(platform.url("/mtop/item/list"))
        .send()
        .await
        .expect("HTTP 请求");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON 解析");
    assert!(
        body["data"]["cardList"].as_array().unwrap().is_empty(),
        "省略列表视为空页"
    );
}

/// E3: rusqlite bundled + WAL/FULL + online backup API。
#[test]
fn sqlite_wal_full_and_backup() {
    let dir = tempfile::tempdir().expect("临时目录");
    let main_path = dir.path().join("main.db");
    let backup_path = dir.path().join("backup.db");

    let conn = rusqlite::Connection::open(&main_path).expect("打开");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("WAL");
    conn.pragma_update(None, "synchronous", "FULL")
        .expect("FULL");
    conn.pragma_update(None, "foreign_keys", "ON").expect("FK");
    conn.pragma_update(None, "busy_timeout", 2000)
        .expect("busy_timeout");
    conn.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES ('ok');",
    )
    .expect("建表写入");

    let mut dst = rusqlite::Connection::open(&backup_path).expect("打开备份");
    {
        let bk = rusqlite::backup::Backup::new(&conn, &mut dst).expect("backup 句柄");
        bk.run_to_completion(16, std::time::Duration::from_millis(1), None)
            .expect("备份完成");
    }

    let copied: String = dst
        .query_row("SELECT v FROM t WHERE id = 1", [], |r| r.get(0))
        .expect("查询备份");
    assert_eq!(copied, "ok");
}

/// E4: Windows DPAPI 当前用户包装/解包往返(经适配器)。
#[test]
#[cfg(windows)]
fn dpapi_roundtrip_current_user() {
    let secret: Vec<u8> = b"data-key-material-0123456789".to_vec();
    let protected = qing_delivery::adapters::windows::dpapi::protect_current_user(&secret)
        .expect("CryptProtectData");
    assert!(!protected.is_empty(), "密文不应为空");
    assert_ne!(protected, secret, "密文不应等于明文");
    let back = qing_delivery::adapters::windows::dpapi::unprotect_current_user(&protected)
        .expect("CryptUnprotectData");
    assert_eq!(back, secret, "DPAPI 往返应还原明文");
}

/// E5: include_dir 静态嵌入(发行版嵌入前端产物的机制)。
#[test]
fn include_dir_embeds_fixtures() {
    static FIXTURES: include_dir::Dir =
        include_dir::include_dir!("$CARGO_MANIFEST_DIR/tests/fixtures");
    assert!(
        FIXTURES.get_file("README.md").is_some(),
        "应能嵌入并读取文件"
    );
}
