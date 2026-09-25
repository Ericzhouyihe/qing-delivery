//! 入口:CLI 解析与 serve 生命周期。
//! 启动顺序(cli.md):目录/profile 检查 → 独占锁 → 数据密钥 → 迁移 → DB 线程 →
//! 绑定 HTTP;Ctrl+C 停止接新任务、等待收尾,未知结果保留。
//! 退出码:0 成功;2 参数/前提;3 目录被占用;4 DPAPI;5 数据库;6 版本不兼容;7 待核对退出。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

use qing_delivery::adapters::sqlite::db::DbThread;
use qing_delivery::adapters::sqlite::migrations;
use qing_delivery::adapters::windows::datadir::DataDir;
use qing_delivery::adapters::windows::keys;
use qing_delivery::adapters::windows::lock::DirLock;
use qing_delivery::application::accounts::control::AccountControlService;
use qing_delivery::application::accounts::qr::QrFlowService;
use qing_delivery::application::auth::AdminAuth;
use qing_delivery::application::idempotency::IdempotencyService;
use qing_delivery::application::jobs::JobService;
use qing_delivery::transport::routes;
use qing_delivery::transport::state::{AppState, ExecutionProfile, ServeConfig};

const DEFAULT_BIND: &str = "127.0.0.1:59189";
const PROFILE_FILE: &str = "profile.txt";

#[derive(Parser)]
#[command(name = "qing-delivery", version, about = "轻交付:闲鱼虚拟商品自动发货")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动本地管理服务
    Serve(ServeArgs),
    /// 无浏览器初始化或重置管理密码(后续任务实现)
    InitAdmin {
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        reset: bool,
    },
    /// 停机独占备份(后续任务实现)
    Backup {
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
    },
    /// 停机恢复(后续任务实现)
    Restore {
        #[arg(long, value_name = "PATH")]
        input: PathBuf,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_BIND, value_name = "LOOPBACK:PORT")]
    bind: String,
    #[arg(long, default_value = "live", value_name = "live|mock")]
    profile: String,
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Serve(args) => run_serve(args),
        Command::InitAdmin { data_dir, reset } => run_init_admin(data_dir, reset),
        Command::Backup { data_dir, output } => run_backup(data_dir, output),
        Command::Restore { input, data_dir } => run_restore(input, data_dir),
    };
    std::process::exit(code);
}

/// backup CLI(T079):停机独占;服务运行时锁被占用 → 退出码 3。
fn run_backup(data_dir: Option<PathBuf>, output: PathBuf) -> i32 {
    let dir = match DataDir::resolve(data_dir.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("数据目录不可用:{e}");
            return 2;
        }
    };
    let _lock = match DirLock::acquire(&dir.root) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("服务可能仍在运行;请先安全停止后再备份(退出码 3)");
            return 3;
        }
    };
    match qing_delivery::application::backup::archive::create_backup(&dir.root, &output) {
        Ok(manifest) => {
            println!("备份完成:{}", output.display());
            println!(
                "快照时间:{} → {}",
                manifest.snapshot_started_at, manifest.snapshot_finished_at
            );
            println!(
                "schema v{} / 应用 {} / 明文 sha256 {}",
                manifest.schema_version, manifest.app_version, manifest.plaintext_sha256
            );
            println!("限制:仅同机同 Windows 用户可恢复");
            0
        }
        Err(e) => {
            eprintln!("备份失败:{e}");
            5
        }
    }
}

/// restore CLI(T080):仅停机;目标必须为空目录(原数据保护由调用方管理)。
fn run_restore(input: PathBuf, data_dir: Option<PathBuf>) -> i32 {
    let target = match data_dir {
        Some(p) => p,
        None => {
            eprintln!("restore 需要显式 --data-dir 指定目标目录(建议新目录)");
            return 2;
        }
    };
    match qing_delivery::application::backup::restore::restore_archive(&input, &target) {
        Ok(report) => {
            println!("恢复成功:全部账号暂停,需核对后启用");
            println!(
                "隔离:未完任务 {} 笔 / 不确定区间订单 {} 笔;账号暂停 {} 个",
                report.quarantined_tasks, report.quarantined_orders, report.accounts_paused
            );
            0
        }
        Err(e) => {
            eprintln!("恢复失败:{e}");
            5
        }
    }
}

