//! 敏感列加密信封:aes-gcm,随机 nonce,AAD 绑定用途、实体 ID 与内容版本。
//! 密钥来源(DPAPI 包装的数据密钥)由 adapters/windows 提供;本模块只定义
//! 信封格式与加解密纯函数。密文格式 version 1。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

pub const ENVELOPE_FORMAT_VERSION: u16 = 1;
pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aad {
    pub purpose: String,
    pub entity_id: String,
    pub content_version: Option<i64>,
}

impl Aad {
    fn bytes(&self) -> Vec<u8> {
        format!(
            "qd-aad\u{1}{}\u{1}{}\u{1}{}",
            self.purpose,
            self.entity_id,
            self.content_version.unwrap_or(0)
        )
        .into_bytes()
    }
}

/// 持久化信封:nonce、密文(含 tag)、key_id、格式版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
    pub key_id: String,
    pub format_version: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("解密失败:密文被篡改或密钥不匹配")]
    OpenFailed,
    #[error("不支持的信封格式版本:{0}")]
    UnsupportedVersion(u16),
}

pub fn seal(key: &[u8; KEY_LEN], key_id: &str, aad: &Aad, plaintext: &[u8]) -> Envelope {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: &aad.bytes(),
            },
        )
        .expect("AES-GCM 加密在有效输入下不会失败");
    Envelope {
        nonce: nonce_bytes,
        ciphertext,
        key_id: key_id.to_string(),
        format_version: ENVELOPE_FORMAT_VERSION,
    }
}

pub fn open(key: &[u8; KEY_LEN], aad: &Aad, envelope: &Envelope) -> Result<Vec<u8>, CryptoError> {
    if envelope.format_version != ENVELOPE_FORMAT_VERSION {
        return Err(CryptoError::UnsupportedVersion(envelope.format_version));
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(&envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &aad.bytes(),
            },
        )
        .map_err(|_| CryptoError::OpenFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; KEY_LEN] {
        [7u8; KEY_LEN]
    }

    #[test]
    fn roundtrip_and_aad_binding() {
        let aad = Aad {
            purpose: "rule_content".into(),
            entity_id: "r1".into(),
            content_version: Some(3),
        };
        let env = seal(&key(), "k1", &aad, b"secret-text");
        assert_eq!(open(&key(), &aad, &env).unwrap(), b"secret-text".to_vec());

        let wrong = Aad {
            purpose: "other".into(),
            ..aad.clone()
        };
        assert!(
            open(&key(), &wrong, &env).is_err(),
            "AAD 不匹配必须解密失败"
        );
    }
}
