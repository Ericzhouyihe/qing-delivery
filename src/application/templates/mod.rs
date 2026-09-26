//! 发货模板用例(T023):模板 CRUD(消息整体替换、≤10 条校验、乐观锁 version)、
//! used_by_rules 引用计数(rules + rule_variants 两处)、被引用删除拒绝
//! (引用规则标记 needs_reconfiguration=1)、掩码预览渲染
//! (预览与实际发送共用 domain::templates::render,经 bool 掩码参数区分)。
//! 模板消息为明文占位符文本(不含机密);卡密真值只进加密快照,永不落模板表。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::templates::{
    self, DeleteTemplateOutcome, TemplateRow,
};
use crate::domain::ids;
use crate::domain::templates::{
    RenderCard, RenderContext, TemplateBindings, TemplateKeys, extract_keys, render_messages,
    validate_messages,
};

/// 模板名长度上限(与卡密组同口径)
const MAX_NAME_CHARS: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum TemplatesError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("模板不存在")]
    NotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error("模板仍被规则引用({});引用规则已标记需重新配置,解除引用后可删除", .0.join(", "))]
    Referenced(Vec<String>),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct TemplateDraft {
    pub name: String,
    pub enabled: bool,
    pub messages: Vec<String>,
}

/// TemplateDto 投影(contracts §2)。
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub messages: Vec<String>,
    pub keys: TemplateKeys,
    pub used_by_rules: i64,
    pub version: i64,
}

/// 预览渲染的系统变量样例值(实际发送由 freeze 阶段注入订单事实)。
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewVars {
    pub buyer_nickname: String,
    pub order_id: String,
    pub buyer_id: String,
    pub card_name: String,
}

#[derive(Clone)]
pub struct TemplateService {
    db: DbThread,
}

fn validate_meta(name: &str) -> Result<(), TemplatesError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TemplatesError::InvalidRequest("模板名不能为空".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(TemplatesError::InvalidRequest(format!(
            "模板名过长(上限 {MAX_NAME_CHARS} 字)"
        )));
    }
    Ok(())
}

fn summary_of(row: &TemplateRow, messages: Vec<String>, used_by_rules: i64) -> TemplateSummary {
    let keys = extract_keys(&messages);
    TemplateSummary {
        id: row.id.clone(),
        name: row.name.clone(),
        enabled: row.enabled,
        messages,
        keys,
        used_by_rules,
        version: row.version,
    }
}