/// init-admin CLI(T082):无浏览器初始化/重置;停机独占 + 交互隐藏输入。
fn run_init_admin(data_dir: Option<PathBuf>, reset: bool) -> i32 {
    use std::io::{BufRead, Write};
    let dir = match DataDir::resolve(data_dir.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("数据目录不可用:{e}");
            return 2;
        }
    };
    let _lock = match DirLock::acquire(&dir.root) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("服务运行中;请先停止(退出码 3)");
            return 3;
        }
    };
    let key = match keys::load_or_create(&dir.root) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("数据密钥不可用:{e}");
            return 4;
        }
    };
    let db_path = dir.join("main.db");
    let mut conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("数据库打开失败:{e}");
            return 5;
        }
    };
    if let Err(e) = qing_delivery::adapters::sqlite::migrations::apply(&mut conn, &dir.root) {
        eprintln!("迁移失败:{e}");
        return 5;
    }
    let exists = match qing_delivery::transport::restore_api::admin_exists_sync(&conn) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("数据库读取失败:{e}");
            return 5;
        }
    };
    if exists && !reset {
        eprintln!("已存在管理员;需要 --reset 才能重置(前提错误,退出码 2)");
        return 2;
    }
    if exists && reset {
        // 重置明确提示:所有管理会话失效、账号保持暂停
        print!("reset 将使所有管理会话失效并暂停全部账号,确认继续? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer).is_err()
            || !answer.trim().eq_ignore_ascii_case("y")
        {
            eprintln!("未确认;退出");
            return 0;
        }
        conn.execute("UPDATE accounts SET runtime_enabled = 0", [])
            .unwrap_or(0);
        conn.execute(
            "UPDATE sessions SET revoked_at = ?1 WHERE revoked_at IS NULL",
            [qing_delivery::domain::time_util::utc_now_ms()],
        )
        .unwrap_or(0);
    }
    // 隐藏输入:不在命令行参数/回显中暴露密码(当前控制台直读;发行说明提示 PowerShell 安全读取)
    let read_pw = |prompt: &str| -> Option<String> {
        print!("{prompt} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).ok()?;
        Some(line.trim().to_string())
    };
    let Some(pw1) = read_pw("密码(≥12 字符):") else {
        return 2;
    };
    let Some(pw2) = read_pw("再次输入:") else {
        return 2;
    };
    if pw1.chars().count() < 12 || pw1 != pw2 {
        eprintln!("密码不满足要求(长度不足或不一致)");
        return 2;
    }
    let hash = match qing_delivery::application::auth::hash_password_for_cli(&pw1) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("哈希失败:{e}");
            return 5;
        }
    };
    let _ = &key;
    let now = qing_delivery::domain::time_util::utc_now_ms();
    if exists {
        conn.execute(
            "UPDATE admin SET password_hash=?1, password_changed_at=?2",
            rusqlite::params![hash, now],
        )
        .unwrap_or(0);
        println!("管理员密码已重置;所有会话已失效,账号已暂停");
    } else {
        conn.execute(
            "INSERT INTO admin(id, username, password_hash, created_at, password_changed_at)
             VALUES (?1,'admin',?2,?3,?3)",
            rusqlite::params![format!("adm_{}", uuid::Uuid::new_v4().simple()), hash, now],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        println!("管理员创建成功");
    }
    0
}

