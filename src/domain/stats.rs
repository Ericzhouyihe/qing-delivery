//! 运营统计口径与分桶(005):只读统计的领域规则。
//! 本模块不得依赖 HTTP、SQLite 或浏览器(宪章 II);聚合口径的唯一来源(tasks 实现策略)。
//! 口径(澄清 Q1/spec FR-010):营收=付款事实(`paid_at` 落区间)且币种为 CNY 且金额非空的合计,
//! 退款/关闭不回冲;订单数只按付款事实计数,不筛币种/金额/状态。
//! 区间与分桶按本地时区自然日/自然小时对齐(澄清 Q2,FR-002/FR-007)。

use serde::{Deserialize, Serialize};

use crate::domain::time_util::{local_day_start_ms, local_hour_start_ms, local_step_day_start};

/// 区间跨度上限:92 天(spec FR-009)。
pub const MAX_SPAN_MS: i64 = 92 * 24 * 60 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;
/// 小时/日粒度分界:≤48 小时按小时(spec FR-007)。
const HOURLY_MAX_SPAN_MS: i64 = 48 * HOUR_MS;
/// 营收合计仅认此币种(spec FR-010)。
pub const REVENUE_CURRENCY: &str = "CNY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    /// to <= from(FR-009)
    Inverted,
    /// 跨度超过 92 天(FR-009)
    TooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hourly,
    Daily,
}

impl Granularity {
    pub fn as_str(self) -> &'static str {
        match self {
            Granularity::Hourly => "hourly",
            Granularity::Daily => "daily",
        }
    }
}

/// 统计区间:`[from_ms, to_ms)` 半开;粒度由服务端按跨度推导,客户端不可指定(契约)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsRange {
    pub from_ms: i64,
    pub to_ms: i64,
    pub granularity: Granularity,
}

impl StatsRange {
    pub fn new(from_ms: i64, to_ms: i64) -> Result<Self, RangeError> {
        if to_ms <= from_ms {
            return Err(RangeError::Inverted);
        }
        if to_ms - from_ms > MAX_SPAN_MS {
            return Err(RangeError::TooLong);
        }
        let granularity = if to_ms - from_ms <= HOURLY_MAX_SPAN_MS {
            Granularity::Hourly
        } else {
            Granularity::Daily
        };
        Ok(Self {
            from_ms,
            to_ms,
            granularity,
        })
    }

    /// 前一等长区间(对比徽标基准,FR-003)。
    pub fn previous(&self) -> Self {
        let span = self.to_ms - self.from_ms;
        Self {
            from_ms: self.from_ms - span,
            to_ms: self.from_ms,
            granularity: self.granularity,
        }
    }

    /// 分桶边界(闭开区间对):覆盖 [from, to),空桶保留;起点对齐本地自然小时/自然日,
    /// 首桶起点可早于 from(对齐下探),行归属以起点二分定位。
    pub fn bucket_ranges(&self) -> Vec<(i64, i64)> {
        let mut buckets: Vec<(i64, i64)> = Vec::new();
        match self.granularity {
            Granularity::Hourly => {
                let mut cursor = local_hour_start_ms(self.from_ms);
                while cursor < self.to_ms && buckets.len() < 49 {
                    let next = local_hour_start_ms(cursor + HOUR_MS);
                    buckets.push((cursor, next));
                    cursor = next;
                }
            }
            Granularity::Daily => {
                let mut cursor = local_day_start_ms(self.from_ms, 0);
                while cursor < self.to_ms && buckets.len() < 93 {
                    let next = local_step_day_start(cursor, 1);
                    buckets.push((cursor, next));
                    cursor = next;
                }
            }
        }
        buckets
    }
}

/// 统计原料行:orders 最小投影(研究 R3:SQL 只过滤,口径在此计算)。
#[derive(Debug, Clone)]
pub struct PaidOrderRaw {
    pub paid_at: i64,
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
}

/// 营收合计与付款订单数(FR-010 口径)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RevenueTotals {
    pub minor_units: i64,
    pub order_count: i64,
}

