//! 关键词回复与账号默认回复用例(T032/T039 配套;分流编排在 replies 域,T036):
//! CRUD 校验(关键词非空、kind=text/image 与内容成对、关联商品归属账号)、
//! 默认回复 upsert 与发送记录清空(FR-033/FR-034)。
//! 关键词/默认回复绝不写订单/交付状态(宪章红线)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{items, rules_ext};
use crate::domain::ids;

const MAX_KEYWORD_CHARS: usize = 100;
const MAX_REPLY_CHARS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum RepliesError {
    #[error("{0}")]
    Invalid(String),
    #[error("关键词规则不存在")]
    NotFound,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone, Debug)]
pub struct ReplyRuleDraft {
    pub keyword: String,
    /// text | image
    pub reply_kind: String,
    pub reply_text: Option<String>,
    pub reply_image_url: Option<String>,
    pub enabled: bool,
    /// 关联商品(空 = 账号级)
    pub item_ids: Vec<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn validate_reply_body(
    draft: &ReplyRuleDraft,
) -> Result<(String, Option<String>, Option<String>), RepliesError> {
    let keyword = draft.keyword.trim();
    if keyword.is_empty() {
        return Err(RepliesError::Invalid("关键词不能为空".into()));
    }
    if keyword.chars().count() > MAX_KEYWORD_CHARS {
        return Err(RepliesError::Invalid(format!(
            "关键词过长(上限 {MAX_KEYWORD_CHARS} 字)"
        )));
    }
    let text = clean(draft.reply_text.clone());
    let image = clean(draft.reply_image_url.clone());
    if let Some(t) = &text
        && t.chars().count() > MAX_REPLY_CHARS
    {
        return Err(RepliesError::Invalid(format!(
            "回复文案过长(上限 {MAX_REPLY_CHARS} 字)"
        )));
    }
    match draft.reply_kind.as_str() {
        "text" => {
            if text.is_none() {
                return Err(RepliesError::Invalid("文字回复必须提供回复文案".into()));
            }
        }
        "image" => {
            if image.is_none() {
                return Err(RepliesError::Invalid("图片回复必须提供图片链接".into()));
            }
        }
        other => {
            return Err(RepliesError::Invalid(format!(
                "回复类型必须是 text/image,收到「{other}」"
            )));
        }
    }
    Ok((keyword.to_string(), text, image))
}

#[derive(Clone)]
pub struct ReplyRulesService {
    db: DbThread,
}

impl ReplyRulesService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    pub async fn list(
        &self,
        account_id: &str,
    ) -> Result<Vec<rules_ext::ReplyRuleRow>, RepliesError> {
        let account = account_id.to_string();
        Ok(self
            .db
            .call(move |conn| rules_ext::list_reply_rules(conn, &account))
            .await??)
    }

