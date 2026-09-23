//! 运行时:账号值守、退避策略与 serve 编排(T093)。

pub mod backoff;
pub mod supervisor;

pub use backoff::delay as backoff_delay;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn backoff_bounds_and_growth() {
        assert_eq!(backoff_delay(0), Duration::from_secs(2));
        assert_eq!(backoff_delay(1), Duration::from_secs(4));
        let third = backoff_delay(2);
        assert!(third >= Duration::from_secs(8) && third <= Duration::from_secs(16));
        let capped = backoff_delay(10);
        assert!(capped <= Duration::from_secs(60), "上限 60 秒:{capped:?}");
        // 抖动存在:同 attempt 多次结果不全等
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            seen.insert(backoff_delay(8));
        }
        assert!(seen.len() > 1, "退避必须带抖动");
    }
}
