//! 浏览器管理器(T051):进程/profile/CDP 生命周期所有权、数量有界、
//! 账号隔离;退出/取消只清理自建进程。
//! 真实浏览器启动验证属 SC-009 实证矩阵;确定性测试只覆盖检测与不可用路径。

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BrowserUnavailable {
    #[error("未找到兼容的 Edge/Chrome 浏览器;请安装后重试官方验证")]
    NoBrowser,
    #[error("浏览器启动失败:{0}")]
    Launch(String),
}

/// 常见安装路径(当前用户/Program Files 优先,不读日常 profile 数据)。
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

pub struct BrowserManager {
    /// 每个账号一个专用 profile 目录,位于数据目录 browser/<account_id> 下。
    data_dir: PathBuf,
    max_sessions: usize,
    active: std::sync::Mutex<Vec<String>>,
}

impl BrowserManager {
    pub fn new(data_dir: PathBuf, max_sessions: usize) -> Self {
        Self {
            data_dir,
            max_sessions,
            active: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 检测可用浏览器;返回可执行文件路径。
    pub fn detect() -> Result<PathBuf, BrowserUnavailable> {
        candidate_paths()
            .into_iter()
            .find(|p| p.exists())
            .ok_or(BrowserUnavailable::NoBrowser)
    }

    /// 为账号准备专用受限 profile 目录(不指向用户日常 profile)。
    pub fn profile_dir(&self, account_id: &str) -> PathBuf {
        self.data_dir.join("browser").join(account_id)
    }

    /// 预约一个会话槽位;超出上限拒绝(数量有界)。
    pub fn reserve(&self, account_id: &str) -> Result<(), BrowserUnavailable> {
        let mut active = self.active.lock().unwrap();
        if active.iter().any(|a| a == account_id) {
            return Ok(()); // 同账号复用
        }
        if active.len() >= self.max_sessions {
            return Err(BrowserUnavailable::Launch("浏览器会话数已达上限".into()));
        }
        active.push(account_id.to_string());
        Ok(())
    }

    /// 释放会话槽位;只清理本管理器登记的账号,不影响用户自有浏览器进程。
    pub fn release(&self, account_id: &str) {
        self.active.lock().unwrap().retain(|a| a != account_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_sessions_and_account_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserManager::new(dir.path().to_path_buf(), 2);
        mgr.reserve("acct-1").unwrap();
        mgr.reserve("acct-2").unwrap();
        assert!(matches!(
            mgr.reserve("acct-3"),
            Err(BrowserUnavailable::Launch(_))
        ));
        // 同账号复用不受上限影响
        mgr.reserve("acct-1").unwrap();
        mgr.release("acct-1");
        mgr.reserve("acct-3").unwrap();

        let profile = mgr.profile_dir("acct-1");
        assert!(
            profile.ends_with("browser\\acct-1"),
            "专用 profile 与账号隔离"
        );
        assert!(profile.starts_with(dir.path()), "profile 位于数据目录内");
    }

    #[test]
    fn detect_reports_missing_browser_clearly() {
        // 机器上可能装有浏览器;两类结果都合法,只断言错误类型可区分
        match BrowserManager::detect() {
            Ok(path) => assert!(path.exists()),
            Err(e) => assert!(matches!(e, BrowserUnavailable::NoBrowser)),
        }
    }
}
