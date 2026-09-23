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

/// 本地时区当日 00:00 对应的 UTC 毫秒(002 概览统计口径:本地自然日)。
/// 解析失败(极端 DST 空档)时回退为入参,宁可多算窗口不抛错。
pub fn local_midnight_ms(now_ms: i64) -> i64 {
    use chrono::TimeZone;
    let Some(local_now) = chrono::Local.timestamp_millis_opt(now_ms).single() else {
        return now_ms;
    };
    let Some(naive_midnight) = local_now.date_naive().and_hms_opt(0, 0, 0) else {
        return now_ms;
    };
    chrono::Local
        .from_local_datetime(&naive_midnight)
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(now_ms)
}
