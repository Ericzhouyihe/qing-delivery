//! 卡密组用例(T010):组 CRUD(信封 AAD 用途 card_pool_content/card_entry/card_api_config,
//! 列表/详情绝不回明文,只回 content_set/api_config 摘要与库存计数)、追加去重、
//! 批量导入(calamine xlsx + csv/tsv,逐行报告仅成功行入库)、test-api(不落库)。
//! 网络调用(test-api 经 CardSupplier 端口)绝不进入 DbThread 闭包。

use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::cards::{
    self, AppendOutcome, CardPoolRow, DeletePoolOutcome, EntryAuditRow, NewEntry, NewPool,
    PoolPatch, StockCounts,
};
use crate::adapters::windows::keys::DataKey;
use crate::application::ports::cards::CardSupplier;
use crate::domain::cards::{ApiCardConfig, CardPoolKind, check_api_config, check_delay_seconds};
use crate::domain::crypto::{self, Aad};
use crate::domain::ids;

/// 组名/描述长度上限与卡密单行上限(沿用产品内容限制口径,不另设标准)
const MAX_NAME_CHARS: usize = 100;
const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_ENTRY_SCALARS: usize = 1000;
const MAX_ENTRY_BYTES: usize = 4000;
/// 批量导入文件大小上限(2 MiB;超出 413 payload_too_large)
pub const IMPORT_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CardsError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("卡密组不存在")]
    NotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error("卡密组仍被引用(规则 {rules} 条/变体 {variants} 条);引用规则已标记需重新配置,解除引用后可删除")]
    Referenced { rules: i64, variants: i64 },
    #[error("导入文件超过大小限制(2 MiB)")]
    PayloadTooLarge,
    #[error("导入文件解析失败:{0}")]
    Import(String),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
}

// ---- AAD 用途(data-model 指定,不得改动) ----

fn pool_content_aad(pool_id: &str) -> Aad {
    Aad {
        purpose: "card_pool_content".into(),
        entity_id: pool_id.into(),
        content_version: None,
    }
}

fn api_config_aad(pool_id: &str) -> Aad {
    Aad {
        purpose: "card_api_config".into(),
        entity_id: pool_id.into(),
        content_version: None,
    }
}

fn card_entry_aad(entry_id: &str) -> Aad {
    Aad {
        purpose: "card_entry".into(),
        entity_id: entry_id.into(),
        content_version: None,
    }
}

// ---- DTO(不含明文) ----

