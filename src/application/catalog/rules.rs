//! 规则用例(T057/T058/T059/T060):精确范围 CRUD、内容版本不可变、
//! 产品限制 1000 Unicode 标量 / 4000 UTF-8 字节(research R7,verbatim)、
//! 保存与执行前双重校验;预览纯校验零发送。

use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{items, rules};
use crate::adapters::windows::keys::DataKey;
use crate::domain::crypto;
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

pub const MAX_UNICODE_SCALARS: usize = 1000;
pub const MAX_UTF8_BYTES: usize = 4000;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("正文为空")]
    EmptyContent,
    #[error(
        "正文超过限制:{scalars} 标量(上限 {MAX_UNICODE_SCALARS})/{bytes} 字节(上限 {MAX_UTF8_BYTES});不截断、不拆分"
    )]
    ContentTooLong { scalars: usize, bytes: usize },
    #[error("同一完整匹配范围已有启用规则(FR-008)")]
    EnabledConflict,
    #[error("规则不存在")]
    NotFound,
    #[error("商品不存在或不属于该账号")]
    ItemNotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, PartialEq)]
pub struct ContentCheck {
    pub valid: bool,
    pub unicode_scalars: usize,
    pub utf8_bytes: usize,
    pub violations: Vec<String>,
}

/// 产品限制校验(保存与执行前同源;保留换行/链接/中文)。
pub fn check_content(text: &str) -> ContentCheck {
    let scalars = text.chars().count();
    let bytes = text.len();
    let mut violations = Vec::new();
    if text.trim().is_empty() {
        violations.push("正文为空".to_string());
    }
    if scalars > MAX_UNICODE_SCALARS || bytes > MAX_UTF8_BYTES {
        violations.push(format!(
            "正文超限:{scalars} 标量/上限 {MAX_UNICODE_SCALARS},{bytes} 字节/上限 {MAX_UTF8_BYTES}"
        ));
    }
    ContentCheck {
        valid: violations.is_empty(),
        unicode_scalars: scalars,
        utf8_bytes: bytes,
        violations,
    }
}

pub struct RuleDraft {
    pub item_id: String,
    pub sku_key: String,
    pub content: String,
    pub enabled: bool,
}

#[derive(Debug)]
pub struct SavedRule {
    pub id: String,
    pub version: i64,
    pub content_version: i64,
}

#[derive(Clone)]
pub struct RuleService {
    db: DbThread,
    key: DataKey,
}

impl RuleService {
    pub fn new(db: DbThread, key: DataKey) -> Self {
        Self { db, key }
    }

    /// 预览:纯校验,不保存、不发平台请求(US3-2)。
    pub fn preview(&self, text: &str) -> ContentCheck {
        check_content(text)
    }

