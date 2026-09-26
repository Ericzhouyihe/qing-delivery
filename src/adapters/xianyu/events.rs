//! sync 消息 → 规范事件提取:系统事件门禁 + 付款文案 + 业务键。
//! 普通聊天(messageDirection=2)不得转换为付款事件(FR-010);
//! 订单号取 updateKey 次段或文案/链接中的 orderId。
//! 007 D5/T045:买家方向普通聊天不再丢弃,转为 ChatMessage 事实进聊天域。

use serde_json::Value;
use sha2::Digest;

use crate::application::ports::platform::{EventMeta, PlatformEvent};
use crate::domain::chat::ChatMsgKind;
use crate::domain::time_util::utc_now_ms;

/// 付款文案标记(上游提取的行为子集;不复制实现)
const PAID_MARKERS: [&str; 4] = [
    "[我已付款，等待你发货]",
    "[已付款，待发货]",
    "我已付款，等待你发货",
    "[记得及时发货]",
];
const COMPLETE_MARKERS: [&str; 2] = ["快给ta一个评价吧", "买家已确认收货"];

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MessageKind {
    /// 系统付款候选(仅触发核验,不是发货凭证)
    PaidOrder {
        order_id: String,
        buyer_hint: Option<String>,
    },
    /// 买家确认收货/交易完成
    OrderCompleted { order_id: String },
    /// 买家方向普通聊天(007 T045):进入聊天域,不触发订单事件
    ChatMessage {
        chat_id: Option<String>,
        buyer_id: Option<String>,
        message_id: String,
        kind: ChatMsgKind,
        text: Option<String>,
        image_url: Option<String>,
        item_id: Option<String>,
    },
    /// 无业务含义消息(非买家方向普通消息/未识别系统消息):不产生事件
    #[default]
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Extracted {
    pub kind: MessageKind,
    pub chat_id: Option<String>,
    pub buyer_id: Option<String>,
    pub item_id: Option<String>,
}

/// 提取一条解码后的 sync 消息的业务语义。
pub fn extract(msg: &Value) -> Extracted {
    let mut out = Extracted::default();

    let root_1 = msg.get("1");
    let direction = root_1
        .and_then(|r| r.get("7"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let content_type = root_1
        .and_then(|r| r.get("6"))
        .and_then(|c| c.get("3"))
        .and_then(|c| c.get("4"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let header_10 = root_1.and_then(|r| r.get("10"));
    let ext_json_raw = header_10
        .and_then(|h| h.get("extJson"))
        .and_then(|v| v.as_str());
    let ext_json = ext_json_raw
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    let reminder_content = header_10
        .and_then(|h| h.get("reminderContent"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let simplified_red = msg
        .get("3")
        .and_then(|r| r.get("redReminder"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let content_json = root_1
        .and_then(|r| r.get("6"))
        .and_then(|c| c.get("3"))
        .and_then(|c| c.get("5"))
        .cloned()
        .unwrap_or(Value::Null);
    // 消息正文(文本):嵌套 1.6.3.2(参考实现同路径)
    let raw_text = root_1
        .and_then(|r| r.get("6"))
        .and_then(|c| c.get("3"))
        .and_then(|c| c.get("2"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // 会话与买家:reminderUrl 的 peerUserId/itemId;消息本身也携带 cid
    if let Some(cid) = root_1.and_then(|r| r.get("2")).and_then(|v| v.as_str()) {
        out.chat_id = Some(strip_goofish(cid));
    }
    let reminder_url = header_10
        .and_then(|h| h.get("reminderUrl"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !reminder_url.is_empty() {
        let parsed_url = url::Url::parse(reminder_url)
            .ok()
            .or_else(|| url::Url::parse(&format!("https://x.g{reminder_url}")).ok());
        if let Some(url) = parsed_url {
            out.buyer_id = query_param(&url, "peerUserId");
            out.item_id = query_param(&url, "itemId");
        }
    }

    // 系统事件门禁:简化消息 | 非聊天方向 | 卡片消息 | 系统业务标记
    let system_biz = ext_json
        .get("bizTag")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.contains("sys") || t.contains("SYSTEM"));
    let is_system =
        !simplified_red.is_empty() || direction == "1" || content_type == "6" || system_biz;
    if !is_system {
        // 007 D5/T045:买家方向(direction="2"语义)普通聊天 → ChatMessage 进聊天域;
        // 方向缺失或非买家方向:证据不足,仍不产生事件
        if direction == "2" {
            let (kind, text, image_url) = classify_chat_body(&content_json, &raw_text);
            out.kind = MessageKind::ChatMessage {
                chat_id: out.chat_id.clone(),
                buyer_id: out.buyer_id.clone(),
                message_id: message_id_of(msg),
                kind,
                text,
                image_url,
                item_id: out.item_id.clone(),
            };
        }
        return out;
    }

    // 订单号:updateKey 次段优先,再从文案/链接提取
    let order_id = ext_json
        .get("updateKey")
        .and_then(|v| v.as_str())
        .and_then(parse_update_key)
        .or_else(|| extract_order_id(reminder_content))
        .or_else(|| {
            content_json
                .get("targetUrl")
                .and_then(|v| v.as_str())
                .and_then(extract_order_id)
        });

    // 付款:简化消息 redReminder 或完整消息 reminderContent 含标记
    let paid = simplified_red == "等待卖家发货"
        || PAID_MARKERS.iter().any(|m| reminder_content.contains(m));
    // 砍价待刀成:不支持自动(只能免拼开关;v1 不实现),显式不产生付款事件
    let bargain_pending = content_json
        .get("dxCard")
        .and_then(|d| d.get("item"))
        .and_then(|i| i.get("main"))
        .and_then(|m| m.get("exContent"))
        .and_then(|e| e.get("title"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t.contains("小刀"));

    if paid && !bargain_pending {
        if let Some(order) = order_id {
            out.kind = MessageKind::PaidOrder {
                order_id: order,
                buyer_hint: out.buyer_id.clone(),
            };
        }
        return out;
    }

    // 完成:contentType=25 且完成文案
    if content_type == "25"
        && COMPLETE_MARKERS
            .iter()
            .any(|m| reminder_content.contains(m))
        && let Some(order) = order_id
    {
        out.kind = MessageKind::OrderCompleted { order_id: order };
    }
    out
}

/// 规范事件包装:失败/无业务含义的消息不产生事件。
pub fn extract_events(msg: &Value, account_id: &str, generation: i64) -> Vec<PlatformEvent> {
    let extracted = extract(msg);
    // 003 D2:风控文本信号(punish 系关键词)→ 验证要求事件;普通消息不触发(误报为零)
    let meta = EventMeta {
        account_id: account_id.to_string(),
        credential_generation: generation,
        source_event_id: None,
        platform_event_at_ms: None,
        received_at_ms: utc_now_ms(),
    };
    match extracted.kind {
        MessageKind::PaidOrder {
            order_id,
            buyer_hint,
        } => vec![PlatformEvent::OrderPaymentSignal {
            meta,
            order_id,
            buyer_hint,
        }],
        MessageKind::OrderCompleted { order_id } => vec![PlatformEvent::OrderStateSignal {
            meta,
            order_id,
            platform_state: "completed".into(),
        }],
        MessageKind::ChatMessage {
            chat_id,
            buyer_id,
            message_id,
            kind,
            text,
            image_url,
            item_id,
        } => {
            // source_event_id 稳定:管道层(platform_message_id 之外)第二道去重
            let mut chat_meta = meta.clone();
            chat_meta.source_event_id = Some(format!("chat:{message_id}"));
            vec![PlatformEvent::ChatMessageReceived {
                meta: chat_meta,
                chat_id,
                buyer_id,
                message_id,
                msg_kind: kind,
                text,
                image_url,
                item_id,
            }]
        }
        MessageKind::Other => Vec::new(),
    }
}

/// 买家消息内容分类(contentType:2=图片、7=商品卡片、4=视频、3=语音;其余按文本)。
/// 参考 extractMessageContent 行为子集;未知类型按 unknown 展示,原文保留在 text。
fn classify_chat_body(content_json: &Value, text: &str) -> (ChatMsgKind, Option<String>, Option<String>) {
    let ct = content_json
        .get("contentType")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    match ct {
        "2" => {
            let url = content_json
                .get("image")
                .and_then(|i| i.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (ChatMsgKind::Image, None, url)
        }
        "7" => (ChatMsgKind::ItemCard, None, None),
        "4" | "3" => (
            ChatMsgKind::Unknown,
            (!text.is_empty()).then(|| text.to_string()),
            None,
        ),
        _ => (
            ChatMsgKind::Text,
            (!text.is_empty()).then(|| text.to_string()),
            None,
        ),
    }
}

/// 平台消息 ID:有界递归查找 messageId/message_id/messageKey 键(深度≤6);
/// 缺失时以整包内容摘要兜底(同载荷重投递稳定去重,参考实现的 digest 回退语义)。
fn message_id_of(msg: &Value) -> String {
    if let Some(found) = find_message_id(msg, 0) {
        return found;
    }
    let repr = serde_json::to_string(msg).unwrap_or_default();
    let digest = sha2::Sha256::digest(repr.as_bytes());
    format!("chat-{}", &hex::encode(digest)[..16])
}

fn find_message_id(value: &Value, depth: usize) -> Option<String> {
    if depth > 6 {
        return None;
    }
    match value {
        Value::Object(map) => {
            for key in ["messageId", "message_id", "messageKey"] {
                if let Some(v) = map.get(key)
                    && let Some(s) = v.as_str().map(|s| s.trim())
                    && !s.is_empty()
                {
                    return Some(s.to_string());
                }
            }
            // 大数组兜底:消息体常见嵌套信封,优先浅层
            for child in map.values() {
                if let Some(found) = find_message_id(child, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|c| find_message_id(c, depth + 1)),
        _ => None,
    }
}

/// updateKey 形如 "chatID:orderID:seq":取第 2 段且须为长数字。
pub fn parse_update_key(key: &str) -> Option<String> {
    let mut parts = key.split(':');
    let _chat = parts.next()?;
    let order = parts.next()?.trim().to_string();
    if order.len() >= 10 && order.chars().all(|c| c.is_ascii_digit()) {
        Some(order)
    } else {
        None
    }
}

/// 从文案/URL 提取订单号:orderId=/bizOrderId=/order_detail?id= 后的 10 位以上数字。
pub fn extract_order_id(text: &str) -> Option<String> {
    for prefix in ["orderId=", "bizOrderId=", "order_detail?id=", "id="] {
        if let Some(pos) = text.find(prefix) {
            let rest = &text[pos + prefix.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 10 {
                return Some(digits);
            }
        }
    }
    None
}

fn query_param(url: &url::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

fn strip_goofish(cid: &str) -> String {
    cid.strip_suffix("@goofish").unwrap_or(cid).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paid_msg(order: &str) -> Value {
        let update_key = format!("cid123:{order}:0");
        json!({
            "1": {
                "2": format!("{order}buyer@goofish"),
                "7": "1",
                "6": {"3": {"4": "6"}},
                "10": {
                    "reminderContent": "买家已拍下商品[我已付款，等待你发货]请及时发货",
                    "reminderUrl": format!("https://www.goofish.com/item?itemId=99&peerUserId=b{order}"),
                    "extJson": format!("{{\"updateKey\":\"{update_key}\",\"bizTag\":\"systemMsg\"}}")
                }
            }
        })
    }

    #[test]
    fn paid_system_message_extracts_order_and_buyer() {
        let msg = paid_msg("284736251923");
        let e = extract(&msg);
        assert_eq!(
            e.kind,
            MessageKind::PaidOrder {
                order_id: "284736251923".into(),
                buyer_hint: Some("b284736251923".into())
            }
        );
        assert_eq!(e.chat_id.as_deref(), Some("284736251923buyer"));
        assert_eq!(e.item_id.as_deref(), Some("99"));
    }

    #[test]
    fn normal_chat_never_becomes_payment() {
        // 复制的通知文案进普通聊天(direction=2):产生 ChatMessage,绝不产生付款事件
        let msg = json!({
            "1": {
                "2": "chat1@goofish",
                "7": "2",
                "10": {"reminderContent": "[我已付款，等待你发货]"}
            }
        });
        match extract(&msg).kind {
            MessageKind::ChatMessage { kind, text, .. } => {
                assert_eq!(kind, ChatMsgKind::Text);
                assert_eq!(text, None, "该消息没有 6.3.2 正文");
            }
            other => panic!("普通聊天应为 ChatMessage,实际 {other:?}"),
        }
        // 复制的付款文案绝不是付款候选
        let events = extract_events(&msg, "acct-1", 1);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            PlatformEvent::ChatMessageReceived { .. }
        ));
    }

    /// T045:买家文本消息 → ChatMessage{text},buyer/chat/item 从 reminderUrl 取。
    #[test]
    fn buyer_text_message_becomes_chat_event() {
        let msg = json!({
            "1": {
                "2": "chat9@goofish",
                "7": "2",
                "6": {"3": {"2": "请问什么时候发货", "4": "1"}},
                "10": {
                    "reminderUrl": "https://www.goofish.com/item?itemId=77&peerUserId=buyer-9",
                    "messageId": "pm-1001"
                }
            }
        });
        let e = extract(&msg);
        match e.kind {
            MessageKind::ChatMessage {
                chat_id,
                buyer_id,
                message_id,
                kind,
                text,
                image_url,
                item_id,
            } => {
                assert_eq!(chat_id.as_deref(), Some("chat9"));
                assert_eq!(buyer_id.as_deref(), Some("buyer-9"));
                assert_eq!(message_id, "pm-1001");
                assert_eq!(kind, ChatMsgKind::Text);
                assert_eq!(text.as_deref(), Some("请问什么时候发货"));
                assert_eq!(image_url, None);
                assert_eq!(item_id.as_deref(), Some("77"));
            }
            other => panic!("应为 ChatMessage,实际 {other:?}"),
        }
        let events = extract_events(&msg, "acct-1", 1);
        match &events[0] {
            PlatformEvent::ChatMessageReceived {
                meta, message_id, ..
            } => {
                assert_eq!(message_id, "pm-1001");
                assert_eq!(
                    meta.source_event_id.as_deref(),
                    Some("chat:pm-1001"),
                    "管道层第二道去重键稳定"
                );
            }
            other => panic!("应为 ChatMessageReceived,实际 {other:?}"),
        }
    }

    /// T045:contentType=2 图片消息 → Image 携带 image_url;缺消息 ID 时内容摘要兜底。
    #[test]
    fn buyer_image_message_classified_with_stable_fallback_id() {
        let msg = json!({
            "1": {
                "2": "chat8@goofish",
                "7": "2",
                "6": {"3": {"4": "2", "5": {"contentType": "2", "image": {"url": "https://img.example.com/a.jpg"}}}}
            }
        });
        match extract(&msg).kind {
            MessageKind::ChatMessage {
                kind, image_url, ..
            } => {
                assert_eq!(kind, ChatMsgKind::Image);
                assert_eq!(image_url.as_deref(), Some("https://img.example.com/a.jpg"));
            }
            other => panic!("应为 ChatMessage,实际 {other:?}"),
        }
        // 同载荷重投:兜底消息 ID 稳定(入站去重口径一致)
        let id1 = match extract(&msg).kind {
            MessageKind::ChatMessage { message_id, .. } => message_id,
            _ => unreachable!(),
        };
        let id2 = match extract(&msg).kind {
            MessageKind::ChatMessage { message_id, .. } => message_id,
            _ => unreachable!(),
        };
        assert_eq!(id1, id2);
        assert!(id1.starts_with("chat-"));
    }

    /// T045:方向缺失(非买家证据不足)仍不产生事件;系统消息(卡片/系统 biz)不走聊天路径。
    #[test]
    fn non_buyer_or_system_messages_stay_on_existing_paths() {
        let no_direction = json!({"1": {"2": "chat1@goofish", "6": {"3": {"2": "无方向"}}}});
        assert_eq!(extract(&no_direction).kind, MessageKind::Other);
        // 系统卡片(contentType=6):既有路径,非 ChatMessage
        let card = json!({"1": {"2": "chat1@goofish", "7": "1", "6": {"3": {"4": "6"}}}});
        assert!(matches!(extract(&card).kind, MessageKind::Other | MessageKind::PaidOrder{..}));
        assert!(!matches!(extract(&card).kind, MessageKind::ChatMessage{..}));
    }

    #[test]
    fn simplified_red_message_paid() {
        let msg = json!({
            "3": {"redReminder": "等待卖家发货"},
            "1": {"10": {"extJson": "{\"updateKey\":\"c:1098765432101:1\"}"}}
        });
        assert_eq!(
            extract(&msg).kind,
            MessageKind::PaidOrder {
                order_id: "1098765432101".into(),
                buyer_hint: None
            }
        );
    }

    #[test]
    fn bargain_pending_not_paid() {
        let msg = json!({
            "1": {
                "7": "1",
                "6": {"3": {"4": "6", "5": {"dxCard": {"item": {"main": {"exContent": {"title": "我已成功小刀,待发货"}}}}}}},
                "10": {"reminderContent": "[我已付款,等待你发货]"}
            }
        });
        assert_eq!(extract(&msg).kind, MessageKind::Other);
    }

    #[test]
    fn completed_message_via_content_type_25() {
        let msg = json!({
            "1": {
                "7": "1",
                "6": {"3": {"4": "25"}},
                "10": {
                    "reminderContent": "交易完成,快给ta一个评价吧",
                    "extJson": "{\"updateKey\":\"c:2098765432101:1\"}"
                }
            }
        });
        assert_eq!(
            extract(&msg).kind,
            MessageKind::OrderCompleted {
                order_id: "2098765432101".into()
            }
        );
    }

    #[test]
    fn order_id_from_text_fallback() {
        assert_eq!(
            extract_order_id("https://www.goofish.com/order?orderId=3198765432101"),
            Some("3198765432101".into())
        );
        assert_eq!(extract_order_id("orderId=123"), None, "短数字不算订单号");
    }
}