    pub async fn create(
        &self,
        account_id: &str,
        draft: ReplyRuleDraft,
    ) -> Result<rules_ext::ReplyRuleRow, RepliesError> {
        let (keyword, text, image) = validate_reply_body(&draft)?;
        let id = ids::new_id("rr");
        let account = account_id.to_string();
        let kind = draft.reply_kind.clone();
        let enabled = draft.enabled;
        let item_ids = draft.item_ids.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<Result<rules_ext::ReplyRuleRow, RepliesError>> {
                for item in &item_ids {
                    match items::get(conn, item)? {
                        Some(it) if it.account_id == account => {}
                        _ => {
                            return Ok(Err(RepliesError::Invalid(format!(
                                "关联商品不存在或不属于该账号:{item}"
                            ))))
                        }
                    }
                }
                Ok(Ok(rules_ext::insert_reply_rule(
                    conn,
                    &id,
                    &account,
                    &keyword,
                    &kind,
                    text.as_deref(),
                    image.as_deref(),
                    enabled,
                    &item_ids,
                )?))
            })
            .await??
    }

    pub async fn update(
        &self,
        account_id: &str,
        reply_rule_id: &str,
        draft: ReplyRuleDraft,
    ) -> Result<rules_ext::ReplyRuleRow, RepliesError> {
        let (keyword, text, image) = validate_reply_body(&draft)?;
        let account = account_id.to_string();
        let rule_id = reply_rule_id.to_string();
        let kind = draft.reply_kind.clone();
        let enabled = draft.enabled;
        let item_ids = draft.item_ids.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<Result<rules_ext::ReplyRuleRow, RepliesError>> {
                let existing = rules_ext::list_reply_rules(conn, &account)?
                    .into_iter()
                    .find(|r| r.id == rule_id);
                if existing.is_none() {
                    return Ok(Err(RepliesError::NotFound));
                }
                for item in &item_ids {
                    match items::get(conn, item)? {
                        Some(it) if it.account_id == account => {}
                        _ => {
                            return Ok(Err(RepliesError::Invalid(format!(
                                "关联商品不存在或不属于该账号:{item}"
                            ))))
                        }
                    }
                }
                match rules_ext::update_reply_rule(
                    conn,
                    &rule_id,
                    &keyword,
                    &kind,
                    text.as_deref(),
                    image.as_deref(),
                    enabled,
                    &item_ids,
                )? {
                    Some(row) => Ok(Ok(row)),
                    None => Ok(Err(RepliesError::NotFound)),
                }
            })
            .await??
    }

    pub async fn delete(&self, account_id: &str, reply_rule_id: &str) -> Result<bool, RepliesError> {
        let account = account_id.to_string();
        let rule_id = reply_rule_id.to_string();
        Ok(self
            .db
            .call(move |conn| -> rusqlite::Result<bool> {
                let owned = rules_ext::list_reply_rules(conn, &account)?
                    .into_iter()
                    .any(|r| r.id == rule_id);
                if !owned {
                    return Ok(false);
                }
                rules_ext::delete_reply_rule(conn, &rule_id)
            })
            .await??)
    }
}

#[derive(Clone, Debug)]
pub struct DefaultReplyDraft {
    pub enabled: bool,
    pub reply_text: Option<String>,
    pub reply_image_url: Option<String>,
    pub reply_once: bool,
}

#[derive(Clone)]
pub struct DefaultReplyService {
    db: DbThread,
}

impl DefaultReplyService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 未配置时返回关闭态投影(前端空态),不落库。
    pub async fn get(
        &self,
        account_id: &str,
    ) -> Result<rules_ext::DefaultReplyRow, RepliesError> {
        let account = account_id.to_string();
        let row = self
            .db
            .call(move |conn| rules_ext::get_default_reply(conn, &account))
            .await??;
        Ok(row.unwrap_or(rules_ext::DefaultReplyRow {
            account_id: account_id.to_string(),
            enabled: false,
            reply_text: None,
            reply_image_url: None,
            reply_once: true,
            updated_at: String::new(),
        }))
    }

    pub async fn put(
        &self,
        account_id: &str,
        draft: DefaultReplyDraft,
    ) -> Result<rules_ext::DefaultReplyRow, RepliesError> {
        let text = clean(draft.reply_text.clone());
        let image = clean(draft.reply_image_url.clone());
        if draft.enabled && text.is_none() && image.is_none() {
            return Err(RepliesError::Invalid(
                "启用默认回复必须提供文字或图片内容".into(),
            ));
        }
        if let Some(t) = &text
            && t.chars().count() > MAX_REPLY_CHARS
        {
            return Err(RepliesError::Invalid(format!(
                "默认回复过长(上限 {MAX_REPLY_CHARS} 字)"
            )));
        }
        let account = account_id.to_string();
        let enabled = draft.enabled;
        let reply_once = draft.reply_once;
        Ok(self
            .db
            .call(move |conn| {
                rules_ext::upsert_default_reply(
                    conn,
                    &account,
                    enabled,
                    text.as_deref(),
                    image.as_deref(),
                    reply_once,
                )
            })
            .await??)
    }

    /// 清空回复记录(FR-034);返回清除行数。
    pub async fn clear_records(&self, account_id: &str) -> Result<usize, RepliesError> {
        let account = account_id.to_string();
        Ok(self
            .db
            .call(move |conn| rules_ext::clear_default_reply_log(conn, &account))
            .await??)
    }

    /// 最近发送记录(审计展示;发送编排属 T036,此处只读)。
    pub async fn records(
        &self,
        account_id: &str,
        limit: i64,
    ) -> Result<Vec<rules_ext::DefaultReplyLogRow>, RepliesError> {
        let account = account_id.to_string();
        Ok(self
            .db
            .call(move |conn| rules_ext::list_default_reply_log(conn, &account, limit))
            .await??)
    }
}