/// api 型配置的脱敏摘要:headers/params/body 只回"是否已配置",值永不回显。
#[derive(Clone, Debug, PartialEq)]
pub struct ApiConfigSummary {
    pub url: String,
    pub method: String,
    pub timeout_ms: i64,
    pub content_type: Option<String>,
    pub response_path: String,
    pub retry_enabled: bool,
    pub headers_configured: bool,
    pub params_configured: bool,
    pub body_configured: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardPoolSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub description: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    /// data 组:分状态库存计数;其他类型 None
    pub stock: Option<StockCounts>,
    /// text/image:固定内容是否已配置
    pub content_set: Option<bool>,
    /// api:脱敏配置摘要
    pub api_config: Option<ApiConfigSummary>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntryAuditDto {
    pub id: String,
    pub state: String,
    pub origin: String,
    /// 摘要前缀(12 位十六进制),仅供比对;明文永不回显
    pub content_digest_prefix: String,
    pub reserved_order_id: Option<String>,
    pub reserved_delivery_id: Option<String>,
    pub request_key: Option<String>,
    pub reserved_at: Option<String>,
    pub used_at: Option<String>,
    pub created_at: String,
}

impl EntryAuditDto {
    fn from_row(r: &EntryAuditRow) -> Self {
        EntryAuditDto {
            id: r.id.clone(),
            state: r.state.clone(),
            origin: r.origin.clone(),
            content_digest_prefix: r.content_digest.chars().take(12).collect(),
            reserved_order_id: r.reserved_order_id.clone(),
            reserved_delivery_id: r.reserved_delivery_id.clone(),
            request_key: r.request_key.clone(),
            reserved_at: r.reserved_at.clone(),
            used_at: r.used_at.clone(),
            created_at: r.created_at.clone(),
        }
    }
}

pub struct CardPoolDraft {
    pub name: String,
    pub kind: CardPoolKind,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub description: String,
    /// text/image:固定内容
    pub content: Option<String>,
    /// data:初始卡密行
    pub entries: Vec<String>,
    /// api:整份配置
    pub api_config: Option<ApiCardConfig>,
}

pub struct AppendResult {
    pub appended: usize,
    pub skipped_empty: usize,
    pub skipped_duplicate: usize,
}

#[derive(Debug)]
pub struct ImportRowFailure {
    /// 表格行号(含表头,从 1 计)
    pub row: i64,
    pub error: String,
}

pub struct ImportReport {
    pub total: usize,
    pub succeeded: usize,
    pub failed: Vec<ImportRowFailure>,
}

pub struct TestApiResult {
    pub ok: bool,
    pub sample: Option<String>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct CardService {
    db: DbThread,
    key: DataKey,
}

fn digest_of(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn validate_meta(name: &str, delay_seconds: i64, description: &str) -> Result<(), CardsError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CardsError::InvalidRequest("组名不能为空".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CardsError::InvalidRequest(format!(
            "组名过长(上限 {MAX_NAME_CHARS} 字)"
        )));
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(CardsError::InvalidRequest(format!(
            "描述过长(上限 {MAX_DESCRIPTION_CHARS} 字)"
        )));
    }
    check_delay_seconds(delay_seconds).map_err(CardsError::InvalidRequest)
}

fn validate_entry_line(line: &str) -> Result<(), CardsError> {
    if line.chars().count() > MAX_ENTRY_SCALARS || line.len() > MAX_ENTRY_BYTES {
        return Err(CardsError::InvalidRequest(
            "卡密内容超过限制(1000 字/4000 字节);不截断".into(),
        ));
    }
    Ok(())
}

fn summary_of(
    row: &CardPoolRow,
    stock: Option<StockCounts>,
    api_summary: Option<ApiConfigSummary>,
) -> CardPoolSummary {
    CardPoolSummary {
        id: row.id.clone(),
        name: row.name.clone(),
        kind: row.kind.clone(),
        enabled: row.enabled,
        delay_seconds: row.delay_seconds,
        description: row.description.clone(),
        version: row.version,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        stock,
        content_set: match row.kind.as_str() {
            "text" | "image" => Some(row.content_envelope.is_some()),
            _ => None,
        },
        api_config: api_summary,
    }
}

/// 解密 api 配置并生成脱敏摘要(授权编辑用;headers/params/body 值不回显)。
fn api_summary_of(key: &DataKey, row: &CardPoolRow) -> Option<ApiConfigSummary> {
    let envelope = row.api_config_envelope.as_ref()?;
    let plain = crypto::open(&key.key, &api_config_aad(&row.id), envelope).ok()?;
    let cfg: ApiCardConfig = serde_json::from_slice(&plain).ok()?;
    Some(ApiConfigSummary {
        url: cfg.url.clone(),
        method: cfg.method.clone(),
        timeout_ms: cfg.timeout_ms,
        content_type: cfg.content_type.clone(),
        response_path: cfg.response_path.clone(),
        retry_enabled: cfg.retry_enabled,
        headers_configured: !cfg.headers.is_empty(),
        params_configured: !cfg.params.is_empty(),
        body_configured: cfg
            .body
            .as_deref()
            .map(|b| !b.trim().is_empty())
            .unwrap_or(false),
    })
}

impl CardService {
    pub fn new(db: DbThread, key: DataKey) -> Self {
        Self { db, key }
    }

