//! 真实闲鱼 WS 客户端(T045):wss-goofish.dingtalk.com 私有 lwp 协议。
//! 连接→/reg 注册→心跳 15s;请求按 mid 首段关联;push 帧按原 headers 回执 ACK 一次;
//! sync 载荷逐条 base64→JSON/msgpack 解码;ACK ≠ 业务提交(§6)。
//! 断线/踢出是正常返回——重连退避由账号运行时决定。

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{ORIGIN, USER_AGENT};

use crate::domain::time_util::utc_now_ms;

/// 解码后的入站载荷:业务语义提取由适配层完成(宪章 II)。
#[derive(Clone, Debug)]
pub enum Inbound {
    Message(Value),
    DecodeError(String),
}

#[derive(Clone, Debug)]
pub struct SyncMessage {
    pub account_id: String,
    pub credential_generation: i64,
    pub inbound: Inbound,
}

pub const WS_URL: &str = "wss://wss-goofish.dingtalk.com:443";
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REG_APP_KEY: &str = crate::adapters::xianyu::mtop::client::REG_APP_KEY;

/// mid = <0-999 随机><UnixMilli> + " 0";响应按首段空白前缀关联。
pub fn generate_mid() -> String {
    let rand_part = rand::random::<u32>() % 1000;
    format!("{rand_part}{} 0", utc_now_ms())
}

/// 发送消息 uuid = "-" + UnixMilli + "1"。
pub fn generate_msg_uuid() -> String {
    format!("-{}1", utc_now_ms())
}

/// /reg 的 ua 头:浏览器指纹 + 钉钉 IMPaaS 标识。
pub fn registration_ua() -> String {
    format!(
        "{BROWSER_UA} DingTalk(2.2.0) OS(Windows/10) Browser(Chrome/131.0.0.0) \
     DingWeb/2.2.0 IMPaaS DingWeb/2.2.0"
    )
}

#[derive(Debug, thiserror::Error)]
pub enum WsRpcError {
    #[error("连接已关闭")]
    NotConnected,
    #[error("请求超时")]
    Timeout,
    #[error("发送失败:{0}")]
    Send(String),
    #[error("协议错误:{0}")]
    Protocol(String),
}

pub enum Command {
    Request {
        path: String,
        headers: HashMap<String, Value>,
        body: Option<Value>,
        resp: oneshot::Sender<Result<Value, WsRpcError>>,
    },
}

/// 已建立注册连接的句柄:请求经通道写入,响应按 mid 关联返回。
#[derive(Clone)]
pub struct WsHandle {
    cmd_tx: mpsc::Sender<Command>,
}

impl WsHandle {
    pub fn new(cmd_tx: mpsc::Sender<Command>) -> Self {
        Self { cmd_tx }
    }
}

impl WsHandle {
    pub async fn request(
        &self,
        path: &str,
        headers: HashMap<String, Value>,
        body: Option<Value>,
    ) -> Result<Value, WsRpcError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::Request {
                path: path.to_string(),
                headers,
                body,
                resp: resp_tx,
            })
            .await
            .map_err(|_| WsRpcError::NotConnected)?;
        match tokio::time::timeout(REQUEST_TIMEOUT, resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(WsRpcError::NotConnected),
            Err(_) => Err(WsRpcError::Timeout),
        }
    }
}

/// 会话终止原因;外层据此决定重连/停止。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// 对端断连/网络错误:按退避重连
    Disconnected,
    /// /reg 拒绝(token 失效):停止并转人工
    AuthExpired,
    /// 连接数超限
    ConnectLimit,
    /// 本地停止信号
    Shutdown,
}

pub struct SessionParams {
    pub account_id: String,
    pub credential_generation: i64,
    /// mtop ws_access_token 返回的 accessToken
    pub access_token: String,
    pub device_id: String,
    pub ws_url: String,
}