    /// 创建规则(不带 expected_version);启用唯一冲突 422 语义由 EnabledConflict 表达。
    pub async fn create(&self, account_id: &str, draft: RuleDraft) -> Result<SavedRule, RuleError> {
        let check = check_content(&draft.content);
        if !check.valid {
            return Err(if draft.content.trim().is_empty() {
                RuleError::EmptyContent
            } else {
                RuleError::ContentTooLong {
                    scalars: check.unicode_scalars,
                    bytes: check.utf8_bytes,
                }
            });
        }
        let key = self.key.key;
        let key_id = self.key.key_id.clone();
        let account = account_id.to_string();
        let draft_content = draft.content.clone();
        let draft_enabled = draft.enabled;
        let draft_item = draft.item_id.clone();
        let draft_sku = draft.sku_key.clone();

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<SavedRule, RuleError>> {
                    // 归属检查:商品必须属于该账号
                    let Some(item) = items::get(conn, &draft_item)? else {
                        return Ok(Err(RuleError::ItemNotFound));
                    };
                    if item.account_id != account {
                        return Ok(Err(RuleError::ItemNotFound));
                    }
                    // 启用范围唯一(FR-008):保存时阻止
                    if draft_enabled
                        && rules::count_enabled_exact(conn, &account, &draft_item, &draft_sku)? > 0
                    {
                        return Ok(Err(RuleError::EnabledConflict));
                    }
                    let rule = rules::insert(
                        conn,
                        &ids::new_id("rul"),
                        &account,
                        &draft_item,
                        &draft_sku,
                        draft_enabled,
                    )?;
                    let digest = hex::encode(Sha256::digest(draft_content.as_bytes()));
                    let envelope = crypto::seal(
                        &key,
                        &key_id,
                        &rules::rule_aad(&rule.id, 1),
                        draft_content.as_bytes(),
                    );
                    rules::insert_content(
                        conn,
                        &ids::new_id("rc"),
                        &rule.id,
                        1,
                        &envelope,
                        &digest,
                        draft_content.chars().count() as i64,
                        draft_content.len() as i64,
                    )?;
                    let fresh =
                        rules::get(conn, &rule.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(SavedRule {
                        id: rule.id,
                        version: fresh.version,
                        content_version: 1,
                    }))
                },
            )
            .await??
    }

    /// 版本化更新(编辑/禁用/启用均走此处);expected_version 必填。
    pub async fn update(
        &self,
        account_id: &str,
        rule_id: &str,
        expected_version: i64,
        new_content: Option<String>,
        enabled: Option<bool>,
    ) -> Result<SavedRule, RuleError> {
        let key = self.key.key;
        let key_id = self.key.key_id.clone();
        let account = account_id.to_string();
        let rule_owned = rule_id.to_string();

        self.db
            .call(
                move |conn| -> rusqlite::Result<Result<SavedRule, RuleError>> {
                    let Some(rule) = rules::get(conn, &rule_owned)? else {
                        return Ok(Err(RuleError::NotFound));
                    };
                    if rule.account_id != account {
                        return Ok(Err(RuleError::NotFound));
                    }
                    if rule.version != expected_version {
                        return Ok(Err(RuleError::VersionConflict));
                    }
                    // 启用唯一:切换到启用时检查
                    if enabled == Some(true)
                        && !rule.enabled
                        && rules::count_enabled_exact(conn, &account, &rule.item_id, &rule.sku_key)?
                            > 0
                    {
                        return Ok(Err(RuleError::EnabledConflict));
                    }
                    // 内容更新 = 新的不可变版本;旧版本与已冻结快照不受影响(FR-009)
                    let mut content_version = rule.current_content_version;
                    if let Some(text) = &new_content {
                        let check = check_content(text);
                        if !check.valid {
                            return Ok(Err(if text.trim().is_empty() {
                                RuleError::EmptyContent
                            } else {
                                RuleError::ContentTooLong {
                                    scalars: check.unicode_scalars,
                                    bytes: check.utf8_bytes,
                                }
                            }));
                        }
                        content_version += 1;
                        let digest = hex::encode(Sha256::digest(text.as_bytes()));
                        let envelope = crypto::seal(
                            &key,
                            &key_id,
                            &rules::rule_aad(&rule.id, content_version),
                            text.as_bytes(),
                        );
                        rules::insert_content(
                            conn,
                            &ids::new_id("rc"),
                            &rule.id,
                            content_version,
                            &envelope,
                            &digest,
                            text.chars().count() as i64,
                            text.len() as i64,
                        )?;
                    }
                    if let Some(enable) = enabled {
                        conn.execute(
                            "UPDATE rules SET enabled = ?2, version = version + 1, updated_at = ?3
                         WHERE id = ?1",
                            rusqlite::params![rule_owned, enable as i64, utc_now_ms()],
                        )?;
                    }
                    let fresh = rules::get(conn, &rule_owned)?
                        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    Ok(Ok(SavedRule {
                        id: fresh.id,
                        version: fresh.version,
                        content_version: fresh.current_content_version,
                    }))
                },
            )
            .await??
    }

    /// 匹配预览(T059):确定性精确匹配,不发送正文。
    pub async fn match_preview(
        &self,
        account_id: &str,
        item_id: &str,
        sku_key: &str,
    ) -> Result<MatchPreview, RuleError> {
        let account = account_id.to_string();
        let item = item_id.to_string();
        let sku = sku_key.to_string();
        let result = self
            .db
            .call(move |conn| -> rusqlite::Result<MatchPreview> {
                match rules::count_enabled_exact(conn, &account, &item, &sku)? {
                    0 => Ok(MatchPreview::None),
                    1 => {
                        let rule = rules::find_enabled_exact(conn, &account, &item, &sku)?;
                        match rule {
                            Some(r) => Ok(MatchPreview::Matched {
                                rule_id: r.id,
                                rule_version: r.version,
                            }),
                            None => Ok(MatchPreview::None),
                        }
                    }
                    _ => Ok(MatchPreview::Ambiguous),
                }
            })
            .await??;
        Ok(result)
    }
}

