//! 商品同步作业(T055):逐页拉取,空页正常、失败/不完整不下架;
//! 只有完整遍历后,未出现的商品才标记 off_sale(FR-006)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::items;
use crate::application::ports::platform::{
    PlatformAdapter, PlatformError, ProductRecord, RequestContext,
};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

/// 分页保护上限:超过视为同步不完整(可审计缺口,不宣称全量)。
pub const MAX_SYNC_PAGES: u32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

pub struct SyncResult {
    pub job_id: String,
    /// running|complete|incomplete|failed
    pub state: String,
    pub fetched_count: usize,
    pub failure_reason: Option<String>,
    /// 未出现而被标记下架的商品数(仅 complete 时 >0)
    pub delisted: usize,
}

pub struct ItemSyncService<A: PlatformAdapter> {
    db: DbThread,
    adapter: A,
}

impl<A: PlatformAdapter> ItemSyncService<A> {
    pub fn new(db: DbThread, adapter: A) -> Self {
        Self { db, adapter }
    }

    /// 执行一次全量同步;job 记录 state 语义与 http-api ItemSyncResult 对齐。
    pub async fn run(
        &self,
        account_id: &str,
        ctx: &RequestContext,
    ) -> Result<SyncResult, SyncError> {
        let job_id = ids::new_id("sync");
        let job_id_for_return = job_id.clone();
        let mut fetched = 0usize;
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0u32;
        let (state, failure) = loop {
            if pages >= MAX_SYNC_PAGES {
                break ("incomplete".to_string(), Some("分页上限耗尽".to_string()));
            }
            match self.adapter.list_products(ctx, cursor.as_deref()).await {
                Ok(page) => {
                    pages += 1;
                    fetched += page.items.len();
                    let records = page.items.clone();
                    let job = job_id.clone();
                    let account = account_id.to_string();
                    let page_seen: Vec<String> =
                        records.iter().map(|r| r.external_item_id.clone()).collect();
                    self.db
                        .call(move |conn| -> rusqlite::Result<()> {
                            for r in &records {
                                upsert_record(conn, &account, r, &job)?;
                            }
                            Ok(())
                        })
                        .await??;
                    seen.extend(page_seen);
                    match page.next_cursor {
                        Some(next) if !next.is_empty() => cursor = Some(next),
                        _ => break ("complete".to_string(), None),
                    }
                }
                Err(e) => {
                    let reason = e.to_string();
                    break ("failed".to_string(), Some(reason));
                }
            }
        };

        // 完整同步才允许下架缺失商品;不完整保留已知事实(FR-006)
        let mut delisted = 0usize;
        if state == "complete" {
            let account = account_id.to_string();
            delisted = self
                .db
                .call(move |conn| -> rusqlite::Result<usize> {
                    let mut stmt = conn
                        .prepare("SELECT id, external_item_id FROM items WHERE account_id = ?1")?;
                    let all: Vec<(String, String)> = stmt
                        .query_map([&account], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?;
                    let mut n = 0;
                    for (id, ext) in all {
                        if !seen.contains(&ext) {
                            conn.execute(
                                "UPDATE items SET listing_state = 'off_sale', version = version + 1,
                                        updated_at = ?2 WHERE id = ?1",
                                rusqlite::params![id, utc_now_ms()],
                            )?;
                            n += 1;
                        }
                    }
                    Ok(n)
                })
                .await??;
        }

        let account = account_id.to_string();
        let (state_owned, failure_owned, fetched_count, _delisted_count) =
            (state.clone(), failure.clone(), fetched, delisted);
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                conn.execute(
                    "INSERT INTO sync_jobs(id, account_id, kind, status, complete, gap_reason,
                             coverage_from, coverage_to, created_at, updated_at)
                     VALUES (?1, ?2, 'items', ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    rusqlite::params![
                        job_id,
                        account,
                        state_owned,
                        (state_owned == "complete") as i64,
                        failure_owned,
                        utc_now_ms(),
                        utc_now_ms(),
                        utc_now_ms()
                    ],
                )?;
                Ok(())
            })
            .await??;

        Ok(SyncResult {
            job_id: job_id_for_return,
            state,
            fetched_count,
            failure_reason: failure,
            delisted,
        })
    }
}

fn upsert_record(
    conn: &rusqlite::Connection,
    account_id: &str,
    r: &ProductRecord,
    job_id: &str,
) -> rusqlite::Result<()> {
    let completeness = if r.sku_complete {
        "complete"
    } else if r.single_sku_confirmed {
        "single"
    } else {
        "incomplete"
    };
    items::upsert(
        conn,
        &ids::new_id("itm"),
        account_id,
        &r.external_item_id,
        &r.title,
        if r.on_sale { "on_sale" } else { "off_sale" },
        &serde_json::to_string(&r.sku_parts).unwrap_or_else(|_| "[]".into()),
        completeness,
    )?;
    conn.execute(
        "UPDATE items SET last_seen_sync = ?2 WHERE account_id = ?1 AND external_item_id = ?3",
        rusqlite::params![account_id, job_id, r.external_item_id],
    )?;
    Ok(())
}