/// 连接、注册并运行会话直到终止;事件经 events_tx 转发,请求经 cmd_rx 接入。
/// 返回终止原因——不在此层重连。
pub async fn run_registered_session(
    params: SessionParams,
    msg_tx: mpsc::Sender<SyncMessage>,
    mut cmd_rx: mpsc::Receiver<Command>,
    mut shutdown: watch::Receiver<bool>,
) -> SessionEnd {
    let mut request = match params.ws_url.clone().into_client_request() {
        Ok(r) => r,
        Err(_) => return SessionEnd::Disconnected,
    };
    request
        .headers_mut()
        .insert(ORIGIN, "https://www.goofish.com".parse().unwrap());
    request
        .headers_mut()
        .insert(USER_AGENT, BROWSER_UA.parse().unwrap());

    let (ws, _resp) = match tokio_tungstenite::connect_async(request).await {
        Ok(c) => c,
        Err(_) => return SessionEnd::Disconnected,
    };
    let (mut sink, mut stream) = ws.split();

    // pending 请求表:mid 首段 → 等待方;getState 响应待 ackDiff 的 mid 集合
    let mut pending: HashMap<String, oneshot::Sender<Result<Value, WsRpcError>>> = HashMap::new();
    let mut ack_state_waiting: HashMap<String, ()> = HashMap::new();

    // /reg:成功 code=200(顶层,与 headers 平级);401/invalid token → 授权失效
    let reg_headers: HashMap<String, Value> = [
        ("cache-header".into(), json!("app-key token ua wv")),
        ("app-key".into(), json!(REG_APP_KEY)),
        ("token".into(), json!(params.access_token)),
        ("ua".into(), json!(registration_ua())),
        ("dt".into(), json!("j")),
        ("wv".into(), json!("im:3,au:3,sy:6")),
        ("sync".into(), json!("0,0;0;0;")),
        ("did".into(), json!(params.device_id)),
    ]
    .into_iter()
    .collect();
    let reg_mid = generate_mid();
    let reg_frame = build_frame("/reg", reg_headers, None, Some(reg_mid.clone()));
    if sink.send(Message::Text(reg_frame.into())).await.is_err() {
        return SessionEnd::Disconnected;
    }

    // 等待注册响应
    let reg_end = loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break SessionEnd::Shutdown;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(frame))) => {
                        match classify_reg_response(&frame) {
                            Some(end) => break end,
                            None => continue, // 忽略注册前的其他帧
                        }
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => break SessionEnd::Disconnected,
                }
            }
        }
    };
    if reg_end != SessionEnd::Shutdown {
        return reg_end;
    }
    let _ = reg_mid;

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    let _ = sink.send(Message::Close(None)).await;
                    return SessionEnd::Shutdown;
                }
            }
            _ = heartbeat.tick() => {
                // 心跳失败即断线重连(上游行为);响应经 pending 超时自然清理
                let mid = generate_mid();
                let frame = build_frame("/!", HashMap::new(), None, Some(mid));
                if sink.send(Message::Text(frame.into())).await.is_err() {
                    return SessionEnd::Disconnected;
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Request { path, headers, body, resp }) => {
                        let mid = headers.get("mid").and_then(|m| m.as_str()).map(|s| s.to_string())
                            .unwrap_or_else(generate_mid);
                        let frame = build_frame(&path, headers, body, Some(mid.clone()));
                        if sink.send(Message::Text(frame.into())).await.is_err() {
                            let _ = resp.send(Err(WsRpcError::NotConnected));
                            return SessionEnd::Disconnected;
                        }
                        pending.insert(mid_key(&mid), resp);
                    }
                    None => {
                        let _ = sink.send(Message::Close(None)).await;
                        return SessionEnd::Shutdown;
                    }
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(frame))) => {
                        if let IncomingOutcome::End(end) = handle_incoming(
                            &params,
                            &frame,
                            &mut sink,
                            &mut pending,
                            &mut ack_state_waiting,
                            &msg_tx,
                        )
                        .await
                        {
                            return end;
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if sink.send(Message::Pong(p)).await.is_err() {
                            return SessionEnd::Disconnected;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return SessionEnd::Disconnected,
                }
            }
        }
    }
}

