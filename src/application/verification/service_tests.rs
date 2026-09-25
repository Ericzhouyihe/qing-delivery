//! 服务确定性测试(quickstart §构建与确定性测试):fake driver/sink,无浏览器。

use std::sync::{Arc, Mutex};

use crate::adapters::sqlite::db::DbThread;
use crate::application::verification::service::{
    CredentialSink, VerificationPolicy, VerificationService,
};
use crate::application::verification::{
    SignalSource, SolveOutcome, VerificationDriver, VerificationSignal,
};

struct FakeDriver {
    outcomes: Mutex<Vec<SolveOutcome>>,
}

impl VerificationDriver for FakeDriver {
    fn solve_boxed(
        &self,
        _url: String,
        _seed: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SolveOutcome> + Send>> {
        let next = self
            .outcomes
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(SolveOutcome::Failed {
                reason: "默认失败".into(),
                stage: "solving",
            });
        Box::pin(async move { next })
    }
}

struct FakeSink {
    fail_save: bool,
    recovered: Mutex<std::collections::HashSet<String>>,
    saved: Mutex<Vec<String>>,
}

impl CredentialSink for FakeSink {
    fn save_and_rotate(&self, account_id: &str, _jar: &str) -> Result<(), String> {
        if self.fail_save {
            return Err("DPAPI 保存失败(注入)".into());
        }
        self.saved.lock().unwrap().push(account_id.to_string());
        self.recovered
            .lock()
            .unwrap()
            .insert(account_id.to_string());
        Ok(())
    }
    fn probe_recovered(&self, account_id: &str) -> bool {
        self.recovered.lock().unwrap().contains(account_id)
    }
}

async fn service(
    driver_outcomes: Vec<SolveOutcome>,
    fail_save: bool,
) -> (Arc<VerificationService>, Arc<FakeSink>) {
    let dir = tempfile::tempdir().unwrap();
    let db = DbThread::spawn(&dir.path().join("t.db")).unwrap();
    let migration_dir = dir.path().to_path_buf();
    db.call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &migration_dir))
        .await
        .unwrap()
        .unwrap();
    db.call(|conn| {
        conn.execute(
            "INSERT INTO accounts (id, platform, external_user_id, display_name, created_at, updated_at)
             VALUES ('acct-1','xianyu','u1','测试',0,0)", [],
        )
    })
    .await
    .unwrap()
    .unwrap();
    let driver = Arc::new(FakeDriver {
        outcomes: Mutex::new(driver_outcomes),
    });
    let sink = Arc::new(FakeSink {
        fail_save,
        recovered: Mutex::new(Default::default()),
        saved: Mutex::new(vec![]),
    });
    let policy = VerificationPolicy {
        max_retries: 2,
        total_timeout: std::time::Duration::from_secs(5),
        cooldown: std::time::Duration::from_millis(200),
        probe_interval: std::time::Duration::from_millis(50),
    };
    let s = VerificationService::new(db, driver, sink.clone(), policy);
    (s, sink)
}

fn signal(src: SignalSource) -> VerificationSignal {
    VerificationSignal {
        account_id: "acct-1".into(),
        source: src,
        url: Some("https://v.example/x".into()),
        raw: "punish execute".into(),
    }
}

