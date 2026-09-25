//! sync 消息 → 规范事件提取:系统事件门禁 + 付款文案 + 业务键。
//! 普通聊天(messageDirection=2)不得转换为付款事件(FR-010);
//! 订单号取 updateKey 次段或文案/链接中的 orderId。

use serde_json::Value;

use crate::application::ports::platform::{EventMeta, PlatformEvent};
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
    /// 普通聊天或其他系统消息:不产生订单事件
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
        return out; // 普通聊天:不产生事件,但保留 chat/buyer 供会话解析
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
        MessageKind::Other => Vec::new(),
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
        // 复制的通知文案进普通聊天(direction=2):不产生事件
        let msg = json!({
            "1": {
                "2": "chat1@goofish",
                "7": "2",
                "10": {"reminderContent": "[我已付款，等待你发货]"}
            }
        });
        assert_eq!(extract(&msg).kind, MessageKind::Other);
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
