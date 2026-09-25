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

pub fn local_day_start_ms(now_ms: i64, days_back: i64) -> i64 {
    use chrono::TimeZone;
    let base = local_midnight_ms(now_ms);
    let Some(local_now) = chrono::Local.timestamp_millis_opt(now_ms).single() else {
        return base;
    };
    let Some(date) = local_now
        .date_naive()
        .checked_sub_days(chrono::Days::new(days_back.max(0) as u64))
    else {
        return base;
    };
    local_naive_day_start(date, base)
}

/// 从某自然日 00:00 步进 step 天(可为负)后的 00:00(005 日分桶迭代)。
/// 按日历步进:24h 固定偏移在 DST 日会错位,这里不会。
pub fn local_step_day_start(day_start_ms: i64, step_days: i64) -> i64 {
    use chrono::TimeZone;
    let Some(local) = chrono::Local.timestamp_millis_opt(day_start_ms).single() else {
        return day_start_ms;
    };
    let date = local.date_naive();
    let stepped = if step_days >= 0 {
        date.checked_add_days(chrono::Days::new(step_days as u64))
    } else {
        date.checked_sub_days(chrono::Days::new((-step_days) as u64))
    };
    let Some(date) = stepped else {
        return day_start_ms;
    };
    local_naive_day_start(date, day_start_ms)
}

/// 时刻所属本地自然小时(00 分 00 秒)的 UTC 毫秒(005 小时分桶)。
/// 解析失败回退入参,宁可多算不抛错。
pub fn local_hour_start_ms(ms: i64) -> i64 {
    use chrono::{TimeZone, Timelike};
    let Some(local) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return ms;
    };
    let Some(naive) = local.date_naive().and_hms_opt(local.hour(), 0, 0) else {
        return ms;
    };
    chrono::Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(ms)
}

/// 本地自然日 00:00 转换;00:00 落入 DST 空档时取 01:00(该日最早存在的零点附近时刻)。
fn local_naive_day_start(date: chrono::NaiveDate, fallback: i64) -> i64 {
    use chrono::TimeZone;
    for hour in [0, 1] {
        let Some(naive) = date.and_hms_opt(hour, 0, 0) else {
            continue;
        };
        if let Some(dt) = chrono::Local.from_local_datetime(&naive).single() {
            return dt.timestamp_millis();
        }
    }
    fallback
}

#[cfg(test)]
mod stats_time_tests {
    use super::*;

    /// 回退 N 天的起点必须严格早于回退 N-1 天,且本身是本地自然日零点。
    #[test]
    fn day_start_steps_are_midnights_and_monotonic() {
        let now = utc_now_ms();
        let today = local_day_start_ms(now, 0);
        assert_eq!(today, local_midnight_ms(now));
        for back in 1..=10 {
            let prev = local_day_start_ms(now, back);
            assert!(prev < today, "回退 {back} 天应早于今天零点");
            assert_eq!(local_day_start_ms(prev, 0), prev, "起点自身为零点");
        }
        let stepped = local_step_day_start(today, -3);
        assert_eq!(stepped, local_day_start_ms(now, 3), "负向步进与回退等价");
        assert_eq!(local_step_day_start(stepped, 3), today, "正负步进互逆");
    }

    #[test]
    fn hour_start_aligns_to_local_hour() {
        use chrono::{TimeZone, Timelike};
        let now = utc_now_ms();
        let hs = local_hour_start_ms(now);
        assert!(hs <= now && now - hs < 3_600_000);
        let local = chrono::Local.timestamp_millis_opt(hs).single().unwrap();
        assert_eq!(
            (local.minute(), local.second(), local.nanosecond()),
            (0, 0, 0)
        );
    }
}