pub fn revenue_totals(rows: &[PaidOrderRaw]) -> RevenueTotals {
    let mut t = RevenueTotals::default();
    for r in rows {
        t.order_count += 1;
        if r.currency.as_deref() == Some(REVENUE_CURRENCY)
            && let Some(v) = r.amount_minor
        {
            t.minor_units += v;
        }
    }
    t
}

/// 趋势点:单个时间桶(data-model §TrendPoint)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendPoint {
    pub bucket_start_ms: i64,
    pub minor_units: i64,
    pub order_count: i64,
}

/// 按区间分桶聚合:空桶补零;Σ桶 = Σ原料(SC-002/契约不变式)。
pub fn trend_points(rows: &[PaidOrderRaw], range: &StatsRange) -> Vec<TrendPoint> {
    let mut points: Vec<TrendPoint> = range
        .bucket_ranges()
        .into_iter()
        .map(|(s, _)| TrendPoint {
            bucket_start_ms: s,
            minor_units: 0,
            order_count: 0,
        })
        .collect();
    let starts: Vec<i64> = points.iter().map(|p| p.bucket_start_ms).collect();
    for r in rows {
        let idx = starts.partition_point(|&s| s <= r.paid_at);
        if idx == 0 {
            continue; // 早于首桶起点:不在统计区间呈现范围
        }
        let p = &mut points[idx - 1];
        p.order_count += 1;
        if r.currency.as_deref() == Some(REVENUE_CURRENCY)
            && let Some(v) = r.amount_minor
        {
            p.minor_units += v;
        }
    }
    points
}

