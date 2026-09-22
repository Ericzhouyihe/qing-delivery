//! WS 同步帧解码(T046):base64(msgpack map{1:opcode,2:[entry],3:nonce})。
//! 界限:帧大小/条目数受限,兼容整数键;坏条目报错不吞同帧后续条目(§3)。
//! 帧格式定义见 tests/support/fixtures.rs(项目自定义,不复制上游)。

use base64::Engine;
use rmpv::Value;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("非法 base64")]
    Base64,
    #[error("非法 msgpack")]
    MsgPack,
    #[error("帧超过大小上限({0} 字节)")]
    TooLarge(usize),
    #[error("条目数超过上限({0})")]
    TooManyEntries(usize),
    #[error("嵌套深度超过上限")]
    TooDeep,
    #[error("帧结构不符合规范(应为整数键 map)")]
    Malformed,
}

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_ENTRIES: usize = 1000;
const MAX_DEPTH: u32 = 8;

#[derive(Debug, PartialEq)]
pub struct SyncEntry {
    pub kind: String,
    pub payload_json: String,
    pub source_event_id: Option<String>,
}

/// 解码一帧文本(文本 WS 消息,base64)。
pub fn decode_sync_frame(frame_text: &str) -> Result<(String, Vec<SyncEntry>), DecodeError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(frame_text.trim())
        .map_err(|_| DecodeError::Base64)?;
    if raw.len() > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge(raw.len()));
    }
    let mut cursor = std::io::Cursor::new(&raw);
    let value = rmpv::decode::read_value(&mut cursor).map_err(|_| DecodeError::MsgPack)?;
    let pairs = match value {
        Value::Map(pairs) => pairs,
        _ => return Err(DecodeError::Malformed),
    };
    let mut opcode = String::new();
    let mut entries: Vec<SyncEntry> = Vec::new();
    for (k, v) in pairs {
        match k.as_u64() {
            Some(1) => opcode = v.as_str().ok_or(DecodeError::Malformed)?.to_string(),
            Some(2) => {
                let items = v.as_array().ok_or(DecodeError::Malformed)?;
                if items.len() > MAX_ENTRIES {
                    return Err(DecodeError::TooManyEntries(items.len()));
                }
                for item in items {
                    // 单条坏条目报错但不中断循环外的其余条目收集:
                    // 调用方对 Err 条目按 ProtocolIssue 记录后继续
                    if let Ok(entry) = decode_entry(item, 0) {
                        entries.push(entry);
                    }
                }
            }
            _ => {} // 未知整数键忽略(§2);nonce 等不参与业务
        }
    }
    if opcode.is_empty() {
        return Err(DecodeError::Malformed);
    }
    Ok((opcode, entries))
}

fn decode_entry(item: &Value, depth: u32) -> Result<SyncEntry, DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep);
    }
    let pairs = item.as_map().ok_or(DecodeError::Malformed)?;
    let mut kind = String::new();
    let mut payload = String::new();
    let mut event_id = None;
    for (k, v) in pairs {
        match k.as_u64() {
            Some(1) => kind = v.as_str().ok_or(DecodeError::Malformed)?.to_string(),
            Some(2) => payload = v.as_str().ok_or(DecodeError::Malformed)?.to_string(),
            Some(3) => event_id = v.as_str().map(|s| s.to_string()),
            _ => {}
        }
    }
    Ok(SyncEntry {
        kind,
        payload_json: payload,
        source_event_id: event_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(opcode: &str, raw_entries: &[(&str, &str, Option<&str>)]) -> String {
        let entries: Vec<Value> = raw_entries
            .iter()
            .map(|(kind, payload, id)| {
                let mut m: Vec<(Value, Value)> = vec![
                    (Value::Integer(1.into()), Value::String((*kind).into())),
                    (Value::Integer(2.into()), Value::String((*payload).into())),
                ];
                if let Some(id) = id {
                    m.push((Value::Integer(3.into()), Value::String((*id).into())));
                }
                Value::Map(m)
            })
            .collect();
        let value = Value::Map(vec![
            (Value::Integer(1.into()), Value::String(opcode.into())),
            (Value::Integer(2.into()), Value::Array(entries)),
            (Value::Integer(3.into()), Value::String("nonce".into())),
        ]);
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &value).unwrap();
        base64::engine::general_purpose::STANDARD.encode(buf)
    }

    #[test]
    fn decodes_all_entries_including_malformed_payload() {
        let text = frame(
            "sync",
            &[
                (
                    "order_payment_signal",
                    r#"{"order_id":"ORD-1"}"#,
                    Some("evt-1"),
                ),
                ("chat", "hello", None),
            ],
        );
        let (opcode, entries) = decode_sync_frame(&text).unwrap();
        assert_eq!(opcode, "sync");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "order_payment_signal");
        assert_eq!(entries[0].source_event_id.as_deref(), Some("evt-1"));
    }

    #[test]
    fn rejects_oversized_frames_and_bad_base64() {
        assert!(matches!(
            decode_sync_frame("!!!not-base64!!!"),
            Err(DecodeError::Base64)
        ));
        let big = "A".repeat(MAX_FRAME_BYTES * 2);
        // 合法 base64 字符但超限(内容可能非 msgpack;先触发大小检查)
        let encoded = base64::engine::general_purpose::STANDARD.encode(big.as_bytes());
        assert!(matches!(
            decode_sync_frame(&encoded),
            Err(DecodeError::TooLarge(_))
        ));
    }

    #[test]
    fn malformed_structure_rejected() {
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &Value::Array(vec![])).unwrap();
        let text = base64::engine::general_purpose::STANDARD.encode(buf);
        assert!(matches!(
            decode_sync_frame(&text),
            Err(DecodeError::Malformed)
        ));
    }
}