    /// 创建组(按 kind 带内容/条目/api_config);组名唯一性应用层校验。
    pub async fn create_pool(&self, draft: CardPoolDraft) -> Result<CardPoolSummary, CardsError> {
        validate_meta(&draft.name, draft.delay_seconds, &draft.description)?;
        // 卡密明文只在入参与信封中出现;整理条目(去空行、批内去重)
        let mut entries: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        match draft.kind {
            CardPoolKind::Data => {
                for raw in &draft.entries {
                    let line = raw.trim();
                    if line.is_empty() {
                        continue;
                    }
                    validate_entry_line(line)?;
                    if seen.insert(digest_of(line)) {
                        entries.push(line.to_string());
                    }
                }
            }
            CardPoolKind::Text | CardPoolKind::Image => {
                let content = draft.content.as_deref().map(str::trim).unwrap_or_default();
                if content.is_empty() {
                    return Err(CardsError::InvalidRequest(
                        "text/image 组必须提供固定内容".into(),
                    ));
                }
                validate_entry_line(content)?;
            }
            CardPoolKind::Api => {
                let cfg = draft
                    .api_config
                    .as_ref()
                    .ok_or_else(|| CardsError::InvalidRequest("api 组必须提供取卡配置".into()))?;
                let violations = check_api_config(cfg);
                if !violations.is_empty() {
                    return Err(CardsError::InvalidRequest(violations.join(";")));
                }
            }
        }
        let pool_id = ids::new_id("cpl");
        let content_envelope = match draft.kind {
            CardPoolKind::Text | CardPoolKind::Image => Some(crypto::seal(
                &self.key.key,
                &self.key.key_id,
                &pool_content_aad(&pool_id),
                draft
                    .content
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .as_bytes(),
            )),
            _ => None,
        };
        let api_envelope = match (&draft.kind, &draft.api_config) {
            (CardPoolKind::Api, Some(cfg)) => Some(crypto::seal(
                &self.key.key,
                &self.key.key_id,
                &api_config_aad(&pool_id),
                &serde_json::to_vec(cfg).expect("配置序列化"),
            )),
            _ => None,
        };
        let sealed_entries: Vec<(String, crypto::Envelope, String)> = entries
            .iter()
            .map(|line| {
                let entry_id = ids::new_id("cent");
                let envelope = crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &card_entry_aad(&entry_id),
                    line.as_bytes(),
                );
                (entry_id, envelope, digest_of(line))
            })
            .collect();

