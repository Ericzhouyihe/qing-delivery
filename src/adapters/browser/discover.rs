//! 系统浏览器探测(003/T007,D1):按序找 Chrome/Edge;缺失时返回带指引的原因。

/// 探测结果。
pub enum BrowserFound {
    Executable(String),
    Unavailable(String),
}

/// 标准安装位置(Windows;按优先序)。
pub fn candidate_paths() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        out.push(format!("{pf}\\Google\\Chrome\\Application\\chrome.exe"));
    }
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        out.push(format!("{pf86}\\Google\\Chrome\\Application\\chrome.exe"));
        out.push(format!("{pf86}\\Microsoft\\Edge\\Application\\msedge.exe"));
    }
    if let Ok(lad) = std::env::var("LOCALAPPDATA") {
        out.push(format!("{lad}\\Google\\Chrome\\Application\\chrome.exe"));
        out.push(format!("{lad}\\Microsoft\\Edge\\Application\\msedge.exe"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        out.push(format!("{pf}\\Microsoft\\Edge\\Application\\msedge.exe"));
    }
    out
}

/// 在给定候选路径中找第一个存在的可执行文件(测试可注入路径表)。
pub fn discover_in(candidates: &[String]) -> BrowserFound {
    for p in candidates {
        if std::path::Path::new(p).is_file() {
            return BrowserFound::Executable(p.clone());
        }
    }
    BrowserFound::Unavailable(
        "未检测到 Chrome/Edge,请安装任一浏览器后重试(安装后无需重启服务)".into(),
    )
}

/// 实际探测(候选 + PATH 上的 chrome/msedge)。
pub fn discover() -> BrowserFound {
    let mut candidates = candidate_paths();
    for name in ["chrome.exe", "msedge.exe"] {
        if let Some(dir) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&dir) {
                candidates.push(dir.join(name).to_string_lossy().into_owned());
            }
        }
    }
    discover_in(&candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 候选不存在时给出指引文案() {
        let found = discover_in(&["C:\\definitely\\missing\\chrome.exe".into()]);
        match found {
            BrowserFound::Unavailable(reason) => {
                assert!(reason.contains("Chrome"));
                assert!(reason.contains("安装"));
            }
            BrowserFound::Executable(_) => panic!("不应命中不存在的路径"),
        }
    }

    #[test]
    fn 命中第一个存在的路径() {
        let real = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let found = discover_in(&[
            "C:\\missing\\a.exe".into(),
            real,
            "C:\\missing\\b.exe".into(),
        ]);
        match found {
            BrowserFound::Executable(p) => assert!(std::path::Path::new(&p).is_file()),
            _ => panic!("应命中真实存在的可执行文件"),
        }
    }
}
