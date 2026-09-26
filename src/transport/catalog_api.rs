//! 商品与规则 HTTP API(T056/T059):items 列表、规则 CRUD、预览与匹配预览。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::adapters::sqlite::repos::{items, rules, rules_ext};
use crate::application::catalog::rules::{MatchPreview, RuleDraft, RuleError, VariantDraft};
use crate::domain::rules_ext::{ContentSource, ReviewConfig, TriggerType, VariantSource};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    #[allow(dead_code)]
    pub status: Option<String>,
}

/// items 列表(T056):rule_state 徽标(configured/missing/conflict)由启用规则计数计算。
pub async fn list_items(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let account = account_id.clone();
    let rows = state
        .inner
        .db
        .call(move |conn| items::list_for_account(conn, &account, limit))
        .await;
    match rows {
        Ok(Ok(rows)) => {
            let counts = state
                .inner
                .db
                .call(move |conn| -> rusqlite::Result<Vec<(String, i64)>> {
                    let mut stmt = conn.prepare(
                        "SELECT item_id, COUNT(*) FROM rules
                         WHERE account_id = ?1 AND enabled = 1 GROUP BY item_id",
                    )?;
                    let out = stmt
                        .query_map([&account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(out)
                })
                .await;
            let badge = |item: &items::ItemRow| -> &'static str {
                match &counts {
                    Ok(Ok(list)) => match list.iter().find(|(id, _)| *id == item.id) {
                        Some((_, 1)) => "configured",
                        Some((_, _)) => "conflict",
                        None => "missing",
                    },
                    _ => "missing",
                }
            };
            Json(json!({
                "items": rows.iter().map(|r| json!({
                    "id": r.id,
                    "account_id": r.account_id,
                    "platform_item_id": r.external_item_id,
                    "title": r.title,
                    "status": r.listing_state,
                    "sku_definition": serde_json::from_str::<serde_json::Value>(&r.sku_definition)
                        .unwrap_or(serde_json::json!([])),
                    "rule_state": badge(r),
                    "version": r.version,
                })).collect::<Vec<_>>(),
                "next_cursor": null,
            }))
            .into_response()
        }
        _ => err_shared(ErrorCode::PersistenceUnavailable, "商品查询不可用"),
    }
}

/// 规则写入请求(007 US3 contracts §3 扩展;旧字段 content_kind/content 保留兼容,
/// 新字段全部可缺省——缺省即 001 固定内容语义)。
#[derive(Deserialize)]
pub struct RuleWriteRequest {
    pub item_id: String,
    pub sku_key: String,
    /// 旧别名(fixed_text);content_source 优先
    pub content_kind: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    pub enabled: bool,
    pub expected_version: Option<i64>,
    #[serde(default)]
    pub trigger_type: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub content_source: Option<String>,
    #[serde(default)]
    pub card_pool_id: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub template_bindings: Option<crate::domain::templates::TemplateBindings>,
    #[serde(default)]
    pub variants: Vec<VariantRequest>,
    #[serde(default)]
    pub review_config: Option<ReviewConfig>,
    #[serde(default)]
    pub all_items_confirmed: Option<bool>,
}

#[derive(Deserialize)]
pub struct VariantRequest {
    #[serde(default)]
    pub spec_name: String,
    #[serde(default)]
    pub spec_values: Vec<String>,
    pub source: String,
    #[serde(default)]
    pub card_pool_id: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub template_bindings: Option<crate::domain::templates::TemplateBindings>,
    #[serde(default = "default_units")]
    pub units_per_item: i64,
    #[serde(default)]
    pub delay_override_seconds: Option<i64>,
}

fn default_units() -> i64 {
    1
}

enum DraftFromRequest {
    Ok(RuleDraft),
    Rejected(Response),
}

fn draft_from_request(body: &RuleWriteRequest) -> DraftFromRequest {
    // 触发类型(buyer_reviewed 能力门禁在应用层;此处只解析)
    let trigger = match body.trigger_type.as_deref() {
        None => TriggerType::default(),
        Some(raw) => match TriggerType::parse(raw) {
            Some(t) => t,
            None => {
                return DraftFromRequest::Rejected(err_shared(
                    ErrorCode::InvalidRequest,
                    &format!("未知触发类型:{raw}"),
                ))
            }
        },
    };
    // 内容来源:content_source 优先,旧 content_kind 兼容
    let raw_source = body
        .content_source
        .as_deref()
        .or(body.content_kind.as_deref())
        .unwrap_or("fixed_text");
    let source = match ContentSource::parse(raw_source) {
        Some(s) => s,
        None => {
            return DraftFromRequest::Rejected(err_shared(
                ErrorCode::InvalidRequest,
                &format!("未知内容来源:{raw_source}(fixed_text/card_pool/template)"),
            ))
        }
    };
    let mut variants = Vec::with_capacity(body.variants.len());
    for (i, v) in body.variants.iter().enumerate() {
        let idx = i + 1;
        let variant_source = match VariantSource::parse(&v.source) {
            Some(s) => s,
            None => {
                return DraftFromRequest::Rejected(err_shared(
                    ErrorCode::InvalidRequest,
                    &format!("第 {idx} 个变体来源必须是 card_pool/template"),
                ))
            }
        };
        variants.push(VariantDraft {
            spec_name: v.spec_name.clone(),
            spec_values: v.spec_values.clone(),
            source: variant_source,
            card_pool_id: v.card_pool_id.clone(),
            template_id: v.template_id.clone(),
            template_bindings: v.template_bindings.clone(),
            units_per_item: v.units_per_item,
            delay_override_seconds: v.delay_override_seconds,
        });
    }
    DraftFromRequest::Ok(RuleDraft {
        item_id: body.item_id.clone(),
        sku_key: body.sku_key.clone(),
        content: body.content.clone().unwrap_or_default(),
        enabled: body.enabled,
        trigger_type: trigger,
        priority: body.priority.unwrap_or(crate::domain::rules_ext::DEFAULT_PRIORITY),
        content_source: source,
        card_pool_id: body.card_pool_id.clone(),
        template_id: body.template_id.clone(),
        template_bindings: body.template_bindings.clone(),
        variants,
        review_config: body.review_config.clone(),
        all_items_confirmed: body.all_items_confirmed.unwrap_or(false),
    })
}

fn rule_err(e: RuleError) -> Response {
    match e {
        RuleError::EmptyContent | RuleError::ContentTooLong { .. } => {
            err_shared(ErrorCode::ContentTooLong, &e.to_string())
        }
        RuleError::EnabledConflict => err_shared(ErrorCode::RuleConflict, &e.to_string()),
        RuleError::NotFound | RuleError::ItemNotFound => {
            err_shared(ErrorCode::ResourceNotFound, &e.to_string())
        }
        RuleError::VersionConflict => err_shared(ErrorCode::VersionConflict, &e.to_string()),
        RuleError::ReferencedByDeliveries(_) => {
            err_shared(ErrorCode::ReferencedResource, &e.to_string())
        }
        RuleError::Invalid(_) => err_shared(ErrorCode::InvalidRequest, &e.to_string()),
        RuleError::UnsupportedTrigger => {
            err_shared(ErrorCode::UnsupportedCapability, &e.to_string())
        }
        RuleError::Db(_) | RuleError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "规则服务不可用")
        }
    }
}

