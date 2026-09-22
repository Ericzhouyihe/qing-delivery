//! 本地 HTTP/WS 假平台:为协议集成测试提供可控的真实网络边界。
//! 页面演示用的进程内模拟适配器在 src/adapters/mock(dev-fixtures feature),
//! 两者不能互相替代(quickstart §4)。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::routing::{any, get};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// 一次脚本化 HTTP 响应(按 path 匹配,先进先出)
#[derive(Clone, Debug)]
pub struct ScriptedResponse {
    pub status: u16,
    pub body: String,
}

impl ScriptedResponse {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }
}

/// WS 侧记录到的客户端消息(文本原文)
#[derive(Clone, Debug)]
pub struct ReceivedFrame {
    pub text: String,
}

#[derive(Default)]
struct Shared {
    http: VecDeque<(String, ScriptedResponse)>,
    /// 连接建立后立即下发的帧(文本,通常为 base64 msgpack)
    ws_push: VecDeque<String>,
    received: Vec<ReceivedFrame>,
}

impl Shared {
    fn pop_http(&mut self, path: &str) -> Option<ScriptedResponse> {
        let idx = self.http.iter().position(|(p, _)| p == path)?;
        let (_, resp) = self.http.remove(idx)?;
        Some(resp)
    }
}

#[derive(Clone)]
pub struct FakePlatform {
    pub addr: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl FakePlatform {
    /// 在随机回环端口启动假平台。
    pub async fn spawn() -> std::io::Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let listener = TcpListener::bind("127.0.0.1:0").await?;

        let ws_shared = Arc::clone(&shared);
        let handle_ws = move |ws: WebSocketUpgrade| {
            let shared = Arc::clone(&ws_shared);
            async move { ws.on_upgrade(move |socket| fake_ws_session(socket, shared)) }
        };

        let http_shared = Arc::clone(&shared);
        let handle_mtop = move |axum::extract::Path(path): axum::extract::Path<String>| {
            let shared = Arc::clone(&http_shared);
            async move {
                let resp = shared.lock().unwrap().pop_http(&format!("/mtop/{path}"));
                match resp {
                    Some(r) => (
                        StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        r.body,
                    ),
                    None => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "no scripted response".to_string(),
                    ),
                }
            }
        };

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/ws", get(handle_ws))
            .route("/mtop/{*path}", any(handle_mtop));

        let (tx, rx) = oneshot::channel::<()>();
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(Mutex::new(Some(tx)));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            shared,
            shutdown,
        })
    }

    /// 脚本化一次 HTTP 响应:path 形如 "/mtop/json/item.list"。
    pub fn script_http(&self, path: &str, resp: ScriptedResponse) {
        self.shared
            .lock()
            .unwrap()
            .http
            .push_back((path.to_string(), resp));
    }

    /// 连接后立即下发一帧(base64 msgpack 文本)。
    pub fn script_ws_push(&self, frame: String) {
        self.shared.lock().unwrap().ws_push.push_back(frame);
    }

    /// 已收到并记录的客户端帧。
    pub fn received(&self) -> Vec<ReceivedFrame> {
        self.shared.lock().unwrap().received.clone()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.addr)
    }

    /// 停止服务;等待 server 任务收尾由调用方 join。
    pub fn stop(&self) {
        if let Some(tx) = self.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for FakePlatform {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn fake_ws_session(mut socket: WebSocket, shared: Arc<Mutex<Shared>>) {
    // 连接即下发脚本化帧
    loop {
        let next = { shared.lock().unwrap().ws_push.pop_front() };
        match next {
            Some(frame) => {
                if socket.send(Message::Text(frame.into())).await.is_err() {
                    return;
                }
            }
            None => break,
        }
    }
    // 记录客户端帧并逐帧 ACK;ACK 只是传输层事实,不代表业务提交(platform-adapter §3)
    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(text) = msg {
            shared.lock().unwrap().received.push(ReceivedFrame {
                text: text.to_string(),
            });
            let ack = format!("{{\"ack\":true,\"of\":{}}}", text.len());
            if socket.send(Message::Text(ack.into())).await.is_err() {
                return;
            }
        }
    }
}
