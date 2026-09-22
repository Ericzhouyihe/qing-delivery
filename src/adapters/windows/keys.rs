//! 数据密钥:随机 32 字节密钥经 DPAPI 当前用户包装后落盘。
//! 文件格式:`<key_id>\n<protected blob>`;key_id 非机密,blob 只有同机同用户可解。
//! 已有密钥缺失/解密失败时停止,不能新建密钥覆盖旧内容(cli.md)。

use std::path::Path;

use super::dpapi;
use crate::domain::crypto::KEY_LEN;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("数据密钥无法解密(DPAPI 失败或跨用户);拒绝覆盖旧密钥")]
    DecryptFailed,
    #[error("密钥文件损坏")]
    Corrupted,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct DataKey {
    pub key_id: String,
    pub key: [u8; KEY_LEN],
}

const KEY_FILE: &str = "data-key.bin";

pub fn load_or_create(dir: &Path) -> Result<DataKey, KeyError> {
    let path = dir.join(KEY_FILE);
    if path.exists() {
        let raw = std::fs::read(&path)?;
        let (id, blob) = split_key_file(&raw).ok_or(KeyError::Corrupted)?;
        let plain = dpapi::unprotect_current_user(blob).map_err(|_| KeyError::DecryptFailed)?;
        let key: [u8; KEY_LEN] = plain
            .as_slice()
            .try_into()
            .map_err(|_| KeyError::Corrupted)?;
        return Ok(DataKey { key_id: id, key });
    }
    let mut key = [0u8; KEY_LEN];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut key);
    let key_id = format!("dk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let blob = dpapi::protect_current_user(&key)
        .map_err(|e| KeyError::Io(std::io::Error::other(e.to_string())))?;
    let mut content = Vec::with_capacity(key_id.len() + 1 + blob.len());
    content.extend_from_slice(key_id.as_bytes());
    content.push(b'\n');
    content.extend_from_slice(&blob);
    super::datadir::atomic_write(&path, &content)?;
    Ok(DataKey { key_id, key })
}

fn split_key_file(raw: &[u8]) -> Option<(String, &[u8])> {
    let nl = raw.iter().position(|&b| b == b'\n')?;
    let id = std::str::from_utf8(&raw[..nl]).ok()?.to_string();
    if id.is_empty() {
        return None;
    }
    Some((id, &raw[nl + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path()).unwrap();
        let second = load_or_create(dir.path()).unwrap();
        assert_eq!(first.key_id, second.key_id, "重复加载不得换密钥");
        assert_eq!(first.key, second.key);
    }
}