/// 创建规则(T057 + 007 US3 T039):POST 不带 expected_version;body 支持全部扩展字段。
pub async fn create_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<RuleWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let draft = match draft_from_request(&body) {
        DraftFromRequest::Ok(d) => d,
        DraftFromRequest::Rejected(resp) => return resp,
    };
    match state.inner.rule_service.create(&account_id, draft).await {
        Ok(saved) => (
            StatusCode::CREATED,
            Json(json!({
                "summary": {
                    "id": saved.id, "account_id": account_id, "item_id": body.item_id,
                    "sku_key": body.sku_key, "enabled": body.enabled,
                    "version": saved.version,
                    "content_length": body.content.as_ref().map(|c| c.chars().count()).unwrap_or(0),
                    "content_version": saved.content_version,
                },
                "content": body.content,
                "content_kind": body
                    .content_source
                    .clone()
                    .or(body.content_kind.clone())
                    .unwrap_or_else(|| "fixed_text".into()),
            })),
        )
            .into_response(),
        Err(e) => rule_err(e),
    }
}

/// 版本化更新(T039):expected_version 必填;全量替换扩展字段(范围不可变)。
pub async fn update_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, rule_id)): Path<(String, String)>,
    Json(body): Json<RuleWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(expected) = body.expected_version else {
        return err_shared(ErrorCode::InvalidRequest, "更新必须携带 expected_version");
    };
    let draft = match draft_from_request(&body) {
        DraftFromRequest::Ok(d) => d,
        DraftFromRequest::Rejected(resp) => return resp,
    };
    match state
        .inner
        .rule_service
        .update_full(&account_id, &rule_id, expected, draft)
        .await
    {
        Ok(saved) => Json(json!({
            "summary": {
                "id": saved.id, "account_id": account_id, "item_id": body.item_id,
                "sku_key": body.sku_key, "enabled": body.enabled,
                "version": saved.version,
                "content_length": body.content.as_ref().map(|c| c.chars().count()).unwrap_or(0),
                "content_version": saved.content_version,
            },
            "content": body.content,
            "content_kind": body
                .content_source
                .clone()
                .or(body.content_kind.clone())
                .unwrap_or_else(|| "fixed_text".into()),
        }))
        .into_response(),
        Err(e) => rule_err(e),
    }
}

