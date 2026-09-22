//! 三轴状态与合法迁移表。迁移不通过时返回 Err,
//! 由应用层转为待处理事项;状态机本身不执行副作用。
//! 自动确认开关只决定"是否发起",不改变迁移表本身。

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentState {
    PendingVerification,
    Queued,
    Dispatching,
    NotSent,
    Accepted,
    Unknown,
    Terminated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    None,
    Required,
    Resolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationState {
    Disabled,
    Pending,
    Dispatching,
    Accepted,
    Rejected,
    Unknown,
    Terminated,
}

#[derive(Debug, thiserror::Error)]
#[error("非法状态迁移:{axis} {from:?} → {to:?}")]
pub struct IllegalTransition {
    pub axis: &'static str,
    pub from: String,
    pub to: String,
}

/// content_state 合法迁移(data-model 交付状态机表):
/// - 可信付款/补偿 → 创建 pending_verification
/// - 匹配完成冻结快照 → queued
/// - 取得执行权持久化 attempt → dispatching
/// - 传输证明未提交/明确拒绝未接收 → not_sent(仅暂时故障按预算重试)
/// - 严格关联平台接收证明 → accepted(终态,不可回退)
/// - 超时/断连/重启发现未定 → unknown(禁止自动重试)
/// - 取消/退款/资格终止 → terminated;已提交尝试保留事实不抹除
pub fn content_transition(
    from: ContentState,
    to: ContentState,
) -> Result<ContentState, IllegalTransition> {
    use ContentState::*;
    let ok = matches!(
        (from, to),
        (PendingVerification, Queued)
            | (PendingVerification, NotSent) // 资格缺失不发,记录确定未发送
            | (PendingVerification, Terminated)
            | (Queued, Dispatching)
            | (Queued, Terminated)
            | (Dispatching, NotSent)
            | (Dispatching, Accepted)
            | (Dispatching, Unknown)
            | (NotSent, Dispatching) // 预算内自动重试或显式补发
            | (Unknown, Accepted)    // 仅人工确认已收到追加 manual 证明
            | (Unknown, Terminated)
            | (NotSent, Terminated)
    );
    if ok {
        Ok(to)
    } else {
        Err(IllegalTransition {
            axis: "content_state",
            from: format!("{from:?}"),
            to: format!("{to:?}"),
        })
    }
}

/// confirmation_state 合法迁移:与正文轴独立;失败/未知只影响确认步骤。
pub fn confirmation_transition(
    from: ConfirmationState,
    to: ConfirmationState,
) -> Result<ConfirmationState, IllegalTransition> {
    use ConfirmationState::*;
    let ok = matches!(
        (from, to),
        (Disabled, Pending) | (Disabled, Terminated)
            | (Pending, Dispatching)
            | (Pending, Terminated)
            | (Dispatching, Accepted)
            | (Dispatching, Rejected)
            | (Dispatching, Unknown)
            | (Rejected, Dispatching) // 仅显式重试确认步骤,绝不触发 send_text
            | (Unknown, Dispatching)
            | (Unknown, Accepted) // 查证已发货补证
            | (Unknown, Terminated)
            | (Rejected, Terminated)
            | (Accepted, Terminated)
    );
    if ok {
        Ok(to)
    } else {
        Err(IllegalTransition {
            axis: "confirmation_state",
            from: format!("{from:?}"),
            to: format!("{to:?}"),
        })
    }
}

/// 复核轴:资格缺失/未知/永久拒绝 → required;人工处理或确认后 → resolved。
pub fn review_from(review: ReviewState, required: bool) -> ReviewState {
    match (review, required) {
        (_, true) => ReviewState::Required,
        (ReviewState::Required, false) => ReviewState::Resolved,
        (r, false) => r,
    }
}

impl ContentState {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentState::PendingVerification => "pending_verification",
            ContentState::Queued => "queued",
            ContentState::Dispatching => "dispatching",
            ContentState::NotSent => "not_sent",
            ContentState::Accepted => "accepted",
            ContentState::Unknown => "unknown",
            ContentState::Terminated => "terminated",
        }
    }
}

impl ReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewState::None => "none",
            ReviewState::Required => "required",
            ReviewState::Resolved => "resolved",
        }
    }
}

impl ConfirmationState {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfirmationState::Disabled => "disabled",
            ConfirmationState::Pending => "pending",
            ConfirmationState::Dispatching => "dispatching",
            ConfirmationState::Accepted => "accepted",
            ConfirmationState::Rejected => "rejected",
            ConfirmationState::Unknown => "unknown",
            ConfirmationState::Terminated => "terminated",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_never_regresses() {
        for to in [
            ContentState::Dispatching,
            ContentState::Unknown,
            ContentState::NotSent,
            ContentState::Queued,
        ] {
            assert!(
                content_transition(ContentState::Accepted, to).is_err(),
                "accepted→{to:?} 必须拒绝"
            );
        }
    }

    #[test]
    fn normal_path_is_allowed() {
        let steps = [
            (ContentState::PendingVerification, ContentState::Queued),
            (ContentState::Queued, ContentState::Dispatching),
            (ContentState::Dispatching, ContentState::Accepted),
        ];
        for (from, to) in steps {
            assert!(
                content_transition(from, to).is_ok(),
                "{from:?}→{to:?} 应允许"
            );
        }
    }

    #[test]
    fn unknown_never_auto_retries() {
        assert!(content_transition(ContentState::Unknown, ContentState::Queued).is_err());
        assert!(content_transition(ContentState::Unknown, ContentState::Dispatching).is_err());
        // 人工确认是唯一出路
        assert!(content_transition(ContentState::Unknown, ContentState::Accepted).is_ok());
    }

    #[test]
    fn confirmation_axis_is_independent() {
        assert!(
            confirmation_transition(ConfirmationState::Pending, ConfirmationState::Dispatching)
                .is_ok()
        );
        assert!(
            confirmation_transition(ConfirmationState::Accepted, ConfirmationState::Dispatching)
                .is_err()
        );
        assert!(
            confirmation_transition(ConfirmationState::Disabled, ConfirmationState::Dispatching)
                .is_err()
        );
    }
}