#[derive(Debug, PartialEq)]
pub enum MatchPreview {
    Matched { rule_id: String, rule_version: i64 },
    None,
    Ambiguous,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;
    use crate::adapters::sqlite::repos::accounts;
    use crate::domain::sku::{SkuPart, combo_key};

    async fn setup() -> (tempfile::TempDir, RuleService, DbThread) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir =
            crate::adapters::windows::datadir::DataDir::resolve(Some(dir.path())).unwrap();
        let key = crate::adapters::windows::keys::load_or_create(&data_dir.root).unwrap();
        let db = DbThread::spawn(&data_dir.join("rules.db")).unwrap();
        let dir_path = data_dir.root.clone();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        db.call(|conn| {
            accounts::upsert_identity(conn, "acct-1", "xianyu", "seller-1", "卖家")?;
            items::upsert(
                conn, "item-1", "acct-1", "EXT-1", "资料", "on_sale", "[]", "single",
            )
        })
        .await
        .unwrap()
        .unwrap();
        (dir, RuleService::new(db.clone(), key), db)
    }

    fn text_ok() -> String {
        "链接 https://example.com/d\n提取码 ab12".to_string()
    }

    #[tokio::test]
    async fn content_limits_enforced_verbatim() {
        let (_d, svc, _db) = setup().await;
        // 空正文拒绝
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: "  ".into(),
                    enabled: false,
                }
            )
            .await,
            Err(RuleError::EmptyContent)
        ));
        // 超长拒绝:1001 个标量
        let too_long = "字".repeat(MAX_UNICODE_SCALARS + 1);
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: too_long,
                    enabled: false,
                }
            )
            .await,
            Err(RuleError::ContentTooLong { .. })
        ));
        // 字节数超限但标量不超(ASCII 4001)
        let too_many_bytes = "a".repeat(MAX_UTF8_BYTES + 1);
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: too_many_bytes,
                    enabled: false,
                }
            )
            .await,
            Err(RuleError::ContentTooLong { .. })
        ));
        // 预览同源校验
        let p = svc.preview(&"a".repeat(MAX_UTF8_BYTES + 1));
        assert!(!p.valid);
    }

    #[tokio::test]
    async fn enabled_scope_uniqueness_and_version_update() {
        let (_d, svc, db) = setup().await;
        let saved = svc
            .create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        // 同范围第二条启用 → 冲突
        assert!(matches!(
            svc.create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: true,
                }
            )
            .await,
            Err(RuleError::EnabledConflict)
        ));
        // 禁用的第二条允许
        let second = svc
            .create(
                "acct-1",
                RuleDraft {
                    item_id: "item-1".into(),
                    sku_key: "single".into(),
                    content: text_ok(),
                    enabled: false,
                },
            )
            .await
            .unwrap();
        // 内容更新 → 版本 2,旧版本保留
        let updated = svc
            .update(
                "acct-1",
                &saved.id,
                saved.version,
                Some("新内容 v2".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.content_version, 2);
        let old_probe = saved.id.clone();
        let old = db
            .call(move |conn| rules::get_content(conn, &old_probe, 1))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(old.content_version, 1, "v1 is retained immutably");

        // 禁用后第二条可启用
        let saved_id = saved.id.clone();
        svc.update("acct-1", &saved_id, updated.version, None, Some(false))
            .await
            .unwrap();
        svc.update("acct-1", &second.id, second.version, None, Some(true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn match_preview_exact() {
        let (_d, svc, _db) = setup().await;
        let combo = combo_key(&[SkuPart {
            property_id: "p1".into(),
            value_id: "v1".into(),
            property_label: String::new(),
            value_label: String::new(),
        }])
        .unwrap();
        assert_eq!(
            svc.match_preview("acct-1", "item-1", &combo).await.unwrap(),
            MatchPreview::None
        );
        svc.create(
            "acct-1",
            RuleDraft {
                item_id: "item-1".into(),
                sku_key: combo.clone(),
                content: text_ok(),
                enabled: true,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            svc.match_preview("acct-1", "item-1", &combo).await.unwrap(),
            MatchPreview::Matched { .. }
        ));
        // 其他组合不命中(不做子串/部分匹配)
        let other = combo_key(&[SkuPart {
            property_id: "p1".into(),
            value_id: "v2".into(),
            property_label: String::new(),
            value_label: String::new(),
        }])
        .unwrap();
        assert_eq!(
            svc.match_preview("acct-1", "item-1", &other).await.unwrap(),
            MatchPreview::None
        );
    }
}