        let name = draft.name.trim().to_string();
        let kind = draft.kind.as_str().to_string();
        let description = draft.description.trim().to_string();
        let enabled = draft.enabled;
        let delay_seconds = draft.delay_seconds;
        let is_data = draft.kind == CardPoolKind::Data;
        let pool_id_for_stock = pool_id.clone();
        let row = self
            .db
            .call(
                move |conn| -> rusqlite::Result<Result<CardPoolRow, CardsError>> {
                    if cards::find_pool_by_name(conn, &name)?.is_some() {
                        return Ok(Err(CardsError::InvalidRequest(
                            "同名卡密组已存在".into(),
                        )));
                    }
                    let new_pool = NewPool {
                        id: &pool_id,
                        name: &name,
                        kind: &kind,
                        enabled,
                        delay_seconds,
                        description: &description,
                        content_envelope: content_envelope.as_ref(),
                        api_config_envelope: api_envelope.as_ref(),
                    };
                    let pool = cards::insert_pool(conn, &new_pool)?;
                    if !sealed_entries.is_empty() {
                        let items: Vec<NewEntry<'_>> = sealed_entries
                            .iter()
                            .map(|(id, envelope, digest)| NewEntry {
                                id,
                                envelope,
                                content_digest: digest,
                            })
                            .collect();
                        cards::append_entries(conn, &pool.id, &items)?;
                    }
                    let row = cards::get_pool(conn, &pool.id)?
                        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(row))
                },
            )
            .await??;
        let row = row?;
        let stock = if is_data {
            Some(self.stock_of(&pool_id_for_stock).await?)
        } else {
            None
        };
        Ok(summary_of(&row, stock, None))
    }

    async fn stock_of(&self, pool_id: &str) -> Result<StockCounts, CardsError> {
        let pool_id = pool_id.to_string();
        Ok(self
            .db
            .call(move |conn| cards::state_counts(conn, &pool_id))
            .await??)
    }

    async fn get_row(&self, pool_id: &str) -> Result<Option<CardPoolRow>, CardsError> {
        let id = pool_id.to_string();
        Ok(self
            .db
            .call(move |conn| cards::get_pool(conn, &id))
            .await??)
    }

    async fn summary_with_api(&self, row: &CardPoolRow) -> Result<CardPoolSummary, CardsError> {
        let stock = if row.kind == "data" {
            Some(self.stock_of(&row.id).await?)
        } else {
            None
        };
        Ok(summary_of(row, stock, api_summary_of(&self.key, row)))
    }

    /// 版本化更新;text/image 换内容 = 整体替换信封;data 组追加走 append-data。
    pub async fn update_pool(
        &self,
        pool_id: &str,
        expected_version: i64,
        name: Option<String>,
        enabled: Option<bool>,
        delay_seconds: Option<i64>,
        description: Option<String>,
        content: Option<String>,
        api_config: Option<ApiCardConfig>,
    ) -> Result<CardPoolSummary, CardsError> {
        let existing = self.get_row(pool_id).await?.ok_or(CardsError::NotFound)?;
        let kind = CardPoolKind::parse(&existing.kind)
            .ok_or_else(|| CardsError::InvalidRequest(format!("未知组类型:{}", existing.kind)))?;
        let new_name = name.unwrap_or_else(|| existing.name.clone());
        let new_delay = delay_seconds.unwrap_or(existing.delay_seconds);
        let new_desc = description.unwrap_or_else(|| existing.description.clone());
        validate_meta(&new_name, new_delay, &new_desc)?;
        // 内容替换仅对相应类型生效并校验
        let content_envelope = match (&content, kind) {
            (Some(text), CardPoolKind::Text | CardPoolKind::Image) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Err(CardsError::InvalidRequest("固定内容不能为空".into()));
                }
                validate_entry_line(trimmed)?;
                Some(crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &pool_content_aad(pool_id),
                    trimmed.as_bytes(),
                ))
            }
            _ => None,
        };
        let api_envelope = match (&api_config, kind) {
            (Some(cfg), _) => {
                let violations = check_api_config(cfg);
                if !violations.is_empty() {
                    return Err(CardsError::InvalidRequest(violations.join(";")));
                }
                Some(crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &api_config_aad(pool_id),
                    &serde_json::to_vec(cfg).expect("配置序列化"),
                ))
            }
            _ => None,
        };
        let id = pool_id.to_string();
        let name_probe = new_name.trim().to_string();
        let updated = self
            .db
            .call(
                move |conn| -> rusqlite::Result<Result<Option<CardPoolRow>, CardsError>> {
                    if let Some(other) = cards::find_pool_by_name(conn, &name_probe)? {
                        if other.id != id {
                            return Ok(Err(CardsError::InvalidRequest(
                                "同名卡密组已存在".into(),
                            )));
                        }
                    }
                    let patch = PoolPatch {
                        name: Some(&name_probe),
                        enabled,
                        delay_seconds: Some(new_delay),
                        description: Some(&new_desc),
                        content_envelope: content_envelope.as_ref(),
                        api_config_envelope: api_envelope.as_ref(),
                    };
                    Ok(Ok(cards::update_pool(conn, &id, expected_version, &patch)?))
                },
            )
            .await??;
        let row = updated?.ok_or(CardsError::VersionConflict)?;
        self.summary_with_api(&row).await
    }

    /// 删除:被规则/变体引用时拒绝(引用规则已置 needs_reconfiguration=1)。
    pub async fn delete_pool(&self, pool_id: &str) -> Result<(), CardsError> {
        let id = pool_id.to_string();
        let outcome = self
            .db
            .call(move |conn| cards::delete_pool(conn, &id))
            .await??;
        match outcome {
            DeletePoolOutcome::Deleted => Ok(()),
            DeletePoolOutcome::Referenced { rules, variants } => {
                Err(CardsError::Referenced { rules, variants })
            }
        }
    }

    /// 组列表(kind/search 过滤);data 组附带库存计数;永不回明文。
    pub async fn list_pools(
        &self,
        kind: Option<CardPoolKind>,
        search: Option<String>,
    ) -> Result<Vec<CardPoolSummary>, CardsError> {
        let kind_str = kind.map(|k| k.as_str().to_string());
        let search = search
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let rows = self
            .db
            .call(move |conn| cards::list_pools(conn, kind_str.as_deref(), search.as_deref(), 200))
            .await??;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let stock = if row.kind == "data" {
                Some(self.stock_of(&row.id).await?)
            } else {
                None
            };
            out.push(summary_of(row, stock, api_summary_of(&self.key, row)));
        }
        Ok(out)
    }

    /// 详情:含分状态计数与脱敏 api 配置摘要。
    pub async fn get_pool(&self, pool_id: &str) -> Result<CardPoolSummary, CardsError> {
        let row = self.get_row(pool_id).await?.ok_or(CardsError::NotFound)?;
        self.summary_with_api(&row).await
    }

    /// 批量组追加:空行忽略;池内摘要去重跳过;仅 kind=data。
    pub async fn append_data(
        &self,
        pool_id: &str,
        lines: Vec<String>,
    ) -> Result<AppendResult, CardsError> {
        let row = self.get_row(pool_id).await?.ok_or(CardsError::NotFound)?;
        if row.kind != "data" {
            return Err(CardsError::InvalidRequest(
                "仅批量库存(data)组支持追加卡密".into(),
            ));
        }
        let mut skipped_empty = 0usize;
        let mut sealed: Vec<(String, crypto::Envelope, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for raw in lines {
            let line = raw.trim();
            if line.is_empty() {
                skipped_empty += 1;
                continue;
            }
            validate_entry_line(line)?;
            let digest = digest_of(line);
            if seen.insert(digest.clone()) {
                let entry_id = ids::new_id("cent");
                let envelope = crypto::seal(
                    &self.key.key,
                    &self.key.key_id,
                    &card_entry_aad(&entry_id),
                    line.as_bytes(),
                );
                sealed.push((entry_id, envelope, digest));
            }
        }
        let pool_id = row.id;
        let appended = self
            .db
            .call(move |conn| -> rusqlite::Result<AppendOutcome> {
                let items: Vec<NewEntry<'_>> = sealed
                    .iter()
                    .map(|(id, envelope, digest)| NewEntry {
                        id,
                        envelope,
                        content_digest: digest,
                    })
                    .collect();
                cards::append_entries(conn, &pool_id, &items)
            })
            .await??;
        Ok(AppendResult {
            appended: appended.appended,
            skipped_empty,
            skipped_duplicate: appended.skipped_duplicate,
        })
    }

    /// 条目审计分页:state 过滤;cursor "created_at|id";明文与完整摘要永不返回。
    pub async fn list_entries(
        &self,
        pool_id: &str,
        state: Option<String>,
        cursor: Option<String>,
        limit: i64,
    ) -> Result<(Vec<EntryAuditDto>, Option<String>), CardsError> {
        if self.get_row(pool_id).await?.is_none() {
            return Err(CardsError::NotFound);
        }
        let id = pool_id.to_string();
        let state = state.filter(|s| !s.is_empty());
        // cursor 解析失败按非法请求拒绝(不静默回第一页)
        let cursor_pair = cursor
            .map(|c| {
                c.split_once('|')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .ok_or_else(|| CardsError::InvalidRequest("cursor 非法".into()))
            })
            .transpose()?;
        let page = self
            .db
            .call(move |conn| {
                cards::list_entries(
                    conn,
                    &id,
                    state.as_deref(),
                    cursor_pair.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                    limit,
                )
            })
            .await??;
        Ok((page.0.iter().map(EntryAuditDto::from_row).collect(), page.1))
    }

    /// test-api:一次性请求(经 CardSupplier 端口),返回样例与延迟;不落库。
    pub async fn test_api(
        &self,
        supplier: &dyn CardSupplier,
        cfg: &ApiCardConfig,
    ) -> Result<TestApiResult, CardsError> {
        let violations = check_api_config(cfg);
        if !violations.is_empty() {
            return Err(CardsError::InvalidRequest(violations.join(";")));
        }
        let request_key = ids::new_request_key();
        let started = std::time::Instant::now();
        let outcome = supplier.fetch(cfg, &request_key).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(sample) => Ok(TestApiResult {
                ok: true,
                sample: Some(sample),
                latency_ms: Some(latency_ms),
                error: None,
            }),
            Err(e) => Ok(TestApiResult {
                ok: false,
                sample: None,
                latency_ms: Some(latency_ms),
                error: Some(e.to_string()),
            }),
        }
    }

    /// 批量导入:xlsx(calamine)/csv/tsv;表头 名称,类型,内容,描述,启用,延迟秒;
    /// 一行 = 一个 data 组 + 一条卡,或 text/image 单行组;仅成功行入库,逐行报告。
    pub async fn batch_import(
        &self,
        bytes: Vec<u8>,
        filename: &str,
    ) -> Result<ImportReport, CardsError> {
        if bytes.len() > IMPORT_MAX_BYTES {
            return Err(CardsError::PayloadTooLarge);
        }
        let parsed = parse_import_rows(bytes, filename)?;
        let mut report = ImportReport {
            total: parsed.rows.len() + parsed.failures.len(),
            succeeded: 0,
            failed: parsed.failures,
        };
        for row in parsed.rows {
            let (content, entries) = match row.kind {
                CardPoolKind::Data => (None, vec![row.content.clone()]),
                _ => (Some(row.content.clone()), Vec::new()),
            };
            let draft = CardPoolDraft {
                name: row.name.clone(),
                kind: row.kind,
                enabled: row.enabled,
                delay_seconds: row.delay_seconds,
                description: row.description.clone(),
                content,
                entries,
                api_config: None,
            };
            match self.create_pool(draft).await {
                Ok(_) => report.succeeded += 1,
                Err(e) => report.failed.push(ImportRowFailure {
                    row: row.row_no,
                    error: e.to_string(),
                }),
            }
        }
        Ok(report)
    }
}

