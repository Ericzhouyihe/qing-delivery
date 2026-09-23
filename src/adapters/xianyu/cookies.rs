//! Cookie Jar(T043 凭证载体):保留完整 domain/path/有效期属性,
//! 序列化为 JSON 交由 account_credentials 加密落库。
//! 参考上游行为:不实现 RFC6265 全集,只覆盖 goofish/taobao 域实际用到的子集。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix 毫秒;None 为会话 Cookie(Jar 序列化时仍保留)
    pub expires_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CookieJar {
    /// 同名同 domain+path 视为一条;后写覆盖先写
    cookies: Vec<StoredCookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "[]".into())
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&StoredCookie> {
        self.cookies.iter().find(|c| c.name == name)
    }

    pub fn unb(&self) -> Option<&str> {
        self.get("unb")
            .map(|c| c.value.as_str())
            .filter(|v| !v.is_empty())
    }

    /// 吸收一个 Set-Cookie 头;Max-Age<0 或过期即删除该条。
    pub fn store_set_cookie(&mut self, url_host: &str, header: &str) {
        let mut parts = header.split(';');
        let Some(first) = parts.next() else { return };
        let Some((name, value)) = first.split_once('=') else {
            return;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        let mut domain = url_host.to_string();
        let mut path = "/".to_string();
        let mut expires: Option<i64> = None;
        let mut delete = false;
        for attr in parts {
            let attr = attr.trim();
            let (k, v) = attr.split_once('=').unwrap_or((attr, ""));
            match k.to_ascii_lowercase().as_str() {
                "domain" => {
                    let d = v.trim().trim_start_matches('.');
                    if !d.is_empty() {
                        domain = d.to_string();
                    }
                }
                "path" => {
                    if !v.is_empty() {
                        path = v.to_string();
                    }
                }
                "max-age" => match v.trim().parse::<i64>() {
                    Ok(n) if n <= 0 => delete = true,
                    Ok(n) => expires = Some(now_ms() + n * 1000),
                    Err(_) => {}
                },
                "expires" => {
                    if v.trim().is_empty()
                        || v.trim()
                            .eq_ignore_ascii_case("thu, 01 jan 1970 00:00:00 gmt")
                    {
                        delete = true;
                    } else {
                        expires = parse_http_date(v.trim());
                    }
                }
                _ => {}
            }
        }
        self.cookies
            .retain(|c| !(c.name == name && c.domain == domain && c.path == path));
        if !delete {
            self.cookies.push(StoredCookie {
                name,
                value,
                domain,
                path,
                expires_at_ms: expires,
            });
        }
    }

    /// 吸收一次响应的全部 Set-Cookie 头。
    pub fn absorb_response(&mut self, url_host: &str, headers: &reqwest::header::HeaderMap) {
        for value in headers.get_all(reqwest::header::SET_COOKIE) {
            if let Ok(h) = value.to_str() {
                self.store_set_cookie(url_host, h);
            }
        }
    }

    /// 构造发往 url_host+path 的 Cookie 请求头值;domain 后缀匹配 + path 前缀 + 未过期。
    pub fn header_value(&self, url_host: &str, url_path: &str) -> String {
        let now = now_ms();
        let host = url_host.trim_start_matches('.');
        let path = if url_path.is_empty() { "/" } else { url_path };
        let mut order: HashMap<&str, usize> = HashMap::new();
        for c in &self.cookies {
            if let Some(exp) = c.expires_at_ms
                && exp <= now
            {
                continue;
            }
            let domain = c.domain.trim_start_matches('.');
            let domain_ok = host == domain || host.ends_with(&format!(".{domain}"));
            let path_ok = c.path == "/"
                || path == c.path
                || (path.starts_with(&c.path) && path.as_bytes().get(c.path.len()) == Some(&b'/'));
            if domain_ok && path_ok {
                order.entry(c.name.as_str()).or_insert(0);
            }
        }
        // 精确 domain 优先(与浏览器行为一致:更具体的域在前)
        let mut matched: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|c| order.contains_key(c.name.as_str()))
            .collect();
        matched.sort_by_key(|c| c.domain.len());
        let mut seen = std::collections::HashSet::new();
        matched
            .into_iter()
            .filter(|c| seen.insert(c.name.clone()))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 签名 Token:_m_h5_tk 值首个 '_' 前的段(取首个匹配,不做 map 化)。
    pub fn sign_token(&self) -> String {
        self.get("_m_h5_tk")
            .map(|c| c.value.split('_').next().unwrap_or("").to_string())
            .unwrap_or_default()
    }

    /// Token 重试耗尽后清除签名对(goofish.com 与 m.goofish.com Path=/ 作用域)。
    pub fn clear_sign_cookies(&mut self) {
        self.cookies
            .retain(|c| !matches!(c.name.as_str(), "_m_h5_c" | "_m_h5_tk" | "_m_h5_tk_enc"));
    }
}

fn now_ms() -> i64 {
    crate::domain::time_util::utc_now_ms()
}

/// 解析 HTTP 日期(RFC1123 子集);失败返回 None(视为会话 Cookie)。
fn parse_http_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_attributes_preserved() {
        let mut jar = CookieJar::new();
        jar.store_set_cookie(
            "passport.goofish.com",
            "sess=abc; Domain=.goofish.com; Path=/; HttpOnly; Secure",
        );
        assert_eq!(jar.get("sess").unwrap().domain, "goofish.com");
        assert_eq!(jar.get("sess").unwrap().path, "/");
        // 子域可发
        assert!(
            jar.header_value("h5api.m.goofish.com", "/x")
                .contains("sess=abc")
        );
        // 无关域不发
        assert!(!jar.header_value("taobao.com", "/").contains("sess"));
    }

    #[test]
    fn sign_token_takes_first_underscore_segment() {
        let mut jar = CookieJar::new();
        jar.store_set_cookie("h5api.m.goofish.com", "_m_h5_tk=abc123_1720000000");
        assert_eq!(jar.sign_token(), "abc123");
        jar.clear_sign_cookies();
        assert!(jar.get("_m_h5_tk").is_none());
    }

    #[test]
    fn json_roundtrip_keeps_full_attributes() {
        let mut jar = CookieJar::new();
        jar.store_set_cookie(
            "www.goofish.com",
            "unb=123456; Domain=.goofish.com; Path=/; Expires=Fri, 01 Jan 2100 00:00:00 GMT",
        );
        let json = jar.to_json();
        let back = CookieJar::from_json(&json).unwrap();
        assert_eq!(jar, back);
        assert_eq!(back.unb(), Some("123456"));
        assert!(back.get("unb").unwrap().expires_at_ms.is_some());
    }

    #[test]
    fn expired_cookie_not_sent_but_negative_max_age_deletes() {
        let mut jar = CookieJar::new();
        jar.store_set_cookie("goofish.com", "old=1; Domain=.goofish.com; Max-Age=-1");
        assert!(jar.get("old").is_none());
        jar.store_set_cookie("goofish.com", "soon=2; Domain=.goofish.com; Max-Age=3600");
        assert!(jar.header_value("www.goofish.com", "/").contains("soon=2"));
    }
}