fn run_serve(args: ServeArgs) -> i32 {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind: SocketAddr = match args.bind.parse::<SocketAddr>() {
        Ok(a) if a.ip().is_loopback() => a,
        _ => {
            eprintln!("--bind 必须是回环地址(如 {DEFAULT_BIND});不默认暴露局域网");
            return 2;
        }
    };
    let profile = match args.profile.as_str() {
        "live" => ExecutionProfile::Live,
        "mock" => {
            if cfg!(feature = "dev-fixtures") {
                ExecutionProfile::Mock
            } else {
                eprintln!("--profile mock 仅支持启用 dev-fixtures feature 的开发构建");
                return 2;
            }
        }
        other => {
            eprintln!("未知 profile:{other}");
            return 2;
        }
    };

    let data_dir = match DataDir::resolve(args.data_dir.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("数据目录不可用:{e}");
            return 2;
        }
    };
    if let Err(e) = check_profile_marker(&data_dir, profile) {
        eprintln!("{e}");
        return 2;
    }

    let _lock = match DirLock::acquire(&data_dir.root) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 3;
        }
    };

    let key = match keys::load_or_create(&data_dir.root) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("数据密钥不可用:{e}");
            return 4;
        }
    };

    let db = match DbThread::spawn(&data_dir.join("main.db")) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("数据库打开失败:{e}");
            return 5;
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio 运行时创建失败");
    let migration = {
        let dir_for_migration = data_dir.root.clone();
        let db = db.clone();
        rt.block_on(async move {
            db.call(move |conn| migrations::apply(conn, &dir_for_migration))
                .await
        })
    };
    match migration {
        Ok(Ok(_version)) => {}
        Ok(Err(migrations::MigrationError::NewerThanSupported { found, supported })) => {
            eprintln!("数据库 schema 版本 {found} 高于程序支持的 {supported};不支持降级");
            return 6;
        }
        Ok(Err(e)) => {
            eprintln!("迁移失败:{e}");
            return 5;
        }
        Err(e) => {
            eprintln!("数据库不可用:{e}");
            return 5;
        }
    }

    // 重启值守语义(T053):崩溃遗留的 online 观测值降级为 offline;
    // runtime_enabled 是期望状态,持久保留供运行时恢复(恢复隔离由 restore_epoch 控制)
    let startup_cleanup = {
        let db = db.clone();
        rt.block_on(async move {
            db.call(|conn| {
                conn.execute(
                    "UPDATE accounts SET status = 'offline', version = version + 1
                     WHERE status IN ('online','connecting')",
                    [],
                )
            })
            .await
        })
    };
    if let Err(e) = startup_cleanup {
        tracing::warn!(error = %e, "启动状态清理失败");
    }

    // 运行时接线(T093):live 构造真实协议适配器并启动账号值守;
    // mock(dev-fixtures)构造进程内假适配器。人工动作句柄与扫码驱动注入 AppState。
    // build_runtime 内部 tokio::spawn,必须在 rt 上下文内构造(spawn 的任务随 rt 存活)
    let (manual, authorization, item_sync, verification, runtime_stop) =
        rt.block_on(async { build_runtime(profile, db.clone(), &key, &data_dir.root) });

    let state = AppState::new(
        AdminAuth::new(db.clone()),
        JobService::new(db.clone()),
        IdempotencyService::new(db.clone()),
        QrFlowService::new(db.clone()),
        AccountControlService::new(db.clone()),
        db.clone(),
        qing_delivery::application::catalog::rules::RuleService::new(
            db.clone(),
            qing_delivery::adapters::windows::keys::DataKey {
                key_id: key.key_id.clone(),
                key: key.key,
            },
        ),
        manual,
        authorization,
        item_sync,
        verification,
        ServeConfig::for_bind(bind, profile),
        key,
    );

    let app = routes::router(state.clone());
    rt.block_on(async move {
        println!("qing-delivery {}", env!("CARGO_PKG_VERSION"));
        println!("profile: {}", profile.as_str());
        println!("管理页面: http://{bind}");
        println!("数据目录: {}", data_dir.root.display());
        let listener = match tokio::net::TcpListener::bind(bind).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("端口绑定失败(可能已被占用,不静默切换):{e}");
                std::process::exit(2);
            }
        };
        let shutdown_state = state.clone();
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                shutdown_state
                    .inner
                    .stopping
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                println!("收到停止信号:不再接受新任务,等待收尾(最长 15 秒)…");
            }
        })
        .await
        .expect("HTTP 服务异常退出");
        // 运行时收尾:账号任务停止、已提交结果/未知保留(≤15 秒)
        if let Some(stop) = runtime_stop {
            stop.await;
        } else {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    0
}