// ---- 批量导入解析(csv/tsv/xlsx 共用行装配) ----

#[derive(Debug)]
struct ImportRow {
    /// 表格行号(含表头,从 1 计)
    row_no: i64,
    name: String,
    kind: CardPoolKind,
    content: String,
    description: String,
    enabled: bool,
    delay_seconds: i64,
}

#[derive(Debug)]
struct ParsedImport {
    rows: Vec<ImportRow>,
    failures: Vec<ImportRowFailure>,
}

/// 必需列(名称/类型/内容);描述/启用/延迟秒可缺省
const REQUIRED_HEADERS: [&str; 3] = ["名称", "类型", "内容"];
const OPTIONAL_HEADERS: [&str; 3] = ["描述", "启用", "延迟秒"];

fn cell_enabled(raw: &str) -> Result<bool, String> {
    match raw.trim() {
        "" => Ok(true),
        "是" | "启用" | "true" | "True" | "TRUE" | "1" | "y" | "Y" | "yes" => Ok(true),
        "否" | "停用" | "false" | "False" | "FALSE" | "0" | "n" | "N" | "no" => Ok(false),
        other => Err(format!("启用列取值无法识别:{other}(期望 是/否)")),
    }
}

fn cell_delay(raw: &str) -> Result<i64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    let v: i64 = trimmed
        .parse()
        .map_err(|_| format!("延迟秒不是整数:{trimmed}"))?;
    check_delay_seconds(v).map_err(|e| e)?;
    Ok(v)
}

