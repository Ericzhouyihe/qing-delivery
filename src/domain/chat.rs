//! 在线聊天域(T043/T048,007 US4):出站消息状态机、未读聚合规则、
//! 消息类型枚举与输入上限。纯业务决策,无副作用(与 cards 同风格);
//! SendOutcome→状态的映射在应用层(application::chat),本模块不依赖端口。
//! 红线:uncertain 仅人工核对,永不自动重发(data-model 状态机汇总)。

/// 单条消息正文的字符上限(unicode 标量,FR-041)
pub const MAX_MESSAGE_CHARS: usize = 2000;
/// 快捷回复条数上限(FR-044;data-model 应用层上限 50 条)
pub const MAX_QUICK_REPLIES: usize = 50;
/// 买家备注字符上限(data-model ≤2000)
pub const MAX_NOTE_CHARS: usize = 2000;
/// 会话列表摘要预览长度(unicode 标量)
pub const PREVIEW_CHARS: usize = 60;

/// 消息方向(data-model CHECK in/out)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatDirection {
    /// 买家→卖家(入站)
    In,
    /// 卖家→买家(出站)
    Out,
}

impl ChatDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatDirection::In => "in",
            ChatDirection::Out => "out",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "in" => Some(ChatDirection::In),
            "out" => Some(ChatDirection::Out),
            _ => None,
        }
    }
}

/// 消息类型(data-model CHECK 集合,FR-040:文本/图片/系统通知/商品卡片)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMsgKind {
    Text,
    Image,
    System,
    ItemCard,
    Unknown,
}

impl ChatMsgKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatMsgKind::Text => "text",
            ChatMsgKind::Image => "image",
            ChatMsgKind::System => "system",
            ChatMsgKind::ItemCard => "item_card",
            ChatMsgKind::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(ChatMsgKind::Text),
            "image" => Some(ChatMsgKind::Image),
            "system" => Some(ChatMsgKind::System),
            "item_card" => Some(ChatMsgKind::ItemCard),
            "unknown" => Some(ChatMsgKind::Unknown),
            _ => None,
        }
    }
}

/// 出站消息状态(仅 out 行,data-model CHECK 集合)。
/// `sending → sent | failed | uncertain`;failed 允许人工重试
/// (新 attempt 由幂等键承载);uncertain 仅人工核对,永不自动重发。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutgoingStatus {
    Sending,
    Sent,
    Failed,
    Uncertain,
    Cancelled,
}

impl OutgoingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OutgoingStatus::Sending => "sending",
            OutgoingStatus::Sent => "sent",
            OutgoingStatus::Failed => "failed",
            OutgoingStatus::Uncertain => "uncertain",
            OutgoingStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sending" => Some(OutgoingStatus::Sending),
            "sent" => Some(OutgoingStatus::Sent),
            "failed" => Some(OutgoingStatus::Failed),
            "uncertain" => Some(OutgoingStatus::Uncertain),
            "cancelled" => Some(OutgoingStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("非法出站状态迁移:{from} → {to}(sending → sent|failed|uncertain;failed 人工重试=新行;uncertain 永不自动重发)")]
pub struct IllegalOutgoingTransition {
    pub from: &'static str,
    pub to: &'static str,
}

/// 发送结果落状态:sending 行按结果迁移到终态
/// (Accepted→sent;NotSubmitted/Rejected→failed;Unknown→uncertain)。
pub fn outcome_transition(from: OutgoingStatus) -> Result<OutgoingStatus, IllegalOutgoingTransition> {
    match from {
        OutgoingStatus::Sending => Ok(OutgoingStatus::Sent),
        other => Err(IllegalOutgoingTransition {
            from: other.as_str(),
            to: "sent",
        }),
    }
}

/// 人工重试守卫:仅 failed 行可重试(新 attempt);其余一律拒绝。
/// uncertain 重试 = unsafe_retry(结果未知禁止重发,宪章红线);
/// sending 在途、sent 已送达、cancelled 已撤销均不可重试。
pub fn retry_guard(status: OutgoingStatus) -> Result<(), IllegalOutgoingTransition> {
    match status {
        OutgoingStatus::Failed => Ok(()),
        other => Err(IllegalOutgoingTransition {
            from: other.as_str(),
            to: "retry",
        }),
    }
}

