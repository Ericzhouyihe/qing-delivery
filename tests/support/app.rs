//! 构建用于契约测试的完整应用(真实 handler + 临时数据目录)。
//! 不以源码字符串匹配替代行为测试(plan.md 验证要求)。

use std::net::SocketAddr;
use std::sync::OnceLock;

use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys;
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::auth::AdminAuth;
use qing_delivery::application::idempotency::IdempotencyService;
use qing_delivery::application::jobs::JobService;
use qing_delivery::transport::routes;
use qing_delivery::transport::state::{AppState, ExecutionProfile, ServeConfig};

/// 每个测试独占一个临时数据目录;持有锁防止清理早于 Drop。
pub struct TestApp {
    pub router: axum::Router,
    /// 002 起 contract 测试可经此直接播种数据(真实 DB 线程,与路由同一实例)。
    pub db: DbThread,
    /// 007 US7:聊天用例句柄(摄取买家消息驱动 AI 分流测试;与路由同 DB)。
    pub chat: std::sync::Arc<dyn qing_delivery::application::chat::ChatOps>,
    _dir: tempfile::TempDir,
    _lock: DirLock,
}

static TRACING: OnceLock<()> = OnceLock::new();

pub async fn spawn_app() -> TestApp {
    TRACING.get_or_init(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
            .try_init();
    });
    let dir = tempfile::tempdir().expect("临时目录");
    let data_dir = DataDir::resolve(Some(dir.path())).expect("数据目录");
    let lock = DirLock::acquire(&data_dir.root).expect("目录锁");
    let key = keys::load_or_create(&data_dir.root).expect("数据密钥");
    let db = DbThread::spawn(&data_dir.join("test.db")).expect("DB 线程");
    let migration_dir = data_dir.root.clone();
    db.call(move |conn| migrations::apply(conn, &migration_dir))
        .await
        .expect("迁移提交")
        .expect("迁移应用");
    let bind: SocketAddr = "127.0.0.1:59189".parse().unwrap();
    // 007 US4:契约测试的聊天端点走 FakeAdapter(mock 能力:chat_send_image=true)
    // 007 US7:AI 分支注入 HttpAiProvider(系统设置驱动;未配 key 等价 NoAi 直落默认)
    let chat: std::sync::Arc<dyn qing_delivery::application::chat::ChatOps> =
        std::sync::Arc::new(
            qing_delivery::application::chat::ChatService::new(
                db.clone(),
                qing_delivery::adapters::mock::fake::FakeAdapter::new(),
            )
            .with_ai(std::sync::Arc::new(
                qing_delivery::application::replies::HttpAiProvider::new(
                    db.clone(),
                    qing_delivery::application::settings_sys::SettingsService::new(
                        db.clone(),
                        qing_delivery::adapters::windows::keys::DataKey {
                            key_id: key.key_id.clone(),
                            key: key.key,
                        },
                    ),
                ),
            ))
            .with_storage(
                data_dir.root.join("uploads"),
                qing_delivery::application::ports::platform::CapabilitySet::MOCK,
            ),
        );
    let state = AppState::new(
        AdminAuth::new(db.clone()),
        JobService::new(db.clone()),
        IdempotencyService::new(db.clone()),
        qing_delivery::application::accounts::qr::QrFlowService::new(db.clone()),
        qing_delivery::application::accounts::control::AccountControlService::new(db.clone()),
        db.clone(),
        qing_delivery::application::catalog::rules::RuleService::new(
            db.clone(),
            qing_delivery::adapters::windows::keys::DataKey {
                key_id: key.key_id.clone(),
                key: key.key,
            },
        ),
        None,
        None,
        None,
        None,
        Some(chat.clone()),
        None,
        ServeConfig::for_bind(bind, ExecutionProfile::Live),
        key,
    );
    TestApp {
        router: routes::router(state),
        db,
        chat,
        _dir: dir,
        _lock: lock,
    }
}