/// 表头定位:必需列缺失 → 整文件拒绝;返回 [名称,类型,内容,描述,启用,延迟秒] 列索引。
fn header_map(header: &[String]) -> Result<[Option<usize>; 6], String> {
    let mut slots: [Option<usize>; 6] = [None; 6];
    for (idx, cell) in header.iter().enumerate() {
        let name = cell.trim();
        if let Some(pos) = REQUIRED_HEADERS.iter().position(|h| h == &name) {
            slots[pos] = Some(idx);
        } else if let Some(pos) = OPTIONAL_HEADERS.iter().position(|h| h == &name) {
            slots[3 + pos] = Some(idx);
        }
    }
    for (pos, name) in REQUIRED_HEADERS.iter().enumerate() {
        if slots[pos].is_none() {
            return Err(format!(
                "表头缺少必需列“{name}”(模板:名称,类型,内容,描述,启用,延迟秒)"
            ));
        }
    }
    Ok(slots)
}

/// 二维文本行(含表头)→ 导入行;行级错误逐行记录不中断整体;整行空白不计入。
fn rows_to_import(rows: Vec<Vec<String>>) -> Result<ParsedImport, String> {
    let mut iter = rows.into_iter().enumerate();
    let (_, header) = iter.next().ok_or("文件没有表头行")?;
    let slots = header_map(&header)?;
    let mut parsed = ParsedImport {
        rows: Vec::new(),
        failures: Vec::new(),
    };
    for (idx, row) in iter {
        let row_no = (idx + 1) as i64;
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let cell = |slot: usize| -> String {
            slots[slot]
                .and_then(|col| row.get(col))
                .map(|c| c.trim().to_string())
                .unwrap_or_default()
        };
        let name = cell(0);
        let kind_raw = cell(1);
        let content = cell(2);
        let description = cell(3);
        let built: Result<ImportRow, String> = (|| {
            if name.is_empty() {
                return Err("名称为空".into());
            }
            let kind = CardPoolKind::parse_label(&kind_raw)
                .ok_or_else(|| format!("类型无法识别:{kind_raw}"))?;
            if kind == CardPoolKind::Api {
                return Err("API 组不支持批量导入,请在页面单独配置".into());
            }
            if content.is_empty() {
                return Err("内容为空".into());
            }
            let enabled = cell_enabled(&cell(4))?;
            let delay_seconds = cell_delay(&cell(5))?;
            Ok(ImportRow {
                row_no,
                name,
                kind,
                content,
                description,
                enabled,
                delay_seconds,
            })
        })();
        match built {
            Ok(r) => parsed.rows.push(r),
            Err(e) => parsed.failures.push(ImportRowFailure {
                row: row_no,
                error: e,
            }),
        }
    }
    Ok(parsed)
}

