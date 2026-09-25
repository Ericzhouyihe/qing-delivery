//! 验证处置服务(003 核心,研究 D3/D5):状态机/去重合并/重试/超时/冷却/降级/审计。
//! Driver 经端口注入(确定性测试用 fake);凭证回写经 CredentialSink 注入。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::verification as attempts;
use crate::application::verification::gate::DeliveryGate;
use crate::application::verification::{SolveOutcome, VerificationDriver, VerificationSignal};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;

/// 策略(规格内建值;测试可注入更小值)。
#[derive(Clone, Debug)]
pub struct VerificationPolicy {
    pub max_retries: u32,         // 默认 2
    pub total_timeout: Duration,  // 默认 120s
    pub cooldown: Duration,       // 降级后 ≥5min
    pub probe_interval: Duration, // 凭证恢复探测 60s
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            total_timeout: Duration::from_secs(120),
            cooldown: Duration::from_secs(5 * 60),
            probe_interval: Duration::from_secs(60),
        }
    }
}

/// 凭证回写出口:成功时保存+轮换;失败必须保留旧值(FR-014)。
pub trait CredentialSink: Send + Sync {
    /// 返回 Err 时服务转 failed_manual 且不 bump epoch。
    fn save_and_rotate(&self, account_id: &str, cookie_jar: &str) -> Result<(), String>;
    /// failed_manual 后的恢复探测:true = 凭证已恢复。
    fn probe_recovered(&self, account_id: &str) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Detected,
    BrowserOpen,
    Solving,
    UpdatingCredential,
    Succeeded,
    FailedManual,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Detected => "detected",
            SessionState::BrowserOpen => "browser_open",
            SessionState::Solving => "solving",
            SessionState::UpdatingCredential => "updating_credential",
            SessionState::Succeeded => "succeeded",
            SessionState::FailedManual => "failed_manual",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionState::Succeeded | SessionState::FailedManual)
    }
}

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub id: String,
    pub account_id: String,
    pub state: SessionState,
    pub trigger: String,
    pub retry_count: u32,
    pub started_at: i64,
}

struct Active {
    snapshot: SessionSnapshot,
}

/// 测试需要直查库(事项断言);字段可见性限 crate 内。
pub struct VerificationService {
    /// 测试直查库断言;可见性限 crate 内。
    pub(crate) db: DbThread,
    driver: Arc<dyn VerificationDriver>,
    sink: Arc<dyn CredentialSink>,
    policy: VerificationPolicy,
    pub(crate) gate: Arc<DeliveryGate>,
    active: Mutex<HashMap<String, Active>>,
    /// 账号 → 上次降级时刻(冷却,防紧密重试)
    cooldown_until: Mutex<HashMap<String, Instant>>,
}

