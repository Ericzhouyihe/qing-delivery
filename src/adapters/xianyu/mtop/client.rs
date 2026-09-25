//! mtop HTTP 客户端(T044):签名调用、Token 获取与续期、错误分类、Set-Cookie 合并。
//! 签名 data 必须与发送字节一致(调用方传最终 JSON 串);重试只针对签名 Token
//! 过期(每次吸收新 Set-Cookie 重签),业务结果取决于提交证据而非新 Token。

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::adapters::xianyu::cookies::CookieJar;
use crate::adapters::xianyu::mtop::sign::sign_request;

/// mtop 签名 appKey(h5api 常规接口)
pub const SIGN_APP_KEY: &str = "34839810";
/// WS /reg 与 token 请求 data 内的 appKey
pub const REG_APP_KEY: &str = "444e9908a51d1cb236a27862abc769c9";

pub const DEFAULT_H5_BASE: &str = "https://h5api.m.goofish.com/h5/";

/// lib-mtop 2.7.3 的 Token 过期重试上限
const TOKEN_RETRY_MAX: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum MtopError {
    /// FAIL_SYS_TOKEN_*:吸收新 Cookie 后可重签重试
    #[error("签名 Token 过期")]
    TokenExpired,
    #[error("登录会话失效:{0}")]
    SessionExpired(String),
    /// 风控/滑块/人脸:返回验证地址,冷却后人工恢复
    #[error("需要平台验证:{ret}")]
    Verification { url: Option<String>, ret: String },
    #[error("业务拒绝:{0}")]
    Biz(String),
    #[error("系统错误:{0}")]
    System(String),
    #[error("限流")]
    RateLimited,
    #[error("网络不可用:{0}")]
    Network(String),
    #[error("超时")]
    Timeout,
    #[error("响应格式异常:{0}")]
    Malformed(String),
}

impl MtopError {
    pub fn to_platform(&self) -> crate::application::ports::platform::PlatformError {
        use crate::application::ports::platform::PlatformError as P;
        match self {
            MtopError::TokenExpired => P::SigningTokenExpired,
            MtopError::SessionExpired(_) => P::SessionExpired,
            MtopError::Verification { .. } => P::VerificationRequired,
            MtopError::Biz(m) => P::BusinessRejected(m.clone()),
            MtopError::System(m) => P::BusinessRejected(format!("FAIL_SYS:{m}")),
            MtopError::RateLimited => P::RateLimited,
            MtopError::Network(_) => P::NetworkUnavailable,
            MtopError::Timeout => P::Timeout,
            MtopError::Malformed(_) => P::MalformedResponse,
        }
    }
}

/// 按 ret 首段分类(上游错误分类表)。
pub fn classify_ret(ret: &str) -> Result<(), MtopError> {
    if ret.contains("SUCCESS") {
        return Ok(());
    }
    if ret.contains("FAIL_SYS_TOKEN_EXOIRED")
        || ret.contains("FAIL_SYS_TOKEN_EXPIRED")
        || ret.contains("FAIL_SYS_TOKEN_EMPTY")
    {
        return Err(MtopError::TokenExpired);
    }
    if ret.contains("FAIL_SYS_USER_VALIDATE")
        || ret.contains("rgv587")
        || ret.contains("punish")
        || ret.contains("x5secdata")
    {
        return Err(MtopError::Verification {
            url: None,
            ret: ret.to_string(),
        });
    }
    if ret.contains("session_expired")
        || ret.contains("sid_invalid")
        || ret.contains("auth_reject")
        || ret.contains("need_login")
        || ret.contains("session过期")
        || ret.contains("会话过期")
    {
        return Err(MtopError::SessionExpired(ret.to_string()));
    }
    if ret.contains("TRAFFIC_LIMIT") || ret.contains("FAIL_SYS_FREQUENCY") {
        return Err(MtopError::RateLimited);
    }
    if ret.contains("FAIL_BIZ_") {
        return Err(MtopError::Biz(ret.to_string()));
    }
    Err(MtopError::System(ret.to_string()))
}

/// accessTokenExpiredTime 归一为 Unix 秒:兼容毫秒/秒/RFC3339/相对时长。
pub fn normalize_expiry(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    let now_sec = crate::domain::time_util::utc_now_ms() / 1000;
    if let Some(n) = v.as_i64() {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return Some(if n > 10_000_000_000 { n / 1000 } else { n });
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp());
        }
        // 相对时长(如 "30m"/"7200s")
        let (num, unit) = s.split_at(s.len().saturating_sub(1));
        if let Ok(n) = num.parse::<i64>() {
            return match unit {
                "s" => Some(now_sec + n),
                "m" => Some(now_sec + n * 60),
                "h" => Some(now_sec + n * 3600),
                _ => None,
            };
        }
    }
    None
}