/// 未读聚合:仅入站的用户消息(text/image/unknown)增加未读;
/// 系统/商品卡片消息不产生红点(参考语义"系统消息永不增加用户红点");
/// 出站消息不改变未读(打开会话才清零,见 mark_read)。
pub fn unread_after(current: i64, direction: ChatDirection, kind: ChatMsgKind) -> i64 {
    match (direction, kind) {
        (ChatDirection::In, ChatMsgKind::Text)
        | (ChatDirection::In, ChatMsgKind::Image)
        | (ChatDirection::In, ChatMsgKind::Unknown) => current.saturating_add(1),
        _ => current,
    }
}

/// 会话唯一键:account_id + peer_buyer_id(去掉 @goofish 后缀归一,
/// 与适配器 buyer 兜底构造一致);不同账号同买家是不同会话。
pub fn conversation_key(account_id: &str, peer_buyer_id: &str) -> String {
    let buyer = peer_buyer_id.trim().trim_end_matches("@goofish");
    format!("{account_id}\u{0}{buyer}")
}

/// 会话摘要预览:文本截断到 PREVIEW_CHARS 标量;图片/卡片给固定占位。
pub fn preview_of(kind: ChatMsgKind, text: Option<&str>) -> String {
    match kind {
        ChatMsgKind::Text => {
            let t = text.unwrap_or_default();
            t.chars().take(PREVIEW_CHARS).collect()
        }
        ChatMsgKind::Image => "[图片]".to_string(),
        ChatMsgKind::ItemCard => "[商品卡片]".to_string(),
        ChatMsgKind::System => "[系统消息]".to_string(),
        ChatMsgKind::Unknown => text
            .map(|t| t.chars().take(PREVIEW_CHARS).collect::<String>())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "[消息]".to_string()),
    }
}

/// 出站文本校验:1—2000 unicode 标量(FR-041;空文本不是一条消息)。
pub fn check_outgoing_text(text: &str) -> Result<(), String> {
    let n = text.chars().count();
    if n == 0 {
        return Err("消息内容不能为空".to_string());
    }
    if n > MAX_MESSAGE_CHARS {
        return Err(format!("消息内容超过 {MAX_MESSAGE_CHARS} 字(当前 {n} 字)"));
    }
    Ok(())
}

/// 快捷回复正文校验:1—2000 标量(data-model ≤2000)。
pub fn check_quick_reply_body(body: &str) -> Result<(), String> {
    let n = body.chars().count();
    if n == 0 {
        return Err("快捷回复内容不能为空".to_string());
    }
    if n > MAX_MESSAGE_CHARS {
        return Err(format!("快捷回复内容超过 {MAX_MESSAGE_CHARS} 字(当前 {n} 字)"));
    }
    Ok(())
}

/// 快捷回复上限守卫:达到 50 条后新增被拒(reply_limit_reached)。
pub fn check_quick_reply_cap(current: usize) -> Result<(), String> {
    if current >= MAX_QUICK_REPLIES {
        Err(format!("快捷回复已达上限 {MAX_QUICK_REPLIES} 条"))
    } else {
        Ok(())
    }
}