/// 删除规则(007 US3:被交付历史引用 409 referenced_resource;乐观锁)。
pub async fn delete_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, rule_id)): Path<(String, String)>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(expected) = params
        .get("expected_version")
        .and_then(|v| v.parse::<i64>().ok())
    else {
        return err_shared(ErrorCode::InvalidRequest, "删除必须携带 expected_version");
    };
    match state
        .inner
        .rule_service
        .delete(&account_id, &rule_id, expected)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => rule_err(e),
    }
}

/// 规则列表查询(FR-038/T039):trigger_type/enabled/search 过滤 + trigger_counts 汇总。
#[derive(Deserialize)]
pub struct RuleListQuery {
    pub limit: Option<i64>,
    pub trigger_type: Option<String>,
    pub enabled: Option<String>,
    pub search: Option<String>,
}

/// RuleDto(contracts §3 扩展):全部新字段 + 变体;正文不回显(授权编辑另行解密)。
fn rule_dto(rule: &rules::RuleRow, variants: &[rules_ext::VariantRow]) -> serde_json::Value {
    let content_source = if rule.template_id.as_deref().map(str::is_empty) == Some(false) {
        "template"
    } else if rule.card_pool_id.as_deref().map(str::is_empty) == Some(false) {
        "card_pool"
    } else {
        "fixed_text"
    };
    json!({
        "id": rule.id,
        "account_id": rule.account_id,
        "item_id": rule.item_id,
        "sku_key": rule.sku_key,
        "enabled": rule.enabled,
        "version": rule.version,
        "content_version": rule.current_content_version,
        "trigger_type": rule.trigger_type,
        "priority": rule.priority,
        "all_items_confirmed": rule.all_items_confirmed,
        "needs_reconfiguration": rule.needs_reconfiguration,
        "content_source": content_source,
        "card_pool_id": rule.card_pool_id,
        "template_id": rule.template_id,
        "template_bindings": rule.template_bindings.as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
        "variants": variants.iter().map(|v| json!({
            "spec_name": v.spec_name,
            "spec_values": v.spec_values,
            "source": v.source.as_str(),
            "card_pool_id": v.card_pool_id,
            "template_id": v.template_id,
            "template_bindings": v.template_bindings.as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
            "units_per_item": v.units_per_item,
            "delay_override_seconds": v.delay_override_seconds,
        })).collect::<Vec<_>>(),
        "review_config": rule.review_config.as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
    })
}

