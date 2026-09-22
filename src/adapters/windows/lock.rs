//! 进程级独占目录锁:锁文件以零共享模式打开并持有到退出。
//! 不使用可遗留的 PID 文件代替(cli.md);第二实例得到明确"目录被占用"错误。

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, ERROR_SHARING_VIOLATION, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_ALWAYS,
};
use windows::core::PCWSTR;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("数据目录正被另一个实例使用(独占锁被占用)")]
    Busy,
    #[error("获取目录锁失败:{0}")]
    System(windows::core::Error),
}

pub struct DirLock {
    handle: HANDLE,
}

impl DirLock {
    /// 获取独占锁;Drop 时自动释放。路径应已规范化(datadir::resolve)。
    pub fn acquire(dir: &Path) -> Result<Self, LockError> {
        let path = dir.join(".qing-delivery.lock");
        let wide: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let handle = CreateFileW(
                PCWSTR(wide.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|e| {
                if e.code() == windows::core::HRESULT::from(ERROR_SHARING_VIOLATION) {
                    LockError::Busy
                } else {
                    LockError::System(e)
                }
            })?;
            Ok(Self { handle })
        }
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let lock = DirLock::acquire(dir.path()).expect("首次加锁应成功");
        assert!(matches!(DirLock::acquire(dir.path()), Err(LockError::Busy)));
        drop(lock);
        assert!(DirLock::acquire(dir.path()).is_ok(), "释放后可重新加锁");
    }
}
