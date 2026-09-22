//! 任务用例:202 任务句柄的状态机与查询。取消仅表达意图;
//! 已提交的外部动作不被撤销,由各执行器解释(http-api §4)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::jobs as repo;
use crate::domain::ids;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
    NeedsReview,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::CancelRequested => "cancel_requested",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::NeedsReview => "needs_review",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled | JobState::NeedsReview
        )
    }
}

pub use repo::JobRow;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("任务不存在")]
    NotFound,
    #[error("版本冲突,请刷新后重试")]
    VersionConflict,
    #[error("任务已处于终态,不能取消")]
    Terminal,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Clone)]
pub struct JobService {
    db: DbThread,
}

impl JobService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    pub async fn create(&self, kind: &str, target_id: Option<&str>) -> Result<JobRow, JobError> {
        let id = ids::new_id("job");
        let kind = kind.to_string();
        let target = target_id.map(|s| s.to_string());
        Ok(self
            .db
            .call(move |conn| repo::insert(conn, &id, &kind, target.as_deref()))
            .await??)
    }

    pub async fn get(&self, id: &str) -> Result<JobRow, JobError> {
        let id = id.to_string();
        self.db
            .call(move |conn| repo::get(conn, &id))
            .await??
            .ok_or(JobError::NotFound)
    }

    pub async fn request_cancel(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<JobRow, JobError> {
        let id_owned = id.to_string();
        let updated = self
            .db
            .call(move |conn| repo::request_cancel(conn, &id_owned, expected_version))
            .await??;
        if !updated {
            let row = self.get(id).await?;
            if row.version != expected_version + 1 {
                return Err(JobError::VersionConflict);
            }
            // 已是 cancel_requested 的幂等重放视为成功
        }
        self.get(id).await
    }
}