pub async fn list_rules(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(q): Query<RuleListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let enabled = match q.enabled.as_deref() {
        None => None,
        Some("") => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(other) => {
            return err_shared(
                ErrorCode::InvalidRequest,
                &format!("enabled 参数必须是 true/false,收到「{other}」"),
            )
        }
    };
    let trigger = q.trigger_type.clone().filter(|s| !s.is_empty());
    let search = q.search.clone().filter(|s| !s.is_empty());
    let result = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<(Vec<rules::RuleRow>, Vec<Vec<rules_ext::VariantRow>>, Vec<(String, i64)>)> {
            let rows = rules_ext::list_rules_filtered(
                conn, &account_id, trigger.as_deref(), enabled, search.as_deref(), limit,
            )?;
            let mut all_variants = Vec::with_capacity(rows.len());
            for row in &rows {
                all_variants.push(rules_ext::list_variants(conn, &row.id)?);
            }
            let counts = rules_ext::trigger_counts(conn, &account_id)?;
            Ok((rows, all_variants, counts))
        })
        .await;
    match result {
        Ok(Ok((rows, variants, counts))) => {
            let mut trigger_counts = serde_json::Map::new();
            for (tt, n) in &counts {
                trigger_counts.insert(tt.clone(), json!(n));
            }
            let items: Vec<serde_json::Value> = rows
                .iter()
                .zip(variants.iter())
                .map(|(r, v)| rule_dto(r, v))
                .collect();
            Json(json!({
                "items": items,
                "trigger_counts": trigger_counts,
                "next_cursor": null,
            }))
            .into_response()
        }
        _ => err_shared(ErrorCode::PersistenceUnavailable, "规则查询不可用"),
    }
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub item_id: String,
    pub sku_key: String,
    pub content: String,
    /// 007 US2:模板来源预览(可选;给出时按模板消息渲染)
    #[serde(default)]
    pub template_id: Option<String>,
    /// 007 US2:模板绑定(可选;卡密变量掩码计数与 custom 取值来源)
    #[serde(default)]
    pub template_bindings: Option<crate::domain::templates::TemplateBindings>,
}

