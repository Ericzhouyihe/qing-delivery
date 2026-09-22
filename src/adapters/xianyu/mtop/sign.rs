//! mtop 请求签名(T044):sign = md5(token & t & appKey & data)。
//! 签名 data 必须与实际发送字节一致——调用方传入序列化后的最终字符串,
//! 不得用结构体重复序列化(R4)。

use md5::Digest;

pub fn sign_request(token: &str, t: &str, app_key: &str, data_json: &str) -> String {
    let mut hasher = md5::Md5::new();
    hasher.update(token.as_bytes());
    hasher.update(b"&");
    hasher.update(t.as_bytes());
    hasher.update(b"&");
    hasher.update(app_key.as_bytes());
    hasher.update(b"&");
    hasher.update(data_json.as_bytes());
    hex::encode(hasher.finalize())
}
