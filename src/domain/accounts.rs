//! 账号与授权状态机(data-model「账号与授权状态」)。
//! runtime_enabled 是期望状态,status 是观测状态,二者不混同;
//! 暂停 pausing→paused 必须等控制屏障完成,授权未完成不能进入在线。

/// 账号连接状态(持久化观测值)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountStatus {
    /// 初始/被禁用(UI 映射 paused)
    Disabled,
    Connecting,
    Online,
    Offline,
    AuthExpired,
    NeedsVerification,
    Paused,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountStatus::Disabled => "disabled",
            AccountStatus::Connecting => "connecting",
            AccountStatus::Online => "online",
            AccountStatus::Offline => "offline",
            AccountStatus::AuthExpired => "auth_expired",
            AccountStatus::NeedsVerification => "needs_verification",
            AccountStatus::Paused => "paused",
        }
    }

    pub fn parse(s: &str) -> AccountStatus {
        match s {
            "connecting" => AccountStatus::Connecting,
            "online" => AccountStatus::Online,
            "offline" => AccountStatus::Offline,
            "needs_verification" => AccountStatus::NeedsVerification,
            "auth_expired" => AccountStatus::AuthExpired,
            "paused" => AccountStatus::Paused,
            _ => AccountStatus::Disabled,
        }
    }

    /// HTTP DTO 映射(http-api §2):disabled→paused、auth_expired→authorization_expired、
    /// needs_verification→verification_required。
    pub fn dto_str(self) -> &'static str {
        match self {
            AccountStatus::Disabled | AccountStatus::Paused => "paused",
            AccountStatus::Connecting => "connecting",
            AccountStatus::Online => "online",
            AccountStatus::Offline => "offline",
            AccountStatus::AuthExpired => "authorization_expired",
            AccountStatus::NeedsVerification => "verification_required",
        }
    }
}

use AccountStatus as S;

/// 状态迁移:授权完成 ≠ 在线;失效/验证不互相覆盖;验证与失效等待人工,不紧密重连。
pub fn account_transition(from: S, to: S) -> bool {
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (S::Disabled, S::Connecting)
            | (S::Connecting, S::Online)
            | (S::Connecting, S::Offline)
            | (S::Connecting, S::AuthExpired)
            | (S::Connecting, S::NeedsVerification)
            | (S::Online, S::Offline)
            | (S::Online, S::AuthExpired)
            | (S::Online, S::NeedsVerification)
            | (S::Online, S::Paused)
            | (S::Paused, S::Connecting)
            | (S::Offline, S::Connecting)
            | (S::Offline, S::AuthExpired)
            | (S::Offline, S::NeedsVerification)
            | (S::AuthExpired, S::Connecting) // 重新授权成功后重连(身份一致)
            | (S::NeedsVerification, S::Connecting)
            | (S::Disabled, S::Paused)
            | (S::Connecting, S::Paused)
            | (S::Offline, S::Paused)
            | (S::AuthExpired, S::Paused)
            | (S::NeedsVerification, S::Paused)
    )
}

/// 授权流程状态(data-model):二维码 confirmed 并非在线,
/// 只有身份核验、必要凭证和连接注册完成才可运行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFlowStatus {
    Pending,
    Scanned,
    Confirmed,
    Completed,
    Canceled,
    Expired,
    Failed,
    NeedsVerification,
}

impl AuthFlowStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthFlowStatus::Pending => "pending",
            AuthFlowStatus::Scanned => "scanned",
            AuthFlowStatus::Confirmed => "confirmed",
            AuthFlowStatus::Completed => "completed",
            AuthFlowStatus::Canceled => "canceled",
            AuthFlowStatus::Expired => "expired",
            AuthFlowStatus::Failed => "failed",
            AuthFlowStatus::NeedsVerification => "needs_verification",
        }
    }

    pub fn parse(s: &str) -> AuthFlowStatus {
        match s {
            "scanned" => AuthFlowStatus::Scanned,
            "confirmed" => AuthFlowStatus::Confirmed,
            "completed" => AuthFlowStatus::Completed,
            "canceled" => AuthFlowStatus::Canceled,
            "expired" => AuthFlowStatus::Expired,
            "failed" => AuthFlowStatus::Failed,
            "needs_verification" => AuthFlowStatus::NeedsVerification,
            _ => AuthFlowStatus::Pending,
        }
    }

    /// 终态:取消、完成、过期、失败不可被晚到结果覆盖(platform-adapter §7)。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            AuthFlowStatus::Canceled
                | AuthFlowStatus::Completed
                | AuthFlowStatus::Expired
                | AuthFlowStatus::Failed
        )
    }

    /// HTTP QrState 映射(http-api §2)。
    pub fn dto_str(self) -> &'static str {
        match self {
            AuthFlowStatus::Pending => "awaiting_scan",
            AuthFlowStatus::Scanned => "awaiting_authorization",
            AuthFlowStatus::Confirmed | AuthFlowStatus::Completed => "authorized",
            AuthFlowStatus::Canceled => "cancelled",
            AuthFlowStatus::Expired => "expired",
            AuthFlowStatus::Failed => "failed",
            AuthFlowStatus::NeedsVerification => "verification_required",
        }
    }
}

/// 授权状态推进:只允许前进或进入人工等待;终态只读。
pub fn auth_flow_transition(from: AuthFlowStatus, to: AuthFlowStatus) -> bool {
    use AuthFlowStatus as F;
    if from == to {
        return true;
    }
    if from.is_terminal() {
        return false;
    }
    matches!(
        (from, to),
        (F::Pending, F::Scanned)
            | (F::Pending, F::Expired)
            | (F::Pending, F::Canceled)
            | (F::Pending, F::Failed)
            | (F::Scanned, F::Confirmed)
            | (F::Scanned, F::NeedsVerification)
            | (F::Scanned, F::Expired)
            | (F::Scanned, F::Canceled)
            | (F::Confirmed, F::Completed)
            | (F::Confirmed, F::NeedsVerification)
            | (F::NeedsVerification, F::Confirmed)
            | (F::NeedsVerification, F::Completed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorized_is_not_online() {
        // 授权完成只是流程终态;账号仍需连接注册才 online —— 由服务层组合保证
        assert!(auth_flow_transition(
            AuthFlowStatus::Confirmed,
            AuthFlowStatus::Completed
        ));
        assert!(
            !account_transition(S::Disabled, S::Online),
            "授权未完成不得直接在线"
        );
    }

    #[test]
    fn terminal_auth_states_are_frozen() {
        for terminal in [AuthFlowStatus::Canceled, AuthFlowStatus::Expired] {
            for next in [
                AuthFlowStatus::Scanned,
                AuthFlowStatus::Confirmed,
                AuthFlowStatus::Completed,
            ] {
                assert!(
                    !auth_flow_transition(terminal, next),
                    "{terminal:?} 后晚到成功不得覆盖"
                );
            }
        }
    }

    #[test]
    fn account_recovery_paths() {
        assert!(
            account_transition(S::AuthExpired, S::Connecting),
            "重新授权后可重连"
        );
        assert!(account_transition(S::Online, S::Paused));
        assert!(
            !account_transition(S::NeedsVerification, S::Online),
            "验证要求必须经人工"
        );
    }
}