/// 预览(T059/US2 扩展):纯校验+掩码渲染,不保存、不发送、不预留卡密。
/// fixed_text 原行为不变(未携带模板字段时响应逐字保持);
/// 携带 template_id/template_bindings 时走 render 掩码模式,
/// 返回 rendered_messages[](卡密值显示 [卡密内容 ×N],contracts §3)。
pub async fn preview_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<PreviewRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let wants_template = body
        .template_id
        .as_deref()
        .map(|t| !t.is_empty())
        .unwrap_or(false);
    if !wants_template && body.template_bindings.is_none() {
        // fixed_text:原有行为(逐字不变)
        let check = state.inner.rule_service.preview(&body.content);
        return Json(json!({
            "rendered_text": body.content,
            "unicode_scalars": check.unicode_scalars,
            "utf8_bytes": check.utf8_bytes,
            "valid": check.valid,
            "violations": check.violations.iter().map(|v| json!({
                "field": "content", "code": "content_invalid", "message": v
            })).collect::<Vec<_>>(),
        }))
        .into_response();
    }
    // 模板/绑定来源:掩码渲染返回 rendered_messages[]
    let bindings = body.template_bindings.clone().unwrap_or_default();
    let template_svc = crate::application::templates::TemplateService::new(state.inner.db.clone());
    let (messages, keys) = if wants_template {
        let template_id = body.template_id.clone().unwrap_or_default();
        match template_svc.get(&template_id).await {
            Ok(t) => (t.messages, t.keys),
            Err(crate::application::templates::TemplatesError::NotFound) => {
                return err_shared(ErrorCode::ResourceNotFound, "模板不存在")
            }
            Err(_) => {
                return err_shared(ErrorCode::PersistenceUnavailable, "模板服务不可用")
            }
        }
    } else {
        // 卡密/自定义绑定作用于正文本身:正文按单条消息掩码渲染
        let messages = vec![body.content.clone()];
        let keys = crate::domain::templates::extract_keys(&messages);
        (messages, keys)
    };
    // 系统变量样例值:预览无订单事实;card_name 尽力解析(item 须属于该账号)
    let card_name = {
        let item_id = body.item_id.clone();
        let account = account_id.clone();
        state
            .inner
            .db
            .call(move |conn| -> rusqlite::Result<Option<String>> {
                Ok(items::get(conn, &item_id)?
                    .filter(|i| i.account_id == account)
                    .map(|i| i.title))
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten()
            .unwrap_or_else(|| "商品名称".to_string())
    };
    let vars = crate::application::templates::PreviewVars {
        buyer_nickname: "买家昵称".to_string(),
        order_id: "平台订单号".to_string(),
        buyer_id: "买家ID".to_string(),
        card_name,
    };
    match template_svc.render_masked(&messages, &bindings, vars) {
        Ok(rendered) => Json(json!({
            // 卡密内容以 [卡密内容 ×N] 掩码;其余与实际发送共用同一渲染函数
            "rendered_messages": rendered,
            "message_count": messages.len(),
            "keys": {
                "cards": keys.cards,
                "custom": keys.custom,
            },
            "valid": true,
            "violations": [],
        }))
        .into_response(),
        Err(crate::application::templates::TemplatesError::InvalidRequest(msg)) => {
            // 变量缺失等渲染失败:纯校验语义, violations 内联返回
            Json(json!({
                "rendered_messages": [],
                "message_count": messages.len(),
                "keys": {
                    "cards": keys.cards,
                    "custom": keys.custom,
                },
                "valid": false,
                "violations": [{
                    "field": "template", "code": "render_failed", "message": msg
                }],
            }))
            .into_response()
        }
        Err(_) => err_shared(ErrorCode::PersistenceUnavailable, "模板服务不可用"),
    }
}

#[derive(Deserialize)]
pub struct MatchPreviewRequest {
    pub item_id: String,
    pub sku_key: String,
}

pub async fn match_preview_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<MatchPreviewRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match state
        .inner
        .rule_service
        .match_preview(&account_id, &body.item_id, &body.sku_key)
        .await
    {
        Ok(MatchPreview::Matched {
            rule_id,
            rule_version,
        }) => Json(json!({
            "state": "matched", "rule_id": rule_id, "rule_version": rule_version,
        }))
        .into_response(),
        Ok(MatchPreview::None) => {
            Json(json!({ "state": "none", "reason": "无启用规则匹配完整规格组合" })).into_response()
        }
        Ok(MatchPreview::Ambiguous) => {
            Json(json!({ "state": "ambiguous", "reason": "多条启用规则,不得任选一条" }))
                .into_response()
        }
        Err(e) => rule_err(e),
    }
}