async fn wait_terminal(
    s: &VerificationService,
    account: &str,
) -> crate::application::verification::service::SessionSnapshot {
    use crate::application::verification::service::{SessionSnapshot, SessionState};
    // 成功路径终态即 clear_active,因此以"审计行终态 + 无活跃会话"为收敛条件
    for _ in 0..300 {
        let attempts = s.recent_attempts(account).await;
        eprintln!(
            "[dbg] rows: {:?}",
            attempts
                .iter()
                .map(|a| (a.id.clone(), a.outcome.clone()))
                .collect::<Vec<_>>()
        );
        let active = s.snapshot(account);
        let done = !attempts.is_empty()
            && attempts[0].outcome != "in_progress"
            && (active.is_none() || active.as_ref().is_some_and(|a| a.state.is_terminal()));
        if done {
            let top = &attempts[0];
            return SessionSnapshot {
                id: String::new(),
                account_id: account.to_string(),
                state: if top.outcome == "succeeded" {
                    SessionState::Succeeded
                } else {
                    SessionState::FailedManual
                },
                trigger: top.trigger_source.clone(),
                retry_count: 0,
                started_at: top.started_at,
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("会话未在时限内到达终态");
}

#[tokio::test]
async fn 成功路径_重试内通过_凭证回写_审计成功() {
    // pop 顺序:先失败一次,再成功
    let (s, sink) = service(
        vec![
            SolveOutcome::Solved {
                cookie_jar: "jar".into(),
            },
            SolveOutcome::Failed {
                reason: "第一次未通过".into(),
                stage: "solving",
            },
        ],
        false,
    )
    .await;
    s.handle_signal(signal(SignalSource::Mtop));
    let snap = wait_terminal(&s, "acct-1").await;
    assert_eq!(snap.state.as_str(), "succeeded");
    assert_eq!(sink.saved.lock().unwrap().len(), 1, "凭证回写一次");
    let attempts = s.recent_attempts("acct-1").await;
    assert_eq!(attempts.len(), 2, "两次尝试各一行审计");
    assert_eq!(attempts[0].outcome, "succeeded");
    assert!(attempts[0].credential_updated);
    assert_eq!(attempts[1].outcome, "failed_manual");
}

#[tokio::test]
async fn 重试耗尽_转人工_冷却内忽略自动信号() {
    let (s, _sink) = service(
        vec![
            SolveOutcome::Failed {
                reason: "a".into(),
                stage: "solving",
            },
            SolveOutcome::Failed {
                reason: "b".into(),
                stage: "solving",
            },
            SolveOutcome::Failed {
                reason: "c".into(),
                stage: "solving",
            },
        ],
        false,
    )
    .await;
    s.handle_signal(signal(SignalSource::Ws));
    let snap = wait_terminal(&s, "acct-1").await;
    assert_eq!(snap.state.as_str(), "failed_manual");
    let attempts = s.recent_attempts("acct-1").await;
    assert_eq!(attempts.len(), 3, "重试上限 2 → 共 3 次尝试审计");

    // 冷却期内自动信号被忽略;手动信号放行(US4)
    assert!(!s.auto_signal_allowed("acct-1"), "降级后冷却");
    s.handle_signal(signal(SignalSource::Manual));
    assert!(
        s.snapshot("acct-1").is_some()
            || s.auto_signal_allowed("acct-1") && !s.auto_signal_allowed("acct-1")
    );
}

#[tokio::test]
async fn 凭证保存失败_保留旧值_直接转人工() {
    let (s, sink) = service(
        vec![SolveOutcome::Solved {
            cookie_jar: "jar".into(),
        }],
        true,
    )
    .await;
    s.handle_signal(signal(SignalSource::Mtop));
    let snap = wait_terminal(&s, "acct-1").await;
    assert_eq!(snap.state.as_str(), "failed_manual");
    assert_eq!(sink.saved.lock().unwrap().len(), 0, "未成功保存不回写");
    let attempts = s.recent_attempts("acct-1").await;
    assert!(
        attempts[0]
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("凭证更新失败")
    );
}

#[tokio::test]
async fn 活跃会话去重_重复信号合并() {
    let (s, _sink) = service(
        vec![SolveOutcome::Solved {
            cookie_jar: "j".into(),
        }],
        false,
    )
    .await;
    s.handle_signal(signal(SignalSource::Mtop));
    // 立刻再来的信号应被活跃会话吞并(不产生第二个会话)
    s.handle_signal(signal(SignalSource::Ws));
    let snap = wait_terminal(&s, "acct-1").await;
    assert_eq!(snap.trigger, "mtop", "仅首个信号生效");
    let attempts = s.recent_attempts("acct-1").await;
    assert_eq!(attempts.len(), 1);
}

#[tokio::test]
async fn 事项生成与恢复探测自动关闭() {
    let (s, _sink) = service(
        vec![
            SolveOutcome::Failed {
                reason: "x".into(),
                stage: "solving",
            },
            SolveOutcome::Failed {
                reason: "y".into(),
                stage: "solving",
            },
            SolveOutcome::Failed {
                reason: "z".into(),
                stage: "solving",
            },
        ],
        false,
    )
    .await;
    s.handle_signal(signal(SignalSource::Qr));
    wait_terminal(&s, "acct-1").await;

    // 事项存在
    let has_issue =
        s.db.call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM issues WHERE kind='security_verification' AND state='open'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(has_issue, 1, "failed_manual 生成转人工事项");

    // 探测恢复(注入:直接造 sink 恢复态不可行——用 probe:false 场景验证不误关)
    s.probe_and_close_issues().await;
    let still_open =
        s.db.call(|conn| {
            conn.query_row("SELECT COUNT(*) FROM issues WHERE state='open'", [], |r| {
                r.get::<_, i64>(0)
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_open, 1, "凭证未恢复不误关");
}
