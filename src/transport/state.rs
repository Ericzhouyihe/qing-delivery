//! HTTP 应用状态:配置、服务句柄与进程内辅助结构。

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::Digest;

use crate::adapters::windows::keys::DataKey;
use crate::application::auth::AdminAuth;
use crate::application::idempotency::IdempotencyService;
use crate::application::jobs::JobService;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionProfile {
    Live,
    Mock,
}

impl ExecutionProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionProfile::Live => "live",
            ExecutionProfile::Mock => "mock",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub bind: SocketAddr,
    /// 允许的 Host 头与 Origin(回环 + 显式开发来源;无通配 CORS)。
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub profile: ExecutionProfile,
}

impl ServeConfig {
    pub fn for_bind(bind: SocketAddr, profile: ExecutionProfile) -> Self {
        let port = bind.port();
        let mut allowed_hosts = vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        let mut allowed_origins = vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ];
        if bind.ip().is_loopback() {
            // 开发期 Vite 代理:固定回环来源,发行版不含(quickstart §3)
            allowed_hosts.push(format!("127.0.0.1:{}", 5173));
            allowed_hosts.push(format!("localhost:{}", 5173));
            allowed_origins.push(format!("http://127.0.0.1:{}", 5173));
        }
        Self {
            bind,
            allowed_hosts,
            allowed_origins,
            profile,
        }
    }
}

/// 匿名预会话 CSRF:短寿命内存表;初始化/登录必须携带(http-api §1)。
pub struct AnonymousCsrf {
    entries: Mutex<HashMap<String, Instant>>,
}

impl AnonymousCsrf {
    fn ttl() -> Duration {
        Duration::from_secs(30 * 60)
    }

    pub fn mint(&self) -> String {
        let token = crate::domain::ids::new_request_key();
        let hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        let mut entries = self.entries.lock().unwrap();
        let now = Instant::now();
        entries.retain(|_, exp| *exp > now);
        entries.insert(hash, now + Self::ttl());
        token
    }

    /// 校验并消费。
    /// 校验(不消费):表单校验失败后的重试、页面刷新前的重复提交
    /// 不应令用户陷入"CSRF 失败→必须手动刷新"的死角;令牌仍受 TTL 与
    /// 服务端哈希绑定保护,登录成功后预会话自然作废(会话接管)。
    pub fn consume(&self, token: &str) -> bool {
        let hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        let entries = self.entries.lock().unwrap();
        match entries.get(&hash) {
            Some(exp) => *exp > Instant::now(),
            None => false,
        }
    }
}

/// 登录限流:按对端地址计数,窗口 60 秒最多 5 次"凭证失败"。
#[derive(Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LoginLimiter {
    const WINDOW: Duration = Duration::from_secs(60);
    const MAX: usize = 5;

    /// 是否已被限流(不记录本次)。
    pub fn is_limited(&self, peer: &str) -> bool {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();
        let q = map.entry(peer.to_string()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) > Self::WINDOW {
                q.pop_front();
            } else {
                break;
            }
        }
        q.len() >= Self::MAX
    }

    /// 记录一次凭证失败。
    pub fn record_failure(&self, peer: &str) {
        let mut map = self.attempts.lock().unwrap();
        let q = map.entry(peer.to_string()).or_default();
        while let Some(front) = q.front() {
            if Instant::now().duration_since(*front) > Self::WINDOW {
                q.pop_front();
            } else {
                break;
            }
        }
        q.push_back(Instant::now());
    }
}

pub struct AppStateInner {
    pub rule_service: crate::application::catalog::rules::RuleService,
    pub auth: AdminAuth,
    pub jobs: JobService,
    pub idempotency: IdempotencyService,
    pub qr_flows: crate::application::accounts::qr::QrFlowService,
    pub account_control: crate::application::accounts::control::AccountControlService,
    pub db: crate::adapters::sqlite::db::DbThread,
    pub config: ServeConfig,
    pub key: DataKey,
    pub manual: Option<std::sync::Arc<dyn crate::application::manual::actions::ManualOps>>,
    /// live 构建的真实扫码驱动;mock/无协议构建为 None(端点明确报不支持)
    pub authorization:
        Option<std::sync::Arc<dyn crate::application::accounts::authorize::QrDriver>>,
    /// live 构建的商品同步驱动(真实平台在售列表);None 时端点明确报不支持
    pub item_sync: Option<std::sync::Arc<dyn crate::application::ports::platform::ItemSyncDriver>>,
    /// 003 安全验证处置服务;None(无浏览器/mock)时端点如实报不支持
    pub verification:
        Option<std::sync::Arc<crate::application::verification::service::VerificationService>>,
    pub anonymous_csrf: AnonymousCsrf,
    pub login_limiter: LoginLimiter,
    pub stopping: AtomicBool,
}

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<AppStateInner>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: AdminAuth,
        jobs: JobService,
        idempotency: IdempotencyService,
        qr_flows: crate::application::accounts::qr::QrFlowService,
        account_control: crate::application::accounts::control::AccountControlService,
        db: crate::adapters::sqlite::db::DbThread,
        rule_service: crate::application::catalog::rules::RuleService,
        manual: Option<std::sync::Arc<dyn crate::application::manual::actions::ManualOps>>,
        authorization: Option<
            std::sync::Arc<dyn crate::application::accounts::authorize::QrDriver>,
        >,
        item_sync: Option<std::sync::Arc<dyn crate::application::ports::platform::ItemSyncDriver>>,
        verification: Option<
            std::sync::Arc<crate::application::verification::service::VerificationService>,
        >,
        config: ServeConfig,
        key: DataKey,
    ) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                auth,
                jobs,
                idempotency,
                qr_flows,
                rule_service,
                account_control,
                manual,
                authorization,
                item_sync,
                verification,
                db,
                config,
                key,
                anonymous_csrf: AnonymousCsrf {
                    entries: Mutex::new(HashMap::new()),
                },
                login_limiter: LoginLimiter::default(),
                stopping: AtomicBool::new(false),
            }),
        }
    }

    pub fn stopping(&self) -> bool {
        self.inner
            .stopping
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub type SharedState = axum::extract::State<AppState>;