/// /reg 响应分类:Some(end) 表示注册失败终止;None 表示继续等。
fn classify_reg_response(frame: &str) -> Option<SessionEnd> {
    let parsed: Value = serde_json::from_str(frame).ok()?;
    if parsed.get("code").is_none() || parsed.get("headers").and_then(|h| h.get("mid")).is_none() {
        return None; // push/无 mid 帧:注册阶段忽略
    }
    let code = parsed.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code == 200 {
        return Some(SessionEnd::Shutdown); // 占位:成功也走 break,外层判 Shutdown 后继续
    }
    let reason = parsed
        .get("headers")
        .and_then(|h| h.get("error"))
        .and_then(|e| e.as_str())
        .unwrap_or_default()
        .to_string();
    if code == 401
        || reason.contains("invalid token")
        || reason.contains("not auth")
        || reason.contains("device id or appkey is not equal")
    {
        return Some(SessionEnd::AuthExpired);
    }
    if reason.contains("connect limit")
        || reason.contains("session remove")
        || reason.contains("too many")
    {
        return Some(SessionEnd::ConnectLimit);
    }
    Some(SessionEnd::Disconnected)
}

enum IncomingOutcome {
    /// 已处理(响应分发/ACK/事件转发)
    Handled,
    /// 无需处理
    Ignored,
    /// 需要终止会话
    End(SessionEnd),
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn handle_incoming(
    params: &SessionParams,
    frame: &str,
    sink: &mut WsSink,
    pending: &mut HashMap<String, oneshot::Sender<Result<Value, WsRpcError>>>,
    ack_state_waiting: &mut HashMap<String, ()>,
    msg_tx: &mpsc::Sender<SyncMessage>,
) -> IncomingOutcome {
    let parsed: Value = match serde_json::from_str(frame) {
        Ok(v) => v,
        Err(_) => return IncomingOutcome::Ignored,
    };
    let headers = parsed.get("headers").filter(|h| h.is_object());

    // 响应帧:顶层 code + headers;按 mid 首段分发完整帧
    if parsed.get("code").is_some() && headers.is_some() {
        if let Some(mid_raw) = headers.and_then(|h| h.get("mid")).and_then(|m| m.as_str()) {
            let key = mid_key(mid_raw);
            // getState 响应:回 ackDiff(body = 该响应 body)
            if ack_state_waiting.remove(&key).is_some() {
                let state_body = parsed.get("body").cloned().unwrap_or(Value::Null);
                let ack_frame = build_frame(
                    "/r/SyncStatus/ackDiff",
                    HashMap::new(),
                    Some(json!([state_body])),
                    None,
                );
                let _ = sink.send(Message::Text(ack_frame.into())).await;
                return IncomingOutcome::Handled;
            }
            if let Some(resp) = pending.remove(&key) {
                let _ = resp.send(Ok(parsed.clone()));
            }
        }
        return IncomingOutcome::Handled;
    }

    let Some(lwp) = parsed.get("lwp").and_then(|l| l.as_str()) else {
        return IncomingOutcome::Ignored;
    };
    let Some(hdrs) = parsed.get("headers").filter(|h| h.is_object()) else {
        return IncomingOutcome::Ignored;
    };

    // push 帧:kickout/超限直接终止
    match lwp {
        "/push/kickout" => return IncomingOutcome::End(SessionEnd::AuthExpired),
        "/s/session/remove" => return IncomingOutcome::End(SessionEnd::ConnectLimit),
        _ => {}
    }

    // ACK:原样复制服务端 headers,code=200;失败不阻塞后续
    let ack = json!({"code": 200, "headers": hdrs.clone()});
    let _ = sink.send(Message::Text(ack.to_string().into())).await;

    let body = parsed.get("body").cloned().unwrap_or(Value::Null);
    // 增量同步(syncExtraType 1|2):先 getState,响应到达后回 ackDiff
    if let Some(sync_type) = body
        .get("syncExtraType")
        .and_then(|s| s.get("type"))
        .and_then(|t| t.as_i64())
        && (sync_type == 1 || sync_type == 2)
    {
        let mid = generate_mid();
        ack_state_waiting.insert(mid_key(&mid), ());
        let frame = build_frame(
            "/r/SyncStatus/getState",
            HashMap::new(),
            Some(json!([{"topic": "sync"}])),
            Some(mid),
        );
        let _ = sink.send(Message::Text(frame.into())).await;
    }

    // 业务载荷:body.syncPushPackage.data[] 逐条解码转发(同帧全部条目,坏条目不吞后续)
    if let Some(entries) = body
        .get("syncPushPackage")
        .and_then(|p| p.get("data"))
        .and_then(|d| d.as_array())
    {
        for entry in entries {
            let Some(data_str) = entry.as_str() else {
                continue;
            };
            let inbound = match decode_sync_entry(data_str) {
                Ok(msg) => Inbound::Message(msg),
                Err(reason) => Inbound::DecodeError(reason),
            };
            // 有界通道满时阻塞读泵(上游有界队列+背压语义)
            let _ = msg_tx
                .send(SyncMessage {
                    account_id: params.account_id.clone(),
                    credential_generation: params.credential_generation,
                    inbound,
                })
                .await;
        }
    }
    IncomingOutcome::Handled
}

fn mid_key(mid: &str) -> String {
    mid.split_whitespace().next().unwrap_or("").to_string()
}

/// 请求帧:lwp + headers(mid 由显式值或自动生成)+ 可选 body(nil body 不输出键)。
fn build_frame(
    path: &str,
    headers: HashMap<String, Value>,
    body: Option<Value>,
    mid: Option<String>,
) -> String {
    let mut map = serde_json::Map::new();
    let mut hdr = serde_json::Map::new();
    for (k, v) in headers {
        hdr.insert(k, v);
    }
    let mid = mid.unwrap_or_else(generate_mid);
    hdr.entry("mid".to_string()).or_insert_with(|| json!(mid));
    map.insert("lwp".into(), json!(path));
    map.insert("headers".into(), Value::Object(hdr));
    if let Some(b) = body {
        map.insert("body".into(), b);
    }
    Value::Object(map).to_string()
}

/// 单条 sync 载荷解码:base64 → JSON(明文系统消息),失败则 base64 → msgpack → 归一 JSON。
pub fn decode_sync_entry(data: &str) -> Result<Value, String> {
    let cleaned: String = data.chars().filter(|c| c.is_ascii()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.trim())
        .map_err(|e| format!("base64 解码失败:{e}"))?;
    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
        return Ok(v);
    }
    let reader = rmpv::decode::value::read_value(&mut bytes.as_slice());
    match reader {
        Ok(v) => rmpv_to_json(&v).ok_or_else(|| "msgpack 归一失败".to_string()),
        Err(_) => {
            // 尝试剔除尾部填充字节后再解(上游兼容行为)
            for cut in (1..bytes.len().min(16)).rev() {
                let mut slice = &bytes[..bytes.len() - cut];
                if let Ok(v) = rmpv::decode::value::read_value(&mut slice)
                    && let Some(json) = rmpv_to_json(&v)
                {
                    return Ok(json);
                }
            }
            Err("msgpack 解码失败".to_string())
        }
    }
}

