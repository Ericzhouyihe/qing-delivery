//! 浏览器实例管理(003/T008/D7):单实例复用、并发槽(默认 1)、空闲回收、
//! 退出清理、泵结束自动清缓存(崩溃重建计入上层会话重试)。
//! user-data-dir 启动前锁检测:被占如实报错,不删目录(边界条款)。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::Browser;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::{BrowserConfig, Page};
use futures_util::StreamExt;
use tokio::sync::{Mutex, Semaphore};

#[derive(Debug, thiserror::Error)]
pub enum BrowserUnavailable {
    #[error("未找到兼容的 Edge/Chrome 浏览器;请安装后重试官方验证")]
    NoBrowser,
    #[error("浏览器用户数据目录被占用:{0};请确认无残留浏览器进程后重试")]
    ProfileLocked(PathBuf),
    #[error("浏览器启动失败:{0}")]
    Launch(String),
    #[error("浏览器队列超时(并发上限占用中)")]
    QueueTimeout,
    #[error("{0}")]
    Io(String),
}

/// 常见安装路径(不读用户日常 profile)。
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(lp) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(Path::new(&lp).join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(Path::new(&lp).join(r"Google\Chrome\Application\chrome.exe"));
    }
    candidates.push(PathBuf::from(
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ));
    candidates.push(PathBuf::from(
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ));
    candidates.push(PathBuf::from(
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    ));
    candidates.push(PathBuf::from(
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ));
    candidates
}

/// 探测本机浏览器(003/T007)。
pub fn detect_browser() -> Result<PathBuf, BrowserUnavailable> {
    candidate_paths()
        .into_iter()
        .find(|p| p.is_file())
        .ok_or(BrowserUnavailable::NoBrowser)
}

/// 策略(规格内建默认;SC-303/307)。
#[derive(Clone, Debug)]
pub struct BrowserInstancePolicy {
    pub max_concurrent: usize,   // 默认 1
    pub idle_reap: Duration,     // 默认 5 分钟
    pub queue_timeout: Duration, // 默认 60s
    pub headless: bool,          // 默认 false(有头,滑块通过率优先)
}

impl Default for BrowserInstancePolicy {
    fn default() -> Self {
        Self {
            max_concurrent: 1,
            idle_reap: Duration::from_secs(5 * 60),
            queue_timeout: Duration::from_secs(60),
            headless: false,
        }
    }
}

pub struct BrowserManager {
    exe: PathBuf,
    policy: BrowserInstancePolicy,
    permits: Arc<Semaphore>,
    browser: Mutex<Option<Arc<Browser>>>,
    user_data_dir: PathBuf,
}

/// 持有页面期间的会话句柄:drop 时释放并发槽并触发空闲回收检查。
pub struct BrowserSession {
    pub page: Page,
    _permit: tokio::sync::OwnedSemaphorePermit,
    manager: Arc<BrowserManager>,
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let manager = self.manager.clone();
        tokio::spawn(async move {
            // 会话结束后启动空闲回收计时(实例在时限内无新会话即关闭)
            tokio::time::sleep(manager.policy.idle_reap).await;
            if manager.permits.available_permits() >= manager.policy.max_concurrent {
                manager.close_all().await;
            }
        });
    }
}

impl BrowserManager {
    pub fn new(data_dir: &Path, policy: BrowserInstancePolicy) -> Result<Self, BrowserUnavailable> {
        let exe = detect_browser()?;
        let user_data_dir = data_dir.join("browser-profile");
        if user_data_dir.exists() {
            if user_data_dir.join("SingletonLock").exists() {
                return Err(BrowserUnavailable::ProfileLocked(user_data_dir));
            }
        } else {
            std::fs::create_dir_all(&user_data_dir)
                .map_err(|e| BrowserUnavailable::Io(format!("创建浏览器数据目录失败:{e}")))?;
        }
        Ok(Self {
            exe,
            permits: Arc::new(Semaphore::new(policy.max_concurrent.max(1))),
            policy,
            browser: Mutex::new(None),
            user_data_dir,
        })
    }

    pub fn policy(&self) -> &BrowserInstancePolicy {
        &self.policy
    }

    /// 打开会话(实例按需创建/复用;队列超时如实报错,风控风暴转人工)。
    pub async fn session(self: &Arc<Self>) -> Result<BrowserSession, BrowserUnavailable> {
        // 队列超时:先非阻塞获取,不行再限时等待
        let permit = match Arc::clone(&self.permits).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => match tokio::time::timeout(
                self.policy.queue_timeout,
                Arc::clone(&self.permits).acquire_owned(),
            )
            .await
            {
                Ok(p) => p.map_err(|_| BrowserUnavailable::Io("信号量关闭".into()))?,
                Err(_) => return Err(BrowserUnavailable::QueueTimeout),
            },
        };
        let browser = {
            let mut guard = self.browser.lock().await;
            match guard.clone() {
                Some(b) => b,
                None => {
                    let mut cfg = BrowserConfig::builder()
                        .chrome_executable(&self.exe)
                        .user_data_dir(&self.user_data_dir)
                        .no_sandbox()
                        .viewport(Viewport::default());
                    if self.policy.headless {
                        cfg = cfg.new_headless_mode();
                    }
                    let cfg = cfg
                        .build()
                        .map_err(|e| BrowserUnavailable::Launch(e.to_string()))?;
                    let (b, mut handler) = Browser::launch(cfg)
                        .await
                        .map_err(|e| BrowserUnavailable::Launch(e.to_string()))?;
                    let manager = self.clone();
                    tokio::spawn(async move {
                        while handler.next().await.is_some() {
                            // 保持泵存活;实例退出(含崩溃)时循环结束
                        }
                        // 泵结束 = 实例退出:清缓存,下次请求重建
                        *manager.browser.lock().await = None;
                    });
                    let b = Arc::new(b);
                    *guard = Some(b.clone());
                    b
                }
            }
        };
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserUnavailable::Launch(format!("打开页面失败:{e}")))?;
        Ok(BrowserSession {
            page,
            _permit: permit,
            manager: self.clone(),
        })
    }

    /// 关闭并清空缓存实例(空闲回收/退出清理调用;SC-303)。
    pub async fn close_all(&self) {
        let b = self.browser.lock().await.take();
        if let Some(b) = b {
            // close 需要独占所有权;仍有引用时丢弃缓存(连接断开即进程退出)
            match Arc::try_unwrap(b) {
                Ok(mut owned) => {
                    let _ = owned.close().await;
                    let _ = owned.wait().await;
                }
                Err(_) => {
                    tracing::info!("浏览器实例仍有页面引用;连接断开后进程退出");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 缺浏览器或锁占用时构造失败且不删目录() {
        let dir = tempfile::tempdir().unwrap();
        // 机器可能有浏览器:两种失败路径分别验证
        let profile = dir.path().join("browser-profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("SingletonLock"), b"x").unwrap();
        let marker = profile.join("keep.txt");
        std::fs::write(&marker, b"keep").unwrap();
        match BrowserManager::new(dir.path(), BrowserInstancePolicy::default()) {
            Err(BrowserUnavailable::ProfileLocked(_)) => {}
            Err(BrowserUnavailable::NoBrowser) => {}
            Err(_) => panic!("应报告不可用"),
            Ok(_) => panic!("不应构造成功"),
        }
        assert!(marker.exists(), "不得删除用户数据目录");
    }

    #[test]
    fn detect_两种合法结果() {
        match detect_browser() {
            Ok(path) => assert!(path.is_file()),
            Err(e) => assert!(matches!(e, BrowserUnavailable::NoBrowser)),
        }
    }
}