/// 买家备注校验:0—2000 标量(空串=清除备注,合法)。
pub fn check_buyer_note(note: &str) -> Result<(), String> {
    let n = note.chars().count();
    if n > MAX_NOTE_CHARS {
        return Err(format!("买家备注超过 {MAX_NOTE_CHARS} 字(当前 {n} 字)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- T043:出站状态机 ----------

    #[test]
    fn 状态机_sending可到三个终态() {
        // 应用层按 SendOutcome 调 outcome_transition 前行必须是 sending
        assert_eq!(
            outcome_transition(OutgoingStatus::Sending),
            Ok(OutgoingStatus::Sent)
        );
    }

    #[test]
    fn 状态机_终态不可再迁移() {
        for from in [
            OutgoingStatus::Sent,
            OutgoingStatus::Failed,
            OutgoingStatus::Uncertain,
            OutgoingStatus::Cancelled,
        ] {
            assert!(
                outcome_transition(from).is_err(),
                "{from:?} 不可再按发送结果迁移"
            );
        }
    }

    #[test]
    fn 状态机_字符串往返_全部枚举() {
        for s in [
            OutgoingStatus::Sending,
            OutgoingStatus::Sent,
            OutgoingStatus::Failed,
            OutgoingStatus::Uncertain,
            OutgoingStatus::Cancelled,
        ] {
            assert_eq!(OutgoingStatus::parse(s.as_str()), Some(s));
        }
        assert!(OutgoingStatus::parse("queued").is_none());
        for k in [
            ChatMsgKind::Text,
            ChatMsgKind::Image,
            ChatMsgKind::System,
            ChatMsgKind::ItemCard,
            ChatMsgKind::Unknown,
        ] {
            assert_eq!(ChatMsgKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(ChatDirection::parse("in"), Some(ChatDirection::In));
        assert_eq!(ChatDirection::parse("out"), Some(ChatDirection::Out));
        assert!(ChatDirection::parse("both").is_none());
    }

    // ---------- T043:重试守卫(uncertain 不可重试) ----------

    #[test]
    fn 重试守卫_仅failed可人工重试() {
        assert_eq!(retry_guard(OutgoingStatus::Failed), Ok(()));
    }

    #[test]
    fn 重试守卫_uncertain与sending与sent均拒绝() {
        // uncertain:结果未知,重发可能双发(unsafe_retry)
        assert!(retry_guard(OutgoingStatus::Uncertain).is_err());
        // sending:上一发送仍在途
        assert!(retry_guard(OutgoingStatus::Sending).is_err());
        // sent:已确认送达
        assert!(retry_guard(OutgoingStatus::Sent).is_err());
        assert!(retry_guard(OutgoingStatus::Cancelled).is_err());
    }

    // ---------- T043:未读聚合 ----------

    #[test]
    fn 未读_入站用户消息递增() {
        assert_eq!(
            unread_after(3, ChatDirection::In, ChatMsgKind::Text),
            4,
            "入站文本 +1"
        );
        assert_eq!(
            unread_after(0, ChatDirection::In, ChatMsgKind::Image),
            1,
            "入站图片 +1"
        );
    }

    #[test]
    fn 未读_系统与卡片消息不加红点() {
        assert_eq!(unread_after(2, ChatDirection::In, ChatMsgKind::System), 2);
        assert_eq!(unread_after(2, ChatDirection::In, ChatMsgKind::ItemCard), 2);
    }

    #[test]
    fn 未读_出站消息不改变未读() {
        for kind in [
            ChatMsgKind::Text,
            ChatMsgKind::Image,
            ChatMsgKind::System,
        ] {
            assert_eq!(unread_after(5, ChatDirection::Out, kind), 5);
        }
    }

    // ---------- T043:2000 字限制 ----------

    #[test]
    fn 文本上限_2000字合法_2001字拒绝_空拒绝() {
        assert!(check_outgoing_text(&"字".repeat(2000)).is_ok());
        assert!(check_outgoing_text(&"字".repeat(2001)).is_err());
        assert!(check_outgoing_text("").is_err());
        // 按标量计数:多字节字符不放大计数
        let multi = "あ".repeat(2000);
        assert!(check_outgoing_text(&multi).is_ok());
    }

    #[test]
    fn 快捷回复与备注上限() {
        assert!(check_quick_reply_body("已发货").is_ok());
        assert!(check_quick_reply_body("").is_err());
        assert!(check_quick_reply_body(&"字".repeat(2001)).is_err());
        assert!(check_quick_reply_cap(49).is_ok());
        assert!(check_quick_reply_cap(50).is_err(), "第 51 条被拒");
        // 备注:空串=清除,合法;超长拒绝
        assert!(check_buyer_note("").is_ok());
        assert!(check_buyer_note(&"字".repeat(2001)).is_err());
    }

    // ---------- T043:会话唯一键 ----------

    #[test]
    fn 会话键_账号加买家唯一_买家id归一() {
        let a = conversation_key("acct-1", "b123");
        let b = conversation_key("acct-1", "b123@goofish");
        assert_eq!(a, b, "@goofish 后缀归一为同一买家");
        assert_ne!(
            conversation_key("acct-2", "b123"),
            a,
            "不同账号同买家是不同会话"
        );
        assert_ne!(conversation_key("acct-1", "b124"), a);
    }

    #[test]
    fn 预览_按类型截断() {
        let long = "x".repeat(100);
        assert_eq!(preview_of(ChatMsgKind::Text, Some(&long)).chars().count(), PREVIEW_CHARS);
        assert_eq!(preview_of(ChatMsgKind::Image, None), "[图片]");
        assert_eq!(preview_of(ChatMsgKind::ItemCard, None), "[商品卡片]");
        assert_eq!(preview_of(ChatMsgKind::System, None), "[系统消息]");
    }
}