/// 商品同步入口:真实协议适配器接入前(US2 剩余),明确不支持而非伪成功。
pub async fn start_item_sync(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    if state.inner.config.profile == crate::transport::state::ExecutionProfile::Mock {
        // mock profile 的同步经 dev 场景驱动(scenario ordinary_payment 等)
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "mock 同步经 /dev/scenarios 驱动",
        );
    }
    // live:经 ItemSyncDriver 真实拉取在售商品并入库;任务句柄可查询,失败如实标注
    let Some(driver) = state.inner.item_sync.clone() else {
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "当前构建未接入商品同步驱动(live 协议接入前不伪成功)",
        );
    };
    let job = match state
        .inner
        .jobs
        .create("item_sync", Some(&account_id))
        .await
    {
        Ok(j) => j,
        Err(_) => return err_shared(ErrorCode::PersistenceUnavailable, "任务服务不可用"),
    };
    let db = state.inner.db.clone();
    let job_id = job.id.clone();
    let job_version = job.version;
    let account = account_id.clone();
    tokio::spawn(async move {
        use crate::application::ports::platform::RequestContext;
        let mut cursor: Option<String> = None;
        let mut added = 0i64;
        let mut updated = 0i64;
        let mut safe_error: Option<String> = None;
        // 分页上限防御:平台异常返回时不得无限循环
        for _page in 0..50u32 {
            let ctx = RequestContext {
                operation_id: job_id.clone(),
                account_id: account.clone(),
                credential_generation: 0,
                control_generation: 0,
                deadline_ms: crate::domain::time_util::utc_now_ms() + 60_000,
            };
            match driver.list_products_boxed(ctx, cursor.clone()).await {
                Ok(page) => {
                    let next = page.next_cursor.clone();
                    let account_page = account.clone();
                    let outcome: Result<(i64, i64), String> = db
                        .call(move |conn| -> rusqlite::Result<(i64, i64)> {
                            let mut counts = (0i64, 0i64);
                            for item in &page.items {
                                let listing_state =
                                    if item.on_sale { "on_sale" } else { "off_sale" };
                                let sku_json = serde_json::to_string(&item.sku_parts)
                                    .unwrap_or_else(|_| "[]".to_string());
                                let existed = items::find_by_external(
                                    conn,
                                    &account_page,
                                    &item.external_item_id,
                                )?
                                .is_some();
                                items::upsert(
                                    conn,
                                    &crate::domain::ids::new_id("item"),
                                    &account_page,
                                    &item.external_item_id,
                                    &item.title,
                                    listing_state,
                                    &sku_json,
                                    if item.sku_complete {
                                        "complete"
                                    } else {
                                        "incomplete"
                                    },
                                )?;
                                if existed {
                                    counts.1 += 1
                                } else {
                                    counts.0 += 1
                                }
                            }
                            Ok(counts)
                        })
                        .await
                        .map_err(|e| format!("数据库不可用:{e}"))
                        .and_then(|r| r.map_err(|e| format!("商品入库失败:{e}")));
                    match outcome {
                        Ok((a, u)) => {
                            added += a;
                            updated += u;
                        }
                        Err(msg) => {
                            safe_error = Some(msg);
                            break;
                        }
                    }
                    match next {
                        Some(c) => cursor = Some(c),
                        None => break,
                    }
                }
                Err(e) => {
                    safe_error = Some(format!("平台同步失败:{e}"));
                    break;
                }
            }
        }
        let state_str = if safe_error.is_some() {
            "failed"
        } else {
            "succeeded"
        };
        let result_ref = if safe_error.is_none() {
            Some(format!("items:added={added},updated={updated}"))
        } else {
            None
        };
        let _ = db
            .call(move |conn| {
                crate::adapters::sqlite::repos::jobs::finish(
                    conn,
                    &job_id,
                    job_version,
                    state_str,
                    result_ref.as_deref(),
                    safe_error.as_deref(),
                )
            })
            .await;
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "operation_id": crate::domain::ids::new_id("op"),
            "job": {
                "id": job.id, "kind": job.kind, "state": job.state, "version": job.version,
                "created_at": crate::domain::time_util::format_rfc3339(job.created_at),
                "updated_at": crate::domain::time_util::format_rfc3339(job.updated_at),
                "target_id": job.target_id,
            },
        })),
    )
        .into_response()
}