fn parse_import_rows(bytes: Vec<u8>, filename: &str) -> Result<ParsedImport, CardsError> {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let rows = match ext.as_str() {
        "xlsx" | "xls" => read_xlsx_rows(bytes).map_err(CardsError::Import)?,
        "csv" | "tsv" | "txt" => {
            read_delimited_rows(bytes, if ext == "tsv" { b'\t' } else { b',' })
                .map_err(CardsError::Import)?
        }
        other => {
            return Err(CardsError::Import(format!(
                "不支持的文件类型 .{other}(支持 xlsx/csv/tsv)"
            )))
        }
    };
    rows_to_import(rows).map_err(CardsError::Import)
}

fn read_xlsx_rows(bytes: Vec<u8>) -> Result<Vec<Vec<String>>, String> {
    use calamine::Reader;
    let cursor = std::io::Cursor::new(bytes);
    let mut workbook: calamine::Sheets<std::io::Cursor<Vec<u8>>> =
        calamine::open_workbook_auto_from_rs(cursor).map_err(|e| format!("xlsx 打开失败:{e}"))?;
    let Some(sheet) = workbook.sheet_names().first().cloned() else {
        return Err("工作簿没有工作表".into());
    };
    let range = workbook
        .worksheet_range(&sheet)
        .map_err(|e| format!("读取工作表失败:{e}"))?;
    Ok(range.rows().map(|row| row.iter().map(cell_text).collect()).collect())
}

fn cell_text(d: &calamine::Data) -> String {
    use calamine::Data;
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => e.to_string(),
    }
}