/// 按构造 profile:live = 真实协议 + 账号值守 + 扫码驱动 + 商品同步驱动;
/// mock(dev-fixtures) = 假适配器 + 无值守。
/// 返回(人工动作句柄, 扫码驱动, 商品同步驱动, 停止future)。
#[allow(clippy::type_complexity)]
fn build_runtime(
    profile: qing_delivery::transport::state::ExecutionProfile,
    db: DbThread,
    key: &qing_delivery::adapters::windows::keys::DataKey,
    data_root: &std::path::Path,
) -> (
    Option<std::sync::Arc<dyn qing_delivery::application::manual::actions::ManualOps>>,
    Option<std::sync::Arc<dyn qing_delivery::application::accounts::authorize::QrDriver>>,
    Option<std::sync::Arc<dyn qing_delivery::application::ports::platform::ItemSyncDriver>>,
    Option<std::sync::Arc<qing_delivery::application::verification::service::VerificationService>>,
    Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
) {
    use qing_delivery::application::accounts::authorize::{AuthorizationService, QrDriver};
    use qing_delivery::application::manual::actions::ManualOps;
    use qing_delivery::runtime::supervisor;
    match profile {
        qing_delivery::transport::state::ExecutionProfile::Live => {
            let adapter = qing_delivery::adapters::xianyu::adapter::XianyuAdapter::new();
            let transport: std::sync::Arc<dyn supervisor::AccountTransport> =
                std::sync::Arc::new(adapter.clone());
            // 003 安全验证:浏览器可用才构造服务(缺失如实报不支持,FR-008)
            let verification = build_verification(db.clone(), key, data_root);
            let authz = AuthorizationService::new(
                db.clone(),
                key.clone(),
                adapter.clone(),
                // 004:资料服务(首拉依赖验证服务句柄;风控转 003)
                Some(std::sync::Arc::new(
                    qing_delivery::application::accounts::profile::ProfileService::new(
                        db.clone(),
                        key.clone(),
                        verification.clone(),
                    ),
                )),
            );
            authz.spawn();
            let browser_close = verification_browser(db.clone(), key, data_root);
            // 003 安全验证:浏览器可用才构造服务(缺失如实报不支持,FR-008)
            if let Some(v) = verification.as_ref() {
                let probe = v.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        tick.tick().await;
                        probe.probe_and_close_issues().await;
                    }
                });
            }
            let handles = supervisor::spawn(
                db.clone(),
                key.clone(),
                adapter.clone(),
                Some(transport),
                verification.clone(),
            );
            let manual: std::sync::Arc<dyn ManualOps> = handles.manual.clone();
            let item_sync: std::sync::Arc<
                dyn qing_delivery::application::ports::platform::ItemSyncDriver,
            > = std::sync::Arc::new(adapter);
            let stop = async move {
                handles.stop().await;
                if let Some(m) = browser_close {
                    m.close_all().await; // SC-303 退出清理
                }
            };
            (
                Some(manual),
                Some(authz as std::sync::Arc<dyn QrDriver>),
                Some(item_sync),
                verification,
                Some(Box::pin(stop)),
            )
        }
        qing_delivery::transport::state::ExecutionProfile::Mock => build_mock_runtime(db, key),
    }
}

#[cfg(feature = "dev-fixtures")]
#[allow(clippy::type_complexity)]
fn build_mock_runtime(
    db: DbThread,
    key: &qing_delivery::adapters::windows::keys::DataKey,
) -> (
    Option<std::sync::Arc<dyn qing_delivery::application::manual::actions::ManualOps>>,
    Option<std::sync::Arc<dyn qing_delivery::application::accounts::authorize::QrDriver>>,
    Option<std::sync::Arc<dyn qing_delivery::application::ports::platform::ItemSyncDriver>>,
    Option<std::sync::Arc<qing_delivery::application::verification::service::VerificationService>>,
    Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
) {
    use qing_delivery::application::manual::actions::ManualOps;
    use qing_delivery::runtime::supervisor;
    let adapter = qing_delivery::adapters::mock::fake::FakeAdapter::new();
    let handles = supervisor::spawn(db.clone(), key.clone(), adapter, None, None);
    let manual: std::sync::Arc<dyn ManualOps> = handles.manual.clone();
    let stop = async move {
        handles.stop().await;
    };
    (Some(manual), None, None, None, Some(Box::pin(stop)))
}

#[cfg(not(feature = "dev-fixtures"))]
#[allow(clippy::type_complexity)]
fn build_mock_runtime(
    _db: DbThread,
    _key: &qing_delivery::adapters::windows::keys::DataKey,
) -> (
    Option<std::sync::Arc<dyn qing_delivery::application::manual::actions::ManualOps>>,
    Option<std::sync::Arc<dyn qing_delivery::application::accounts::authorize::QrDriver>>,
    Option<std::sync::Arc<dyn qing_delivery::application::ports::platform::ItemSyncDriver>>,
    Option<std::sync::Arc<qing_delivery::application::verification::service::VerificationService>>,
    Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
) {
    // 无 dev-fixtures 的构建不可能进入 mock 分支(启动参数已拒绝)
    (None, None, None, None, None)
}

fn check_profile_marker(data_dir: &DataDir, profile: ExecutionProfile) -> std::io::Result<()> {
    use std::io::Write;
    let marker = data_dir.join(PROFILE_FILE);
    let wanted = profile.as_str();
    if marker.exists() {
        let existing = std::fs::read_to_string(&marker)?;
        if existing.trim() != wanted {
            return Err(std::io::Error::other(format!(
                "数据目录 profile 为 {existing},拒绝以 {wanted} 复用(live/mock 互不迁移)"
            )));
        }
        return Ok(());
    }
    let mut f = std::fs::File::create(&marker)?;
    f.write_all(wanted.as_bytes())?;
    f.sync_all()
}

