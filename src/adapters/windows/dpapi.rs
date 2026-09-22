//! DPAPI 当前用户数据保护:CryptProtectData/CryptUnprotectData 包装。
//! 仅限同机同用户解包;失败不得生成新密钥覆盖旧密钥(research R6)。

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};
use windows::core::{PWSTR, Result as WinResult};

pub fn protect_current_user(plain: &[u8]) -> WinResult<Vec<u8>> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&in_blob, PWSTR::null(), None, None, None, 0, &mut out)?;
        read_and_free(out)
    }
}

pub fn unprotect_current_user(blob: &[u8]) -> WinResult<Vec<u8>> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out)?;
        read_and_free(out)
    }
}

unsafe fn read_and_free(blob: CRYPT_INTEGER_BLOB) -> WinResult<Vec<u8>> {
    if blob.pbData.is_null() {
        return Ok(Vec::new());
    }
    unsafe {
        let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut core::ffi::c_void)));
        Ok(bytes)
    }
}
