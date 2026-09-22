//! 脱敏合成协议夹具:WS 同步帧的编码与样本。
//! 规范格式(项目自定义,不复制上游实现):
//!   帧 = base64( msgpack map{ 1: opcode(str), 2: [entry...], 3: nonce(str) } )
//!   entry = msgpack map{ 1: kind(str), 2: payload(str, JSON), 3?: source_event_id(str) }
//! kind 取值与 contracts/platform-adapter.md §3 规范事件对应;
//! "malformed" 用于验证坏条目不吞同帧后续有效条目。

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rmpv::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct FixtureEntry {
    pub kind: String,
    pub payload_json: String,
    pub source_event_id: Option<String>,
}

impl FixtureEntry {
    pub fn payment(order_id: &str, buyer_id: &str, paid_at_ms: i64) -> Self {
        Self {
            kind: "order_payment_signal".into(),
            payload_json: serde_json::json!({
                "order_id": order_id,
                "buyer_id": buyer_id,
                "paid_at_ms": paid_at_ms,
                "trade_type": "ordinary"
            })
            .to_string(),
            source_event_id: Some(format!("evt-{order_id}-pay")),
        }
    }

    pub fn order_state(order_id: &str, state: &str) -> Self {
        Self {
            kind: "order_state_signal".into(),
            payload_json: serde_json::json!({
                "order_id": order_id,
                "platform_state": state
            })
            .to_string(),
            source_event_id: Some(format!("evt-{order_id}-{state}")),
        }
    }

    /// 普通聊天噪声:不应触发任何付款事实(FR-010)
    pub fn chat(from: &str, text: &str) -> Self {
        Self {
            kind: "chat".into(),
            payload_json: serde_json::json!({ "from": from, "text": text }).to_string(),
            source_event_id: None,
        }
    }

    /// 坏条目:载荷不是合法 JSON
    pub fn malformed() -> Self {
        Self {
            kind: "order_payment_signal".into(),
            payload_json: "{not-json".into(),
            source_event_id: Some("evt-malformed".into()),
        }
    }
}

fn entry_to_value(entry: &FixtureEntry) -> Value {
    let mut map: Vec<(Value, Value)> = vec![
        (
            Value::Integer(1.into()),
            Value::String(entry.kind.as_str().into()),
        ),
        (
            Value::Integer(2.into()),
            Value::String(entry.payload_json.as_str().into()),
        ),
    ];
    if let Some(id) = &entry.source_event_id {
        map.push((Value::Integer(3.into()), Value::String(id.as_str().into())));
    }
    Value::Map(map)
}

/// 编码一帧同步数据为 base64(msgpack)。
pub fn encode_sync_frame(opcode: &str, entries: &[FixtureEntry]) -> String {
    let value = Value::Map(vec![
        (Value::Integer(1.into()), Value::String(opcode.into())),
        (
            Value::Integer(2.into()),
            Value::Array(entries.iter().map(entry_to_value).collect()),
        ),
        (
            Value::Integer(3.into()),
            Value::String(uuid::Uuid::new_v4().to_string().into()),
        ),
    ]);
    let mut buf: Vec<u8> = Vec::new();
    rmpv::encode::write_value(&mut buf, &value).expect("msgpack 编码不应失败");
    B64.encode(buf)
}

/// 测试侧解码(用于断言;生产解码由 src/adapters/xianyu/codec 实现,
/// 须施加大小/深度/条目数界限)。
pub fn decode_sync_frame(frame: &str) -> Vec<FixtureEntry> {
    let raw = B64.decode(frame).expect("测试帧应为合法 base64");
    let mut cursor = std::io::Cursor::new(raw);
    let value = rmpv::decode::read_value(&mut cursor).expect("测试帧应为合法 msgpack");
    let Value::Map(pairs) = value else {
        panic!("测试帧应为 msgpack map");
    };
    let entries_value = pairs
        .iter()
        .find(|(k, _)| k.as_u64() == Some(2))
        .map(|(_, v)| v.clone())
        .expect("测试帧应含整数键 2(条目列表)");
    let Value::Array(items) = entries_value else {
        panic!("条目应为数组");
    };
    items
        .into_iter()
        .map(|item| {
            let Value::Map(pairs) = item else {
                panic!("条目应为 map");
            };
            let get = |key: u64| -> Option<Value> {
                pairs
                    .iter()
                    .find(|(k, _)| k.as_u64() == Some(key))
                    .map(|(_, v)| v.clone())
            };
            FixtureEntry {
                kind: get(1)
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default(),
                payload_json: get(2)
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default(),
                source_event_id: get(3).and_then(|v| v.as_str().map(|s| s.to_string())),
            }
        })
        .collect()
}

/// 标准夹具集:一批通知含多笔订单+聊天+坏条目(V05 场景基础)。
pub fn batch_mixed_sample() -> Vec<FixtureEntry> {
    vec![
        FixtureEntry::payment("ORD-0001", "buyer-a", 1_760_000_000_000),
        FixtureEntry::chat("buyer-a", "我付款了,麻烦发货"),
        FixtureEntry::malformed(),
        FixtureEntry::payment("ORD-0002", "buyer-b", 1_760_000_001_000),
        FixtureEntry::order_state("ORD-0001", "pending_ship"),
    ]
}