impl TemplateService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 创建模板(占位符/条数/长度校验失败 → InvalidRequest)。
    pub async fn create(&self, draft: TemplateDraft) -> Result<TemplateSummary, TemplatesError> {
        validate_meta(&draft.name)?;
        validate_messages(&draft.messages).map_err(|e| TemplatesError::InvalidRequest(e.to_string()))?;
        let id = ids::new_id("tpl");
        let name = draft.name.trim().to_string();
        let enabled = draft.enabled;
        let messages = draft.messages.clone();
        let row = self
            .db
            .call(move |conn| templates::insert_template(conn, &id, &name, enabled, &messages))
            .await??;
        Ok(summary_of(&row, draft.messages, 0))
    }

    /// 版本化更新;messages = Some → 消息整体替换。
    pub async fn update(
        &self,
        template_id: &str,
        expected_version: i64,
        name: Option<String>,
        enabled: Option<bool>,
        messages: Option<Vec<String>>,
    ) -> Result<TemplateSummary, TemplatesError> {
        let existing = self
            .get_row(template_id)
            .await?
            .ok_or(TemplatesError::NotFound)?;
        let new_name = name.unwrap_or_else(|| existing.name.clone());
        validate_meta(&new_name)?;
        if let Some(msgs) = &messages {
            validate_messages(msgs)
                .map_err(|e| TemplatesError::InvalidRequest(e.to_string()))?;
        }
        let id = template_id.to_string();
        let name_ref = new_name.trim().to_string();
        let updated = self
            .db
            .call(move |conn| {
                templates::update_template(
                    conn,
                    &id,
                    expected_version,
                    Some(&name_ref),
                    enabled,
                    messages.as_deref(),
                )
            })
            .await??;
        let row = updated.ok_or(TemplatesError::VersionConflict)?;
        self.summary_of_row(&row).await
    }

    /// 删除:被规则/变体引用时拒绝(引用规则已标记需重新配置)。
    pub async fn delete(&self, template_id: &str) -> Result<(), TemplatesError> {
        let id = template_id.to_string();
        let outcome = self
            .db
            .call(move |conn| templates::delete_template(conn, &id))
            .await??;
        match outcome {
            DeleteTemplateOutcome::Deleted => Ok(()),
            DeleteTemplateOutcome::Referenced { rule_ids } => {
                Err(TemplatesError::Referenced(rule_ids))
            }
        }
    }

    pub async fn get(&self, template_id: &str) -> Result<TemplateSummary, TemplatesError> {
        let row = self
            .get_row(template_id)
            .await?
            .ok_or(TemplatesError::NotFound)?;
        self.summary_of_row(&row).await
    }

    /// 模板列表(search 按名包含);每项含 used_by_rules 引用计数。
    pub async fn list(&self, search: Option<String>) -> Result<Vec<TemplateSummary>, TemplatesError> {
        let search = search
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let rows = self
            .db
            .call(move |conn| templates::list_templates(conn, search.as_deref(), 200))
            .await??;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(self.summary_of_row(row).await?);
        }
        Ok(out)
    }

    /// 掩码预览渲染:按模板 id 装载消息后走 render(mask=true)。
    /// 卡密值显示 [卡密内容 ×N](N=绑定份数);custom 取绑定真值;
    /// 变量未赋值 → InvalidRequest(保存侧校验的预览同源判定)。
    pub async fn render_preview(
        &self,
        template_id: &str,
        bindings: &TemplateBindings,
        vars: PreviewVars,
    ) -> Result<Vec<String>, TemplatesError> {
        if self.get_row(template_id).await?.is_none() {
            return Err(TemplatesError::NotFound);
        }
        let id = template_id.to_string();
        let messages = self
            .db
            .call(move |conn| templates::list_messages(conn, &id))
            .await??;
        self.render_masked(&messages, bindings, vars)
    }

    /// 掩码渲染给定消息(fixed_text 正文也可带绑定经掩码预览)。
    pub fn render_masked(
        &self,
        messages: &[String],
        bindings: &TemplateBindings,
        vars: PreviewVars,
    ) -> Result<Vec<String>, TemplatesError> {
        let cards = bindings
            .cards
            .iter()
            .map(|b| {
                (
                    b.key.clone(),
                    RenderCard {
                        text: String::new(), // 掩码模式不取真值
                        count: b.units.max(1) as usize,
                    },
                )
            })
            .collect();
        let ctx = RenderContext {
            buyer_nickname: vars.buyer_nickname,
            order_id: vars.order_id,
            buyer_id: vars.buyer_id,
            card_name: vars.card_name,
            cards,
            custom: bindings.custom.clone(),
        };
        render_messages(messages, &ctx, true)
            .map_err(|e| TemplatesError::InvalidRequest(e.to_string()))
    }

    async fn get_row(&self, template_id: &str) -> Result<Option<TemplateRow>, TemplatesError> {
        let id = template_id.to_string();
        Ok(self
            .db
            .call(move |conn| templates::get_template(conn, &id))
            .await??)
    }

    async fn summary_of_row(&self, row: &TemplateRow) -> Result<TemplateSummary, TemplatesError> {
        let id = row.id.clone();
        let messages = self
            .db
            .call(move |conn| templates::list_messages(conn, &id))
            .await??;
        let used_by = self.used_by_rules(&row.id).await?;
        Ok(summary_of(row, messages, used_by))
    }

    pub async fn used_by_rules(&self, template_id: &str) -> Result<i64, TemplatesError> {
        let id = template_id.to_string();
        let ids = self
            .db
            .call(move |conn| templates::referencing_rules(conn, &id))
            .await??;
        Ok(ids.len() as i64)
    }
}