/// rmpv → JSON 归一:整数键转字符串键,字节串按 UTF-8 损耗转换。
pub fn rmpv_to_json(v: &rmpv::Value) -> Option<Value> {
    let out = match v {
        rmpv::Value::Nil => Value::Null,
        rmpv::Value::Boolean(b) => Value::Bool(*b),
        rmpv::Value::Integer(i) => {
            if let Some(u) = i.as_u64() {
                Value::from(u)
            } else {
                Value::from(i.as_i64()?)
            }
        }
        rmpv::Value::F32(f) => Value::from(*f as f64),
        rmpv::Value::F64(f) => Value::from(*f),
        rmpv::Value::String(s) => Value::String(s.as_str()?.to_string()),
        rmpv::Value::Binary(b) => Value::String(String::from_utf8_lossy(b).to_string()),
        rmpv::Value::Array(items) => {
            Value::Array(items.iter().map(rmpv_to_json).collect::<Option<Vec<_>>>()?)
        }
        rmpv::Value::Map(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, val) in pairs {
                let key = match k {
                    rmpv::Value::String(s) => s.as_str()?.to_string(),
                    rmpv::Value::Integer(i) => i.to_string(),
                    _ => return None,
                };
                map.insert(key, rmpv_to_json(val)?);
            }
            Value::Object(map)
        }
        rmpv::Value::Ext(_, _) => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_format_matches_protocol() {
        let mid = generate_mid();
        assert!(mid.ends_with(" 0"), "mid 以空格+0 结尾:{mid}");
        let first = mid.split_whitespace().next().unwrap();
        assert!(first.len() >= 13, "首段含随机数+毫秒时间戳:{first}");
    }

    #[test]
    fn frame_building_adds_mid_and_omits_empty_body() {
        let mut headers = HashMap::new();
        headers.insert("app-key".to_string(), json!("k"));
        let frame = build_frame("/reg", headers, None, None);
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["lwp"], "/reg");
        assert!(v["headers"]["mid"].is_string());
        assert!(v.get("body").is_none(), "body 为 nil 时不输出键");
    }

    #[test]
    fn reg_response_classification() {
        let ok = json!({"code":200,"headers":{"mid":"1 0"}}).to_string();
        // 成功也以 Shutdown 占位返回(None 表示继续等待)
        assert!(classify_reg_response(&ok).is_some());
        let unauthorized =
            json!({"code":401,"headers":{"mid":"2 0","error":"invalid token"}}).to_string();
        assert_eq!(
            classify_reg_response(&unauthorized),
            Some(SessionEnd::AuthExpired)
        );
        let limit = json!({"code":500,"headers":{"mid":"3 0","error":"connect limit"}}).to_string();
        assert_eq!(
            classify_reg_response(&limit),
            Some(SessionEnd::ConnectLimit)
        );
        let push = json!({"lwp":"/push/x","headers":{"mid":"4"},"body":{}}).to_string();
        assert_eq!(classify_reg_response(&push), None);
    }

    #[test]
    fn sync_entry_json_first_then_msgpack() {
        let json_msg = serde_json::json!({"hello": "世界"});
        let b64 = base64::engine::general_purpose::STANDARD.encode(json_msg.to_string().as_bytes());
        assert_eq!(decode_sync_entry(&b64).unwrap(), json_msg);

        // msgpack:整数键 map → 字符串键 JSON
        let mut buf = Vec::new();
        rmpv::encode::write_value(
            &mut buf,
            &rmpv::Value::Map(vec![
                (rmpv::Value::from(1), rmpv::Value::from("paid")),
                (
                    rmpv::Value::from(3),
                    rmpv::Value::Map(vec![(
                        rmpv::Value::from("redReminder"),
                        rmpv::Value::from("等待卖家发货"),
                    )]),
                ),
            ]),
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        let decoded = decode_sync_entry(&b64).unwrap();
        assert_eq!(decoded["1"], "paid");
        assert_eq!(decoded["3"]["redReminder"], "等待卖家发货");
    }

    #[test]
    fn rmpv_binary_becomes_lossy_string() {
        let v = rmpv::Value::Binary(b"\xff\xfe".to_vec());
        assert_eq!(
            rmpv_to_json(&v).unwrap(),
            Value::String("\u{fffd}\u{fffd}".into())
        );
    }
}