fn read_delimited_rows(bytes: Vec<u8>, delimiter: u8) -> Result<Vec<Vec<String>>, String> {
    // 去 UTF-8 BOM(Excel 导出常见)
    let bytes = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]) {
        Some(rest) => rest.to_vec(),
        None => bytes,
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(bytes.as_slice());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("行解析失败:{e}"))?;
        rows.push(record.iter().map(|s| s.to_string()).collect());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv_bytes(rows: &[&str]) -> Vec<u8> {
        rows.join("\n").into_bytes()
    }

    /// 行装配:成功行/行级错误/空白行跳过(表头必需列定位)
    #[test]
    fn 导入行装配_成功与行级错误() {
        let rows = vec![
            vec![
                "名称".into(),
                "类型".into(),
                "内容".into(),
                "描述".into(),
                "启用".into(),
                "延迟秒".into(),
            ],
            vec![
                "网课组A".into(),
                "批量".into(),
                "CARD-001".into(),
                "".into(),
                "是".into(),
                "0".into(),
            ],
            vec!["文本组".into(), "text".into(), "固定内容".into(), "".into(), "".into(), "".into()],
            // 错误行:类型无法识别
            vec!["坏行".into(), "voice".into(), "x".into(), "".into(), "".into(), "".into()],
            // 错误行:内容为空
            vec!["空内容".into(), "批量".into(), "  ".into(), "".into(), "".into(), "".into()],
            // 错误行:延迟秒越界
            vec!["越界".into(), "批量".into(), "y".into(), "".into(), "否".into(), "9999".into()],
            // 空白行不计入
            vec!["".into(), "".into(), "".into(), "".into(), "".into(), "".into()],
        ];
        let parsed = rows_to_import(rows).unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.failures.len(), 3);
        assert_eq!(parsed.rows[0].name, "网课组A");
        assert_eq!(parsed.rows[0].kind, CardPoolKind::Data);
        assert!(parsed.rows[0].enabled);
        assert_eq!(parsed.rows[0].delay_seconds, 0);
        assert_eq!(parsed.rows[1].kind, CardPoolKind::Text);
        assert!(parsed.failures.iter().any(|f| f.row == 4 && f.error.contains("类型")));
        assert!(parsed.failures.iter().any(|f| f.row == 5 && f.error.contains("内容为空")));
        assert!(
            parsed
                .failures
                .iter()
                .any(|f| f.row == 6 && f.error.contains("延迟秒"))
        );
    }

    #[test]
    fn 表头缺必需列整文件拒绝() {
        let rows = vec![vec!["名称".into(), "内容".into()], vec!["a".into(), "b".into()]];
        let err = rows_to_import(rows).unwrap_err();
        assert!(err.contains("类型"), "应指出缺失列:{err}");
    }

    #[test]
    fn csv解析_逗号与制表符及bom() {
        let bytes = csv_bytes(&[
            "名称,类型,内容,描述,启用,延迟秒",
            "组1,批量,CARD-1,备注,是,10",
        ]);
        let parsed = parse_import_rows(bytes, "a.csv").unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].delay_seconds, 10);
        assert_eq!(parsed.rows[0].description, "备注");

        let tsv = "名称\t类型\t内容\t描述\t启用\t延迟秒\n组2\t文本\t固定\t\t否\t".as_bytes().to_vec();
        let parsed = parse_import_rows(tsv, "b.tsv").unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert!(!parsed.rows[0].enabled);

        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"\xe5\x90\x8d\xe7\xa7\xb0,\xe7\xb1\xbb\xe5\x9e\x8b,\xe5\x86\x85\xe5\xae\xb9\n,,"); // 名称,类型,内容
        assert!(parse_import_rows(bom, "c.csv").is_ok(), "BOM 应被剥离");
    }

    #[test]
    fn 不支持扩展名拒绝() {
        let out = parse_import_rows(b"x".to_vec(), "a.pdf");
        assert!(matches!(out, Err(CardsError::Import(_))));
    }

    #[test]
    fn 启用与延迟单元格取值() {
        assert!(cell_enabled("").unwrap());
        assert!(cell_enabled(" 是 ").unwrap());
        assert!(!cell_enabled("0").unwrap());
        assert!(cell_enabled("maybe").is_err());
        assert_eq!(cell_delay("").unwrap(), 0);
        assert_eq!(cell_delay(" 30 ").unwrap(), 30);
        assert!(cell_delay("abc").is_err());
        assert!(cell_delay("3601").is_err());
    }
}