/// 对比徽标百分比:四舍五入取整;前值为 0 → None(前端隐藏徽标,FR-003/SC-005)。
pub fn change_percent(current: i64, previous: i64) -> Option<i32> {
    if previous == 0 {
        return None;
    }
    Some(((current - previous) as f64 * 100.0 / previous as f64).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    fn row(paid_at: i64, amount_minor: Option<i64>, currency: Option<&str>) -> PaidOrderRaw {
        PaidOrderRaw {
            paid_at,
            amount_minor,
            currency: currency.map(str::to_string),
        }
    }

    #[test]
    fn range_rejects_inverted_and_overlong() {
        assert_eq!(StatsRange::new(100, 100), Err(RangeError::Inverted));
        assert_eq!(StatsRange::new(200, 100), Err(RangeError::Inverted));
        assert_eq!(
            StatsRange::new(0, MAX_SPAN_MS + 1),
            Err(RangeError::TooLong),
            "跨度超过 92 天拒绝"
        );
        assert!(StatsRange::new(0, MAX_SPAN_MS).is_ok(), "恰 92 天合法");
    }

    #[test]
    fn granularity_derived_from_span() {
        assert_eq!(
            StatsRange::new(0, 48 * HOUR_MS).unwrap().granularity,
            Granularity::Hourly
        );
        assert_eq!(
            StatsRange::new(0, 48 * HOUR_MS + 1).unwrap().granularity,
            Granularity::Daily
        );
    }

    #[test]
    fn previous_is_equal_length_before() {
        let r = StatsRange::new(1_000, 3_000).unwrap();
        assert_eq!(r.previous(), StatsRange::new(-1_000, 1_000).unwrap());
    }

    #[test]
    fn revenue_counts_paid_facts_only_cny_amounts() {
        let rows = vec![
            row(10, Some(1_000), Some("CNY")), // 计入
            row(20, Some(2_500), Some("CNY")), // 计入
            row(30, Some(9_999), Some("USD")), // 非 CNY:排除合计,计入订单数
            row(40, None, Some("CNY")),        // 金额缺失:排除合计,计入订单数
            row(50, Some(800), Some("cny")),   // 币种大小写敏感:不入合计
        ];
        let t = revenue_totals(&rows);
        assert_eq!(t.minor_units, 3_500);
        assert_eq!(t.order_count, 5, "退款/关闭/币种/金额缺失均不回冲订单数");
    }

    /// SC-005:≥10 组样本,含取整边界(.5 进位、临界 ±1)与零基准隐藏。
    #[test]
    fn change_percent_rounds_and_hides_zero_base() {
        assert_eq!(change_percent(150, 100), Some(50));
        assert_eq!(change_percent(50, 100), Some(-50));
        assert_eq!(change_percent(105, 100), Some(5));
        assert_eq!(change_percent(104, 100), Some(4));
        assert_eq!(change_percent(0, 0), None);
        assert_eq!(change_percent(100, 0), None);
        // 补足至 10 组以上:持平、临界、.5 取整
        assert_eq!(change_percent(100, 100), Some(0));
        assert_eq!(change_percent(99, 100), Some(-1));
        assert_eq!(change_percent(101, 100), Some(1));
        assert_eq!(change_percent(333, 100), Some(233));
        assert_eq!(change_percent(1, 3), Some(-67), "round(-66.67) = -67");
        assert_eq!(change_percent(2, 3), Some(-33), "round(-33.33) = -33");
        assert_eq!(change_percent(5, 8), Some(-38), "round(-37.5) 远离零取整");
        assert_eq!(change_percent(11, 8), Some(38), "round(37.5) 远离零取整");
    }

    #[test]
    fn trend_buckets_cover_span_with_zero_fill_and_match_totals() {
        let now = crate::domain::time_util::utc_now_ms();
        let from = local_day_start_ms(now, 6);
        let range = StatsRange::new(from, from + 7 * DAY_MS).unwrap();
        assert_eq!(range.granularity, Granularity::Daily);
        let rows = vec![
            row(from + DAY_MS + 3_600_000, Some(1_000), Some("CNY")),
            row(from + DAY_MS + 3_600_000, Some(500), Some("USD")),
            row(from + 2 * DAY_MS + 60_000, Some(700), Some("CNY")),
        ];
        let points = trend_points(&rows, &range);
        assert_eq!(points.len(), 7, "回退 6 天+今天共 7 个自然日桶,空桶补零");
        assert_eq!(
            points[0],
            TrendPoint {
                bucket_start_ms: points[0].bucket_start_ms,
                minor_units: 0,
                order_count: 0
            }
        );
        assert_eq!(points[1].minor_units, 1_000);
        assert_eq!(points[1].order_count, 2);
        assert_eq!(points[2].minor_units, 700);
        assert_eq!(points[2].order_count, 1);
        let total = revenue_totals(&rows);
        assert_eq!(
            points.iter().map(|p| p.minor_units).sum::<i64>(),
            total.minor_units
        );
        assert_eq!(
            points.iter().map(|p| p.order_count).sum::<i64>(),
            total.order_count
        );
    }

    #[test]
    fn hourly_buckets_align_and_contain() {
        let now = crate::domain::time_util::utc_now_ms();
        let range = StatsRange::new(now - 5 * HOUR_MS, now).unwrap();
        assert_eq!(range.granularity, Granularity::Hourly);
        let buckets = range.bucket_ranges();
        assert!(
            (5..=6).contains(&buckets.len()),
            "对齐下探至多多一桶:实际 {}",
            buckets.len()
        );
        for (s, e) in &buckets {
            assert_eq!(local_hour_start_ms(*s), *s, "桶起点必须是本地整点");
            assert!(e > s);
        }
        assert!(buckets[0].0 <= range.from_ms);
        assert!(buckets.last().unwrap().1 >= range.to_ms - 1);
    }

    #[test]
    fn daily_buckets_are_local_midnights() {
        let now = crate::domain::time_util::utc_now_ms();
        let from = local_day_start_ms(now, 2);
        let range = StatsRange::new(from, from + 3 * DAY_MS).unwrap();
        let buckets = range.bucket_ranges();
        assert_eq!(buckets.len(), 3);
        for (s, _) in &buckets {
            let day_start = local_day_start_ms(*s, 0);
            assert_eq!(day_start, *s, "日桶起点必须是本地自然日 00:00");
        }
    }
}
