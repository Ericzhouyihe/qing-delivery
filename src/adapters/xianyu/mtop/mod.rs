//! mtop 协议模块:签名与请求构造。

pub mod sign;

#[cfg(test)]
mod tests {
    use super::sign::sign_request;

    /// 签名必须与实际发送字节一致(R4):固定向量钉住公式,防实现漂移。
    #[test]
    fn sign_matches_pinned_vector() {
        let got = sign_request("TOKEN", "1720000000000", "12574496", r#"{"itemId":"1"}"#);
        // 期望值由公式独立计算得出:md5("TOKEN&1720000000000&12574496&{\"itemId\":\"1\"}")
        let expected = md5_of("TOKEN&1720000000000&12574496&{\"itemId\":\"1\"}");
        assert_eq!(got, expected);
    }

    #[test]
    fn sign_changes_with_data_bytes() {
        let a = sign_request("T", "1", "A", "{}");
        let b = sign_request("T", "1", "A", "{ }");
        assert_ne!(a, b, "签名必须区分字节差异");
    }

    fn md5_of(input: &str) -> String {
        use md5::Digest;
        let mut h = md5::Md5::new();
        h.update(input.as_bytes());
        hex::encode(h.finalize())
    }
}