pub struct AccessToken {
    pub token: String,
    pub expires_at_sec: i64,
}

pub struct MtopClient {
    http: reqwest::Client,
    /// 共享 Jar:扫码收齐的 Cookie、mtop 响应 Set-Cookie、token 续期都落在这里
    pub jar: Arc<tokio::sync::Mutex<CookieJar>>,
    h5_base: String,
}

impl MtopClient {
    pub fn new(jar: Arc<tokio::sync::Mutex<CookieJar>>) -> Self {
        Self::with_base(jar, DEFAULT_H5_BASE)
    }

    /// 测试/夹具注入自定义基址。
    pub fn with_base(jar: Arc<tokio::sync::Mutex<CookieJar>>, h5_base: &str) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .expect("mtop HTTP 客户端构建失败");
        Self {
            http,
            jar,
            h5_base: h5_base.to_string(),
        }
    }

    /// WS 接入 Token:deviceId 与后续 /reg 的 did 必须完全一致。
    pub async fn ws_access_token(&self, device_id: &str) -> Result<AccessToken, MtopError> {
        let data = serde_json::json!({
            "appKey": REG_APP_KEY,
            "deviceId": device_id,
        })
        .to_string();
        let resp = self
            .call(
                "mtop.taobao.idlemessage.pc.login.token",
                &data,
                "https://www.goofish.com/im",
            )
            .await?;
        let token = resp
            .get("accessToken")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MtopError::Malformed("缺少 accessToken".into()))?
            .to_string();
        let expires_at_sec = normalize_expiry(resp.get("accessTokenExpiredTime"))
            .unwrap_or_else(|| crate::domain::time_util::utc_now_ms() / 1000 + 1800);
        Ok(AccessToken {
            token,
            expires_at_sec,
        })
    }

    /// 签名调用;Token 过期自动吸收 Set-Cookie 重签(≤5 次),耗尽后清除签名对。
    pub async fn call(
        &self,
        api: &str,
        data_json: &str,
        referer: &str,
    ) -> Result<Value, MtopError> {
        let version = "1.0";
        let endpoint = format!("{h5}{api}/{version}/", h5 = self.h5_base);
        let host = url::Url::parse(&self.h5_base)
            .ok()
            .and_then(|u| u.host_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "h5api.m.goofish.com".into());

        let mut attempt = 0usize;
        loop {
            let (t, sign, cookie_header) = {
                let jar = self.jar.lock().await;
                let t = crate::domain::time_util::utc_now_ms().to_string();
                let sign = sign_request(&jar.sign_token(), &t, SIGN_APP_KEY, data_json);
                (t, sign, jar.header_value(&host, "/"))
            };
            let query = build_query(api, &t, &sign);
            let body = format!("data={}", urlencode(data_json));
            let request = self
                .http
                .post(&endpoint)
                .query(&query)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(reqwest::header::ORIGIN, "https://www.goofish.com")
                .header(reqwest::header::REFERER, referer)
                .header(
                    reqwest::header::USER_AGENT,
                    crate::adapters::xianyu::ws::real::BROWSER_UA,
                )
                .body(body);
            let request = if cookie_header.is_empty() {
                request
            } else {
                request.header(reqwest::header::COOKIE, cookie_header)
            };

            let response = match request.send().await {
                Ok(r) => r,
                Err(e) if e.is_timeout() => return Err(MtopError::Timeout),
                Err(e) if e.is_connect() => return Err(MtopError::Network(e.to_string())),
                Err(e) => return Err(MtopError::Network(e.to_string())),
            };
            let status = response.status();
            {
                let mut jar = self.jar.lock().await;
                jar.absorb_response(&host, response.headers());
            }
            let text = response
                .text()
                .await
                .map_err(|e| MtopError::Network(e.to_string()))?;
            if !status.is_success() {
                return Err(MtopError::System(format!("HTTP {status}")));
            }
            let parsed: Value = serde_json::from_str(&text)
                .map_err(|e| MtopError::Malformed(format!("非 JSON 响应:{e}")))?;

            match classify_response(&parsed) {
                Ok(()) => {
                    return Ok(parsed.get("data").cloned().unwrap_or(Value::Null));
                }
                Err(MtopError::TokenExpired) => {
                    attempt += 1;
                    if attempt >= TOKEN_RETRY_MAX {
                        self.jar.lock().await.clear_sign_cookies();
                        return Err(MtopError::TokenExpired);
                    }
                    // 无有效 _m_h5_tk 时先空手 GET 一次换取新 token Cookie
                    let fresh = {
                        let jar = self.jar.lock().await;
                        jar.get("_m_h5_tk").is_none()
                    };
                    if fresh {
                        self.prime_token_cookie(&endpoint, &host).await.ok();
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 首次调用前触发服务器下发 _m_h5_tk(任意未签名请求即可)。
    async fn prime_token_cookie(&self, endpoint: &str, host: &str) -> Result<(), MtopError> {
        let response = self
            .http
            .get(endpoint)
            .header(reqwest::header::REFERER, "https://www.goofish.com/")
            .send()
            .await
            .map_err(|e| MtopError::Network(e.to_string()))?;
        self.jar
            .lock()
            .await
            .absorb_response(host, response.headers());
        Ok(())
    }
}

/// ret 数组分类:任一非 SUCCESS 段即错误,取首个错误段。
fn classify_response(parsed: &Value) -> Result<(), MtopError> {
    let rets = parsed
        .get("ret")
        .and_then(|v| v.as_array())
        .ok_or_else(|| MtopError::Malformed("缺少 ret 数组".into()))?;
    if rets.is_empty() {
        return Err(MtopError::Malformed("ret 为空".into()));
    }
    for ret in rets {
        if let Some(s) = ret.as_str() {
            // 风控分支补充验证 URL(gotoUrl 等字段,003 D2;提取失败仍触发,url=None)
            if let Err(MtopError::Verification { ret, .. }) = classify_ret(s) {
                let url = crate::application::verification::extract_verification_url(parsed);
                return Err(MtopError::Verification { url, ret });
            }
            classify_ret(s)?;
        }
    }
    Ok(())
}

/// query 键序与取值对齐浏览器抓包(值已单次编码,拼接不再编码)。
fn build_query(api: &str, t: &str, sign: &str) -> Vec<(&'static str, String)> {
    vec![
        ("jsv", "2.7.2".into()),
        ("appKey", SIGN_APP_KEY.into()),
        ("t", t.into()),
        ("sign", sign.into()),
        ("v", "1.0".into()),
        ("type", "originaljson".into()),
        ("accountSite", "xianyu".into()),
        ("dataType", "json".into()),
        ("timeout", "20000".into()),
        ("needLoginPC", "false".into()),
        ("showErrorToast", "false".into()),
        ("api", api.into()),
        ("needLogin", "false".into()),
        ("sessionOption", "AutoLoginOnly".into()),
        ("ecode", "0".into()),
        ("dangerouslySetWindvaneParams", "[object Object]".into()),
        ("spm_cnt", "a21ybx.im.0.0".into()),
        ("spm_pre", String::new()),
        ("log_id", String::new()),
    ]
}

/// application/x-www-form-urlencoded 的 data 值编码(空格→%20 对齐浏览器)。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl MtopClient {
    /// 004:GET 型调用(user.page.nav 等 query 签名接口,研究 D1)。
    /// 签名 data 为固定 `{"%40user%7Eid":...}` 形态时直接传空 JSON;额外 query 键原样附加。
    pub async fn call_get(
        &self,
        api: &str,
        extra_query: &[(&str, &str)],
        referer: &str,
    ) -> Result<Value, MtopError> {
        let version = "1.0";
        let endpoint = format!("{h5}{api}/{version}/", h5 = self.h5_base);
        let host = url::Url::parse(&self.h5_base)
            .ok()
            .and_then(|u| u.host_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "h5api.m.goofish.com".into());
        let data_json = "{}";
        let mut attempt = 0usize;
        loop {
            let (t, sign, cookie_header) = {
                let jar = self.jar.lock().await;
                let t = crate::domain::time_util::utc_now_ms().to_string();
                let sign = sign_request(&jar.sign_token(), &t, SIGN_APP_KEY, data_json);
                (t, sign, jar.header_value(&host, "/"))
            };
            let mut query = build_query(api, &t, &sign);
            for (k, v) in extra_query {
                query.push((*k, (*v).to_string()));
            }
            let mut request = self
                .http
                .get(&endpoint)
                .query(&query)
                .header(reqwest::header::ORIGIN, "https://www.goofish.com")
                .header(reqwest::header::REFERER, referer)
                .header(
                    reqwest::header::USER_AGENT,
                    crate::adapters::xianyu::ws::real::BROWSER_UA,
                );
            if !cookie_header.is_empty() {
                request = request.header(reqwest::header::COOKIE, cookie_header);
            }
            let response = match request.send().await {
                Ok(r) => r,
                Err(e) if e.is_timeout() => return Err(MtopError::Timeout),
                Err(e) => return Err(MtopError::Network(e.to_string())),
            };
            let status = response.status();
            {
                let mut jar = self.jar.lock().await;
                jar.absorb_response(&host, response.headers());
            }
            let text = response
                .text()
                .await
                .map_err(|e| MtopError::Network(e.to_string()))?;
            if !status.is_success() {
                return Err(MtopError::System(format!("HTTP {status}")));
            }
            let parsed: Value = serde_json::from_str(&text)
                .map_err(|e| MtopError::Malformed(format!("非 JSON 响应:{e}")))?;
            match classify_response(&parsed) {
                Ok(()) => return Ok(parsed.get("data").cloned().unwrap_or(Value::Null)),
                Err(MtopError::TokenExpired) => {
                    attempt += 1;
                    if attempt >= TOKEN_RETRY_MAX {
                        self.jar.lock().await.clear_sign_cookies();
                        return Err(MtopError::TokenExpired);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ret_classification_table() {
        assert!(classify_ret("SUCCESS::调用成功").is_ok());
        // 平台拼写如此(EXOIRED)
        assert!(matches!(
            classify_ret("FAIL_SYS_TOKEN_EXOIRED::令牌过期"),
            Err(MtopError::TokenExpired)
        ));
        assert!(matches!(
            classify_ret("FAIL_SYS_USER_VALIDATE::rgv587"),
            Err(MtopError::Verification { .. })
        ));
        assert!(matches!(
            classify_ret("FAIL_SYS_SESSION_EXPIRED::session过期"),
            Err(MtopError::SessionExpired(_))
        ));
        assert!(matches!(
            classify_ret("FAIL_BIZ_ORDER_NOT_FOUND::x"),
            Err(MtopError::Biz(_))
        ));
        assert!(matches!(
            classify_ret("FAIL_SYS_TRAFFIC_LIMIT::x"),
            Err(MtopError::RateLimited)
        ));
    }

    #[test]
    fn query_order_is_pinned() {
        let q = build_query("mtop.x.y", "172", "ab");
        assert_eq!(q[0], ("jsv", "2.7.2".to_string()));
        assert_eq!(q[3].0, "sign");
        assert_eq!(q[11], ("api", "mtop.x.y".to_string()));
    }

    #[test]
    fn data_encoding_percent_uppercase() {
        assert_eq!(urlencode(r#"{"a":1}"#), "%7B%22a%22%3A1%7D");
    }

    #[tokio::test]
    async fn token_retry_absorbs_set_cookie_then_succeeds() {
        // 本地夹具服务:第一次返回 TOKEN_EMPTY 并下发 _m_h5_tk,第二次成功
        use axum::routing::any;
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        let app = axum::Router::new().route(
            "/h5/{*rest}",
            any(move |_headers: reqwest::header::HeaderMap| async move {
                let n = c2.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    let mut h = reqwest::header::HeaderMap::new();
                    h.insert(
                        reqwest::header::SET_COOKIE,
                        "_m_h5_tk=fresh_172; Path=/".parse().unwrap(),
                    );
                    (h, r#"{"ret":["FAIL_SYS_TOKEN_EMPTY::令牌为空"]}"#)
                } else {
                    (
                        reqwest::header::HeaderMap::new(),
                        r#"{"ret":["SUCCESS::调用成功"],"data":{"accessToken":"T1","accessTokenExpiredTime":7200}}"#,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let jar = Arc::new(tokio::sync::Mutex::new(CookieJar::new()));
        let client = MtopClient::with_base(jar.clone(), &format!("http://{addr}/h5/"));
        let token = client.ws_access_token("did-1").await.unwrap();
        assert_eq!(token.token, "T1");
        // 重签后的 token cookie 已吸收
        assert_eq!(jar.lock().await.sign_token(), "fresh");
    }
}
