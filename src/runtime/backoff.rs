//! 退避计算:指数 + 确定性抖动,纯函数便于测试。

use std::time::Duration;

const BASE_SECS: u64 = 2;
const MAX_SECS: u64 = 60;

pub fn delay(attempt: u32) -> Duration {
    let exp = BASE_SECS.saturating_mul(1u64.checked_shl(attempt.min(30)).unwrap_or(u64::MAX / 2));
    let secs = exp.min(MAX_SECS);
    // ±20% 抖动:用进程内计数器错开并发账号,避免同步风暴
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let jitter_pct = 20u64;
    let swing = (secs * jitter_pct) / 100;
    if swing == 0 {
        return Duration::from_secs(secs);
    }
    let offset = (n % (swing * 2 + 1)).saturating_sub(swing) as i64;
    let final_secs = (secs as i64 + offset).max(1) as u64;
    Duration::from_secs(final_secs)
}
