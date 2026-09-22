//! WS 客户端(T045):连接平台、读泵逐帧解码、按原始帧回执 ACK 一次、
//! 规范事件转发到应用通道。读泵不被发送/详情阻塞;ACK ≠ 业务提交(§3)。

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::adapters::xianyu::codec::decode_sync_frame;
use crate::application::ports::platform::{EventMeta, PlatformEvent};
use crate::domain::time_util::utc_now_ms;

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("连接失败:{0}")]
    Connect(String),
}

pub struct WsSessionConfig {
    pub url: String,
    pub account_id: String,
    pub credential_generation: i64,
}

/// 运行一个读会话直到断连或停止信号;每帧 ACK 一次;事件发往 tx。
/// 断连是正常返回——重连退避由 supervisor 决定。
pub async fn run_session(
    cfg: WsSessionConfig,
    tx: mpsc::Sender<PlatformEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), WsError> {
    let (ws, _resp) = tokio_tungstenite::connect_async(&cfg.url)
        .await
        .map_err(|e| WsError::Connect(e.to_string()))?;
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    // 直接丢弃写半区:停止不等待对端 close 握手
                    drop(sink);
                    return Ok(());
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(frame))) => {
                        // 只有成功解码的同步帧才回执 ACK(按原始帧,一次);
                        // 协议回执/非帧文本不应答,避免 ACK 回声乒乓
                        if decode_sync_frame(&frame).is_ok() {
                            let ack = format!("ack:{frame}");
                            if sink.send(Message::Text(ack.into())).await.is_err() {
                                return Ok(());
                            }
                        }
                        forward_frame(&cfg, &frame, &tx).await;
                    }
                    Some(Ok(_)) => {} // 二进制/心跳帧:当前协议只用文本
                    Some(Err(_)) | None => return Ok(()), // 断连:正常返回,由上层退避重连
                }
            }
        }
    }
}

/// 帧解码与事件映射:坏帧记 ProtocolIssue;同帧其余条目继续(§3)。
async fn forward_frame(cfg: &WsSessionConfig, frame: &str, tx: &mpsc::Sender<PlatformEvent>) {
    let entries = match decode_sync_frame(frame) {
        Ok((_, entries)) => entries,
        Err(e) => {
            let _ = tx
                .send(PlatformEvent::ProtocolIssue {
                    meta: meta(cfg),
                    reason: e.to_string(),
                })
                .await;
            return;
        }
    };
    for entry in entries {
        let meta = EventMeta {
            account_id: cfg.account_id.clone(),
            credential_generation: cfg.credential_generation,
            source_event_id: entry.source_event_id.clone(),
            platform_event_at_ms: None,
            received_at_ms: utc_now_ms(),
        };
        let event = match entry.kind.as_str() {
            "order_payment_signal" => {
                let order_id = json_str(&entry.payload_json, "order_id").unwrap_or_default();
                let buyer = json_str(&entry.payload_json, "buyer_id");
                if order_id.is_empty() {
                    PlatformEvent::ProtocolIssue {
                        meta,
                        reason: "付款候选缺少订单号".into(),
                    }
                } else {
                    PlatformEvent::OrderPaymentSignal {
                        meta,
                        order_id,
                        buyer_hint: buyer,
                    }
                }
            }
            "order_state_signal" => {
                let order_id = json_str(&entry.payload_json, "order_id").unwrap_or_default();
                let state = json_str(&entry.payload_json, "platform_state").unwrap_or_default();
                if order_id.is_empty() || state.is_empty() {
                    PlatformEvent::ProtocolIssue {
                        meta,
                        reason: "状态候选字段缺失".into(),
                    }
                } else {
                    PlatformEvent::OrderStateSignal {
                        meta,
                        order_id,
                        platform_state: state,
                    }
                }
            }
            // 普通聊天不转换为任何业务事件(FR-010)
            "chat" => continue,
            other => PlatformEvent::ProtocolIssue {
                meta,
                reason: format!("未知条目类型:{other}"),
            },
        };
        let _ = tx.send(event).await;
    }
}

fn meta(cfg: &WsSessionConfig) -> EventMeta {
    EventMeta {
        account_id: cfg.account_id.clone(),
        credential_generation: cfg.credential_generation,
        source_event_id: None,
        platform_event_at_ms: None,
        received_at_ms: utc_now_ms(),
    }
}

fn json_str(json: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get(key)?
        .as_str()
        .map(|s| s.to_string())
}