impl VerificationService {
    pub fn new(
        db: DbThread,
        driver: Arc<dyn VerificationDriver>,
        sink: Arc<dyn CredentialSink>,
        policy: VerificationPolicy,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            driver,
            sink,
            policy,
            gate: Arc::new(DeliveryGate::new()),
            active: Mutex::new(HashMap::new()),
            cooldown_until: Mutex::new(HashMap::new()),
        })
    }

    pub fn policy(&self) -> &VerificationPolicy {
        &self.policy
    }

    pub fn snapshot(&self, account_id: &str) -> Option<SessionSnapshot> {
        self.active
            .lock()
            .unwrap()
            .get(account_id)
            .map(|a| a.snapshot.clone())
    }

    pub async fn recent_attempts(&self, account_id: &str) -> Vec<attempts::AttemptRow> {
        let account = account_id.to_string();
        match self
            .db
            .call(move |conn| attempts::list_recent(conn, &account, 20))
            .await
        {
            Ok(Ok(rows)) => rows,
            _ => Vec::new(),
        }
    }

    /// 冷却期内(降级后)拒绝自动拉起;返回 false 表示应忽略该自动信号。
    pub fn auto_signal_allowed(&self, account_id: &str) -> bool {
        let map = self.cooldown_until.lock().unwrap();
        match map.get(account_id) {
            Some(t) => Instant::now() >= *t,
            None => true,
        }
    }

    /// 统一入口:信号 → 会话(去重合并:活跃会话存在时忽略新信号)。
    pub fn handle_signal(self: &Arc<Self>, signal: VerificationSignal) {
        if signal.source != crate::application::verification::SignalSource::Manual
            && !self.auto_signal_allowed(&signal.account_id)
        {
            return; // 冷却期内不紧密重试(FR-004)
        }
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(&signal.account_id) {
                // 陈旧会话自愈:超过 总超时+60s 仍活跃 = 任务已死(panic/丢失),清理后放行
                let stale_after = self.policy.total_timeout + std::time::Duration::from_secs(60);
                let started = active[&signal.account_id].snapshot.started_at;
                let is_stale = utc_now_ms() - started > stale_after.as_millis() as i64;
                if !is_stale {
                    return; // 活跃会话存在(去重合并)
                }
                eprintln!("[verify] 清理陈旧会话 {}", signal.account_id);
                self.gate.release(&signal.account_id);
                active.remove(&signal.account_id);
            }
            // 同步登记活跃会话:去重窗口从信号到达即生效(spawn 前置)
            let snapshot = SessionSnapshot {
                id: ids::new_id("vs"),
                account_id: signal.account_id.clone(),
                state: SessionState::Detected,
                trigger: signal.source.as_str().to_string(),
                retry_count: 0,
                started_at: utc_now_ms(),
            };
            self.gate.hold(&signal.account_id); // 003 FR-003:会话活跃即关闸
            active.insert(signal.account_id.clone(), Active { snapshot });
        }
        let service = self.clone();
        let panic_account = signal.account_id.clone();
        tokio::spawn(async move {
            let svc2 = service.clone();
            let fut = svc2.run_session(signal);
            if futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(fut))
                .await
                .is_err()
            {
                // panic 路径也必须释放会话与闸门,否则账号永久卡"活跃会话"(409)
                eprintln!("[verify] session PANIC: 释放 {panic_account}");
                service.clear_active(&panic_account);
            }
        });
    }

    async fn run_session(self: Arc<Self>, signal: VerificationSignal) {
        let account_id = signal.account_id.clone();
        let session_id = ids::new_id("vs");
        let attempt_base = session_id.clone();
        let mut attempt_ids: Vec<String> = Vec::new();
        let mut last_failure: Option<String> = None;
        let deadline = Instant::now() + self.policy.total_timeout;
        let trigger = signal.source.as_str().to_string();

        let snapshot = SessionSnapshot {
            id: session_id.clone(),
            account_id: account_id.clone(),
            state: SessionState::Detected,
            trigger: trigger.clone(),
            retry_count: 0,
            started_at: utc_now_ms(),
        };
        self.set_active(snapshot);

        let url = signal.url.clone().unwrap_or_default();
        let raw_short: String = signal.raw.chars().take(120).collect();

        // 信号未携带验证页 URL:拉浏览器无意义(空导航只会超时),
        // 快速转人工,让用户在平台完成验证后经「我已处理」/恢复检测闭环。
        if url.is_empty() {
            last_failure =
                Some("信号未携带验证页 URL(平台未返回);请在闲鱼 App 或网页版完成安全验证".into());
            self.cooldown_until
                .lock()
                .unwrap()
                .insert(account_id.clone(), Instant::now() + self.policy.cooldown);
            self.transition(&account_id, SessionState::FailedManual);
            self.open_issue(&account_id, "", &attempt_ids, last_failure.as_deref())
                .await;
            self.clear_active(&account_id);
            return;
        }

        'attempts: for attempt in 0..=self.policy.max_retries {
            if Instant::now() >= deadline {
                last_failure = Some("总超时(120s)".into());
                break;
            }
            let vat_id = format!("{attempt_base}-a{attempt}");
            attempt_ids.push(vat_id.clone());
            let vat_insert = vat_id.clone();
            let opt_url: Option<String> = if url.is_empty() {
                None
            } else {
                Some(url.clone())
            };
            let (acc, reason, trig) = (account_id.clone(), raw_short.clone(), trigger.clone());
            let _ = self
                .db
                .call(move |conn| {
                    attempts::insert(conn, &vat_insert, &acc, &trig, &reason, opt_url.as_deref())
                })
                .await;

            self.transition(&account_id, SessionState::BrowserOpen);
            self.transition(&account_id, SessionState::Solving);
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = if std::env::var("QING_FORCE_SLIDER_FAIL").is_ok() {
                SolveOutcome::Failed {
                    reason: "故障注入:滑块未通过".into(),
                    stage: "solving",
                }
            } else {
                match tokio::time::timeout(
                    remaining,
                    self.driver.solve_boxed(url.clone(), String::new()),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => SolveOutcome::Failed {
                        reason: "处置超时".into(),
                        stage: "timeout",
                    },
                }
            };

            match result {
                SolveOutcome::Solved { cookie_jar } => {
                    self.transition(&account_id, SessionState::UpdatingCredential);
                    match self.sink.save_and_rotate(&account_id, &cookie_jar) {
                        Ok(()) => {
                            let v = vat_id.clone();
                            let _ = self
                                .db
                                .call(move |conn| {
                                    attempts::finish(conn, &v, "succeeded", true, None)
                                })
                                .await;
                            self.transition(&account_id, SessionState::Succeeded);
                            self.clear_active(&account_id);
                            return; // 成功即终态
                        }
                        Err(e) => {
                            last_failure = Some(format!("凭证更新失败:{e}"));
                            let v = vat_id.clone();
                            let reason = last_failure.clone();
                            let _ = self
                                .db
                                .call(move |conn| {
                                    attempts::finish(
                                        conn,
                                        &v,
                                        "failed_manual",
                                        false,
                                        reason.as_deref(),
                                    )
                                })
                                .await;
                            break 'attempts; // 保留旧凭证,直接转人工(FR-014)
                        }
                    }
                }
                SolveOutcome::Failed { reason, stage: _ } => {
                    last_failure = Some(reason.clone());
                    let v = vat_id.clone();
                    let r = Some(reason);
                    let _ = self
                        .db
                        .call(move |conn| {
                            attempts::finish(conn, &v, "failed_manual", false, r.as_deref())
                        })
                        .await;
                    if Instant::now() >= deadline || attempt >= self.policy.max_retries {
                        break 'attempts;
                    }
                }
            }
        }

        // 降级:冷却登记 + failed_manual 终态 + 转人工事项
        self.cooldown_until
            .lock()
            .unwrap()
            .insert(account_id.clone(), Instant::now() + self.policy.cooldown);
        self.transition(&account_id, SessionState::FailedManual);
        self.open_issue(&account_id, &url, &attempt_ids, last_failure.as_deref())
            .await;
        self.clear_active(&account_id);
    }

    async fn open_issue(
        &self,
        account_id: &str,
        url: &str,
        attempt_ids: &[String],
        failure: Option<&str>,
    ) {
        let iss = ids::new_id("iss");
        let acc = account_id.to_string();
        let reason = failure.unwrap_or("验证未通过").to_string();
        let metadata = serde_json::json!({
            "verification_url": if url.is_empty() { None } else { Some(url) },
            "attempt_ids": attempt_ids,
        })
        .to_string();
        let allowed = "[\"open_verification\",\"resolve\"]";
        let now = utc_now_ms();
        let _ = self
            .db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO issues (id, account_id, kind, reason_code, allowed_actions, state, created_at, metadata)
                     VALUES (?1, ?2, 'security_verification', ?3, ?4, 'open', ?5, ?6)",
                    rusqlite::params![iss, acc, reason, allowed, now, metadata],
                )
            })
            .await;
    }

    /// failed_manual 事项存在时的恢复探测(FR-005,60s 周期由调用方驱动)。
    pub async fn probe_and_close_issues(&self) {
        let recovered: Vec<String> = {
            // 探测所有有待人工验证事项的账号
            let rows_res = self
                .db
                .call(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT DISTINCT i.account_id, i.id FROM issues i
                         WHERE i.state='open' AND i.kind='security_verification'",
                    )?;
                    let r = stmt
                        .query_map([], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok::<_, rusqlite::Error>(r)
                })
                .await;
            let rows: Vec<(String, String)> = match rows_res {
                Ok(Ok(r)) => r,
                _ => Vec::new(),
            };
            rows.into_iter()
                .filter(|(acc, _)| self.sink.probe_recovered(acc))
                .map(|(_, iss)| iss)
                .collect()
        };
        for iss in recovered {
            let now = utc_now_ms();
            let _ = self
                .db
                .call(move |conn| {
                    conn.execute(
                        "UPDATE issues SET state='resolved', resolved_at=?2, version=version+1
                     WHERE id=?1 AND state='open'",
                        rusqlite::params![iss, now],
                    )
                })
                .await;
        }
    }

    fn set_active(&self, snapshot: SessionSnapshot) {
        self.active
            .lock()
            .unwrap()
            .insert(snapshot.account_id.clone(), Active { snapshot });
    }

    fn transition(&self, account_id: &str, state: SessionState) {
        if let Some(a) = self.active.lock().unwrap().get_mut(account_id) {
            a.snapshot.state = state;
        }
    }

    fn clear_active(&self, account_id: &str) {
        self.gate.release(account_id);
        self.active.lock().unwrap().remove(account_id);
    }
}