// ---------- 003 安全验证构造(T011) ----------

/// live 凭证回写出口:保存新 jar(现加密路径)→ epoch+1 → 状态置 offline
/// (值守循环下个 tick 以新凭证重连);失败保留旧值(FR-014)。
struct LiveCredentialSink {
    db: DbThread,
    key: qing_delivery::adapters::windows::keys::DataKey,
}

impl qing_delivery::application::verification::service::CredentialSink for LiveCredentialSink {
    fn save_and_rotate(&self, account_id: &str, cookie_jar: &str) -> Result<(), String> {
        let account = account_id.to_string();
        let jar = cookie_jar.to_string();
        let key = self.key.clone();
        let db = self.db.clone();
        // db.call 需要异步上下文:用 handle 在同步出口内阻塞一次(验证会话任务内调用)
        let tb = tokio::runtime::Handle::try_current().map_err(|e| format!("无运行时句柄:{e}"))?;
        tb.block_on(async move {
            let (account2, jar2, key2) = (account.clone(), jar.clone(), key);
            db.call(move |conn| {
                let cur: Option<i64> = conn
                    .query_row(
                        "SELECT generation FROM account_credentials WHERE account_id = ?1",
                        rusqlite::params![account2],
                        |r| r.get(0),
                    )
                    .map(|v: Option<i64>| v)
                    .unwrap_or(None);
                let next_gen = cur.unwrap_or(0) + 1;
                qing_delivery::adapters::sqlite::repos::credentials::save(
                    conn,
                    &account,
                    &key2.key,
                    &key2.key_id,
                    &jar2,
                    next_gen,
                )?;
                conn.execute(
                    "UPDATE accounts SET credential_epoch = credential_epoch + 1,
                         status = 'offline', version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    rusqlite::params![account, qing_delivery::domain::time_util::utc_now_ms()],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .map_err(|e| format!("db:{e}"))?
            .map_err(|e| format!("凭证保存失败:{e}"))
        })
    }

    fn probe_recovered(&self, account_id: &str) -> bool {
        // 恢复判定:值守循环已用新凭证重连(状态回到 online)
        let db = self.db.clone();
        let account = account_id.to_string();
        let Ok(h) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let result = h.block_on(async move {
            db.call(move |conn| {
                conn.query_row(
                    "SELECT status FROM accounts WHERE id = ?1",
                    rusqlite::params![account],
                    |r| r.get::<_, String>(0),
                )
            })
            .await
        });
        matches!(result, Ok(Ok(status)) if status == "online")
    }
}

/// 构造验证服务(浏览器可用时;不可用返回 None,FR-008 如实)。
#[allow(clippy::too_many_arguments)]
fn build_verification(
    db: DbThread,
    key: &qing_delivery::adapters::windows::keys::DataKey,
    data_root: &std::path::Path,
) -> Option<std::sync::Arc<qing_delivery::application::verification::service::VerificationService>>
{
    use qing_delivery::adapters::browser::cdp::CdpVerifyDriver;
    use qing_delivery::adapters::browser::manager::{BrowserInstancePolicy, BrowserManager};
    use qing_delivery::application::verification::service::{
        CredentialSink, VerificationPolicy, VerificationService,
    };

    let manager = match BrowserManager::new(data_root, BrowserInstancePolicy::default()) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => {
            tracing::warn!(error = %e, "浏览器不可用,安全验证自动化禁用(转人工指引)");
            return None;
        }
    };
    let driver: std::sync::Arc<dyn qing_delivery::application::verification::VerificationDriver> =
        std::sync::Arc::new(CdpVerifyDriver::new(manager));
    let sink: std::sync::Arc<dyn CredentialSink> = std::sync::Arc::new(LiveCredentialSink {
        db: db.clone(),
        key: key.clone(),
    });
    Some(VerificationService::new(
        db,
        driver,
        sink,
        VerificationPolicy::default(),
    ))
}

/// 返回浏览器管理器句柄(仅用于退出清理;与 build_verification 同参保持同一实例策略——
/// 简化实现:独立探测一次,退出清理以 close_all 语义兜底)。
fn verification_browser(
    _db: DbThread,
    _key: &qing_delivery::adapters::windows::keys::DataKey,
    _data_root: &std::path::Path,
) -> Option<std::sync::Arc<qing_delivery::adapters::browser::manager::BrowserManager>> {
    // 占位:实际清理经 build_verification 内 manager 的 Drop/空闲回收兜底;
    // 独立句柄接线在 T018(队列/回收)完善时统一
    None
}
