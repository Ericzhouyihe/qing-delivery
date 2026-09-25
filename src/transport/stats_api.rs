//! 运营统计端点(005,T007):薄 handler——鉴权+参数解析+调用应用服务。
//! 传输层不做聚合、不含 SQL(FR-011/宪章 II);既有 /api/v1/dashboard 零改动(研究 R1)。

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::HashMap;

use crate::application::stats::StatsService;
use crate::domain::stats::{REVENUE_CURRENCY, StatsRange};
use crate::domain::time_util::format_rfc3339;
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

pub async fn stats_overview(
    State(state): SharedState,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(range) = parse_range(&params) else {
        return err_shared(
            ErrorCode::InvalidRequest,
            "区间参数非法:from/to 必须为 epoch 毫秒、to 大于 from 且跨度不超过 92 天",
        );
    };
    // 服务无状态,经 DbThread 句柄即取即用(与请求生命周期一致)
    let service = StatsService::new(state.inner.db.clone());
    match service.overview(range).await {
        Ok(ov) => Json(json!({
            "range": {
                "from": ov.range.from_ms,
                "to": ov.range.to_ms,
                "granularity": ov.range.granularity.as_str(),
            },
            "revenue": {
                "minor_units": ov.revenue_minor_units,
                "currency": REVENUE_CURRENCY,
                "order_count": ov.revenue_order_count,
                "previous_minor_units": ov.previous_minor_units,
                "change_percent": ov.change_percent,
            },
            "accounts": {
                "online": ov.accounts_online,
                "total": ov.accounts_total,
            },
            "pending_issues": ov.pending_issues,
            // 007 T014:库存卡密余量(可选字段;缺失为 null,前端降级不闪 0)
            "stock": ov.stock.map(|s| json!({"available_total": s.available_total})),
            "trend": ov.trend.iter().map(|p| json!({
                "bucket_start": p.bucket_start_ms,
                "minor_units": p.minor_units,
                "order_count": p.order_count,
            })).collect::<Vec<_>>(),
            // 横幅状态随统计同响应返回,页面单请求装配(研究 R7)
            "stopping": state.stopping(),
            "restore": ov.restore.map(|r| json!({
                "state": "quarantined",
                "quarantine_started_at": r.quarantine_started_ms.map(format_rfc3339),
                "unresolved_count": r.unresolved_count,
                "restore_epoch": r.restore_epoch,
            })),
        }))
        .into_response(),
        Err(_) => err_shared(ErrorCode::PersistenceUnavailable, "统计不可用"),
    }
}

/// 缺失/非整数一律 None → 400;区间合法性交由 StatsRange::new 校验(FR-009)。
fn parse_range(params: &HashMap<String, String>) -> Option<StatsRange> {
    let from: i64 = params.get("from")?.parse().ok()?;
    let to: i64 = params.get("to")?.parse().ok()?;
    StatsRange::new(from, to).ok()
}
