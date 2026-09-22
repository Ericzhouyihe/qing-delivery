//! 账号运行时:supervisor、重连退避与暂停屏障。

use std::time::Duration;

pub mod backoff;

/// 断线指数退避:初始 2 秒、倍增、上限 60 秒;成功后由调用方清零(attempt=0)。
/// 带确定性抖动(±20%),避免紧密重连;验证/失效状态等待人工,不进入退避。
pub fn backoff_delay(attempt: u32) -> Duration {
    backoff::delay(attempt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_bounds_and_growth() {
        // 小退避(<5s)无抖动空间,精确;之后带 ±20% 抖动,验证范围
        assert_eq!(backoff_delay(0), Duration::from_secs(2));
        assert_eq!(backoff_delay(1), Duration::from_secs(4));
        let third = backoff_delay(2);
        assert!(
            third >= Duration::from_secs(7) && third <= Duration::from_secs(9),
            "8s±1: {third:?}"
        );
        let capped = backoff_delay(10);
        assert!(
            capped >= Duration::from_secs(48) && capped <= Duration::from_secs(72),
            "60s ±20%"
        );
        // 抖动带内多次采样应出现不同值(确定性计数器保证)
        let mut seen = std::collections::HashSet::new();
        for _ in 0..25 {
            seen.insert(backoff_delay(8));
        }
        assert!(seen.len() > 1, "多次采样应得到不同退避值");
    }
}
