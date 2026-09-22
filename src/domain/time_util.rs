//! 时间约定:业务时间为 UTC 毫秒,HTTP 用 RFC3339 UTC 字符串(data-model)。
//! 扫描源时钟使用 UTC;重试间隔使用单调时间,由调用方分别取用。

use chrono::{DateTime, SecondsFormat, Utc};

pub fn utc_now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn format_rfc3339(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}
