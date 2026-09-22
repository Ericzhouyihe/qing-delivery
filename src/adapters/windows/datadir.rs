//! 数据目录:解析/规范化 --data-dir,拒绝网络存储,限制当前用户访问。
//! 数据库文件不得位于网络共享目录(research R2;cli.md)。

use std::io::Write;
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::GetDriveTypeW;

/// GetDriveTypeW 的网络驱动器返回值(Win32 DRIVE_REMOTE)。
const DRIVE_REMOTE: u32 = 4;

/// 去掉 canonicalize 产生的 verbatim 前缀;\\?\UNC\ 还原为普通 UNC 路径。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest.to_string())
    } else {
        p
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DataDirError {
    #[error("数据目录不能位于网络共享(UNC)路径:{0}")]
    UncPath(PathBuf),
    #[error("数据目录所在驱动器 {0}: 是网络映射盘,不支持")]
    NetworkDrive(String),
    #[error("无法确定默认数据目录(缺少 LOCALAPPDATA 环境变量)")]
    NoLocalAppData,
    #[error("无法限制数据目录权限:{0}")]
    Acl(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct DataDir {
    /// 规范化(小写盘符、解析连接点)后的绝对路径;锁与身份判断都以它为准。
    pub root: PathBuf,
}

impl DataDir {
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, DataDirError> {
        let base = match explicit {
            Some(p) => p.to_path_buf(),
            None => {
                let la = std::env::var_os("LOCALAPPDATA").ok_or(DataDirError::NoLocalAppData)?;
                Path::new(&la).join("QingDelivery").join("data")
            }
        };
        reject_unc(&base)?;
        std::fs::create_dir_all(&base)?;
        let root = strip_verbatim(std::fs::canonicalize(&base)?);
        reject_unc(&root)?;
        reject_network_drive(&root)?;
        restrict_acl_current_user(&root)?;
        Ok(Self { root })
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn reject_unc(p: &Path) -> Result<(), DataDirError> {
    let s = p.to_string_lossy();
    if s.starts_with("\\\\") || s.starts_with("//") {
        return Err(DataDirError::UncPath(p.to_path_buf()));
    }
    Ok(())
}

fn reject_network_drive(p: &Path) -> Result<(), DataDirError> {
    let Some(root) = p.ancestors().last() else {
        return Ok(());
    };
    let root_str = root.to_string_lossy().to_string();
    // 规范化盘符形式(如 "C:"→"C:\",统一大写)
    let drive = if root_str.len() == 2 && root_str.as_bytes()[1] == b':' {
        let mut s = root_str.to_ascii_uppercase();
        s.push('\\');
        s
    } else {
        root_str
    };
    let wide: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
    let drive_type = unsafe { GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) };
    if drive_type == DRIVE_REMOTE {
        return Err(DataDirError::NetworkDrive(drive));
    }
    Ok(())
}

/// 限制目录 ACL 为当前用户与 SYSTEM(满权限,继承断开)。
/// 使用受控 icacls 子进程实现;调用前后不改变业务数据。
pub fn restrict_acl_current_user(dir: &Path) -> Result<(), DataDirError> {
    let user = std::env::var("USERNAME").map_err(|e| DataDirError::Acl(e.to_string()))?;
    let output = std::process::Command::new("icacls")
        .arg(dir)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:(OI)(CI)F"))
        .arg("/grant:r")
        .arg("SYSTEM:(OI)(CI)F")
        .output()
        .map_err(|e| DataDirError::Acl(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DataDirError::Acl(stderr.trim().to_string()));
    }
    Ok(())
}

/// 原子写文件:同目录临时文件 + rename,避免半写状态被当作完整数据。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    if tmp.exists() {
        std::fs::remove_file(&tmp)?;
    }
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
