//! 专用数据库线程:独占业务连接,窄操作经有界队列提交,Tokio 侧一次性响应。
//! 短事务只包含本地校验与状态变化;网络等待绝不进入事务(research R2)。

use std::path::Path;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

const QUEUE_BOUND: usize = 256;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

#[derive(Clone)]
pub struct DbThread {
    tx: mpsc::Sender<Job>,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("数据库线程已关闭")]
    Closed,
    #[error("数据库操作失败:{0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl DbThread {
    /// 打开数据库并应用 WAL/FULL/FK/busy_timeout,启动专用线程。
    pub fn spawn(path: &Path) -> std::io::Result<Self> {
        let conn = Connection::open(path).map_err(|e| std::io::Error::other(e.to_string()))?;
        // journal_mode 会返回结果行,需用查询读取;其余用 pragma_update
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        conn.pragma_update(None, "busy_timeout", 2000)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let (tx, mut rx) = mpsc::channel::<Job>(QUEUE_BOUND);
        std::thread::Builder::new()
            .name("qing-db".into())
            .spawn(move || run_loop(conn, &mut rx))?;
        Ok(Self { tx })
    }

    /// 提交一个窄仓储操作并在 Tokio 侧等待结果。
    pub async fn call<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&mut Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = tx.send(f(conn));
        });
        self.tx.send(job).await.map_err(|_| DbError::Closed)?;
        rx.await.map_err(|_| DbError::Closed)
    }
}

fn run_loop(mut conn: Connection, rx: &mut mpsc::Receiver<Job>) {
    while let Some(job) = rx.blocking_recv() {
        job(&mut conn);
    }
    // 线程结束前尽力落盘(WAL 模式下 checkpoint 由 SQLite 常规机制处理)
    let _ = conn.pragma_update(None, "wal_checkpoint", "PASSIVE");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn db_thread_executes_narrow_ops() {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("t.db")).unwrap();
        let n: i64 = db
            .call(|conn| -> rusqlite::Result<i64> {
                conn.execute_batch("CREATE TABLE t(v INTEGER); INSERT INTO t(v) VALUES (41);")?;
                conn.query_row("SELECT v FROM t", [], |r| r.get(0))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 41);
    }
}
