//! T045 WS 生命周期集成测试:真实网络边界(本地假平台)上验证
//! 连接→帧下发→逐条事件转发→坏条目不吞后续→ACK 一次→停止退出。

use qing_delivery::adapters::xianyu::ws::client::{WsSessionConfig, run_session};
use qing_delivery::application::ports::platform::PlatformEvent;
use tokio::sync::mpsc;

use crate::support::fake_platform::FakePlatform;
use crate::support::fixtures;

async fn spawn_fake() -> FakePlatform {
    FakePlatform::spawn().await.expect("假平台启动")
}

#[tokio::test]
async fn forwards_all_entries_and_acks_each_frame() {
    let platform = spawn_fake().await;
    let frame = fixtures::encode_sync_frame("sync", &fixtures::batch_mixed_sample());
    platform.script_ws_push(frame.clone());

    let (tx, mut rx) = mpsc::channel::<PlatformEvent>(64);
    let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let cfg = WsSessionConfig {
        url: platform.ws_url(),
        account_id: "acct-1".into(),
        credential_generation: 1,
    };
    let session = tokio::spawn(async move { run_session(cfg, tx, stop_rx).await });

    let mut payments = 0;
    let mut states = 0;
    let mut issues = 0;
    let mut chats = 0;
    // 批次:付款×2、聊天×1、坏条目×1、状态×1 → 事件 4 条(聊天不转发)
    while payments + states + issues < 4 {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("等待事件超时")
            .expect("通道关闭");
        match ev {
            PlatformEvent::OrderPaymentSignal { .. } => payments += 1,
            PlatformEvent::OrderStateSignal { .. } => states += 1,
            PlatformEvent::ProtocolIssue { .. } => issues += 1,
            PlatformEvent::TraceGap { .. } => issues += 1,
            _ => chats += 1,
        }
    }
    assert_eq!(payments, 2, "两笔付款候选都应转发");
    assert_eq!(states, 1);
    assert_eq!(issues, 1, "坏条目记 ProtocolIssue 且不吞后续");
    assert_eq!(chats, 0, "普通聊天不转换为业务事件");

    // ACK:客户端对每帧回执一次(带原始帧内容);轮询等待服务端记录
    let mut received = Vec::new();
    for _ in 0..50 {
        received = platform.received();
        if !received.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(received.len(), 1, "一帧一条 ACK");
    assert!(
        received[0].text.starts_with("ack:"),
        "ACK 应携带原始帧关联内容"
    );

    // 停止信号 → 会话正常退出
    _stop_tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), session)
        .await
        .expect("会话应退出")
        .expect("join 失败")
        .expect("会话内部错误");
}

#[tokio::test]
async fn disconnect_is_normal_exit() {
    let platform = spawn_fake().await;
    let (tx, _rx) = mpsc::channel::<PlatformEvent>(8);
    let (_stop, stop_rx) = tokio::sync::watch::channel(false);
    let cfg = WsSessionConfig {
        url: platform.ws_url(),
        account_id: "acct-1".into(),
        credential_generation: 1,
    };
    let handle = tokio::spawn(async move { run_session(cfg, tx, stop_rx).await });
    // 服务停止 → 客户端读泵结束 → 会话正常返回(重连是上层职责)
    platform.stop();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("应退出");
    assert!(result.is_ok());
}

/// 客户端不发未授权业务消息:读泵期间唯一出站是 ACK。
#[tokio::test]
async fn outbound_traffic_is_ack_only() {
    let platform = spawn_fake().await;
    let frame = fixtures::encode_sync_frame(
        "sync",
        &[fixtures::FixtureEntry::payment("ORD-9", "buyer-9", 1)],
    );
    platform.script_ws_push(frame);

    // 直接用 tungstenite 连接做对照:假平台只应收到 ack 前缀消息
    let (tx, mut rx) = mpsc::channel::<PlatformEvent>(16);
    let (_stop, stop_rx) = tokio::sync::watch::channel(false);
    let cfg = WsSessionConfig {
        url: platform.ws_url(),
        account_id: "a".into(),
        credential_generation: 0,
    };
    let session = tokio::spawn(async move { run_session(cfg, tx, stop_rx).await });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
    let received = platform.received();
    assert!(received.iter().all(|r| r.text.starts_with("ack:")));
    _stop.send(true).unwrap();
    let _ = session.await;
}
