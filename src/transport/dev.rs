//! 开发场景端点(dev-fixtures feature 专用;live 构建该路由不存在并返回 404)。
//! 鉴权 + CSRF + 幂等键与正式端点一致(quickstart §4)。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::domain::ids;
use crate::transport::routes::job_summary_dto_public;
use crate::transport::state::SharedState;

pub async fn list_scenarios() -> Response {
    Json(json!({ "scenarios": crate::adapters::mock::SCENARIOS })).into_response()
}

pub async fn run_scenario(
    State(state): SharedState,
    headers: HeaderMap,
    Path(scenario): Path<String>,
) -> Response {
    // 复用正式鉴权路径:必须已登录且携带 CSRF
    if let Err(resp) = crate::transport::routes::require_admin_public(&state, &headers, true).await
    {
        return resp;
    }
    if !crate::adapters::mock::is_allowed(&scenario) {
        return crate::transport::routes::not_found_public("未知场景");
    }
    // v1:场景任务立即以 succeeded 记录;负载语义随 US1 假适配器(T025)充实
    let Ok(job) = state
        .inner
        .jobs
        .create("dev_scenario", Some(&scenario))
        .await
    else {
        return crate::transport::routes::not_found_public("任务服务不可用");
    };
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "operation_id": ids::new_id("op"),
            "job": job_summary_dto_public(&job),
        })),
    )
        .into_response()
}
