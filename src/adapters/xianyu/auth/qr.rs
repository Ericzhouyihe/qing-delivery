//! 真实扫码登录协议(T041):passport.goofish.com 二维码创建/轮询/确认。
//! mini_login.htm 提取 loginFormData → generate.do 取码 → query.do 轮询状态;
//! CONFIRMED 后以专用 Jar 访问 goofish.com/im 跟随全部重定向收齐 Cookie,
//! 校验 unb 存在才认定账号身份(没有 unb 不建账号)。
//! 参考 R4 行为重写,不做逐行迁移。

use std::time::Duration;

use serde_json::Value;

use crate::adapters::xianyu::cookies::CookieJar;
use crate::adapters::xianyu::ws::real::BROWSER_UA;

pub const DEFAULT_PASSPORT_BASE: &str = "https://passport.goofish.com";
pub const DEFAULT_IM_URL: &str = "https://www.goofish.com/im";

/// 会话有效期 5 分钟(上游 qrSessionTTL);轮询间隔由调用方控制
pub const QR_SESSION_TTL_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QrStatus {
    /// 未扫码
    New,
    /// 已扫码待确认
    Scanned,
    /// 已确认:可收 Cookie
    Confirmed,
    /// 二维码过期
    Expired,
    /// 用户取消
    Canceled,
    /// 风控/人脸验证:需人工在浏览器完成
    Verification {
        url: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum QrError {
    #[error("网络不可用:{0}")]
    Network(String),
    #[error("响应格式异常:{0}")]
    Malformed(String),
    #[error("登录参数页无法解析:{0}")]
    LoginParams(String),
    #[error("扫码完成但未取得账号标识(unb)")]
    MissingAccountIdentity,
}

pub struct QrCodeMaterial {
    /// 二维码原始内容(用于生成图片)
    pub code_content: String,
    /// PNG data URL(256px 等效 SVG,前端 <img> 可直接显示)
    pub image_data_url: String,
    pub expires_at_ms: i64,
}

pub struct ConfirmedLogin {
    pub cookie_jar_json: String,
    pub unb: String,
}

pub struct QrLogin {
    http: reqwest::Client,
    jar: std::sync::Arc<tokio::sync::Mutex<CookieJar>>,
    passport_base: String,
    im_url: String,
    /// loginFormData 展开后的表单键值(含 umidTag)
    params: Vec<(String, String)>,
}

impl QrLogin {
    /// 新建登录会话;基址可注入(夹具)。
    pub fn new(passport_base: &str, im_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .expect("扫码 HTTP 客户端构建失败");
        Self {
            http,
            jar: std::sync::Arc::new(tokio::sync::Mutex::new(CookieJar::new())),
            passport_base: passport_base.to_string(),
            im_url: im_url.to_string(),
            params: Vec::new(),
        }
    }

    /// 生成二维码:先取登录参数页,再请求 generate.do。
    pub async fn generate(&mut self) -> Result<QrCodeMaterial, QrError> {
        self.fetch_login_params().await?;
        let query = self.form_query();
        let url = format!("{}/newlogin/qrcode/generate.do?{query}", self.passport_base);
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::REFERER, format!("{}/", self.passport_base))
            .header(reqwest::header::USER_AGENT, BROWSER_UA)
            .send()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let host = host_of(&url);
        self.jar
            .lock()
            .await
            .absorb_response(&host, response.headers());
        let text = response
            .text()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|e| QrError::Malformed(format!("generate.do:{e}")))?;
        let success = parsed
            .get("content")
            .and_then(|c| c.get("success"))
            .and_then(|s| s.as_bool())
            .unwrap_or(false);
        if !success {
            return Err(QrError::Malformed("generate.do 返回 success=false".into()));
        }
        let data = parsed
            .get("content")
            .and_then(|c| c.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        // t 为毫秒时间戳数字:转纯数字字符串,防科学计数法
        let t =
            json_number_plain(data.get("t")).ok_or_else(|| QrError::Malformed("缺少 t".into()))?;
        let ck = data
            .get("ck")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QrError::Malformed("缺少 ck".into()))?
            .to_string();
        let code_content = data
            .get("codeContent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QrError::Malformed("缺少 codeContent".into()))?
            .to_string();
        self.params.retain(|(k, _)| k != "t" && k != "ck");
        self.params.push(("t".into(), t));
        self.params.push(("ck".into(), ck));
        let image_data_url = qr_svg_data_url(&code_content);
        Ok(QrCodeMaterial {
            expires_at_ms: crate::domain::time_util::utc_now_ms() + QR_SESSION_TTL_MS,
            code_content,
            image_data_url,
        })
    }

    /// 轮询一次扫码状态。
    pub async fn poll(&self) -> Result<QrStatus, QrError> {
        let mut form: Vec<(String, String)> = self.params.clone();
        for (k, v) in [
            ("ua", ""),
            ("navlanguage", "zh-CN"),
            ("navUserAgent", BROWSER_UA),
            ("navPlatform", "Win32"),
            ("isIframe", "true"),
            ("documentReferer", "https://www.goofish.com/im"),
            ("defaultView", "qrcode"),
        ] {
            form.push((k.into(), v.into()));
        }
        let body = form
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}/newlogin/qrcode/query.do", self.passport_base);
        let response = self
            .http
            .post(&url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::REFERER, format!("{}/", self.passport_base))
            .header(reqwest::header::ORIGIN, self.passport_base.clone())
            .header(reqwest::header::USER_AGENT, BROWSER_UA)
            .body(body)
            .send()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let host = host_of(&url);
        self.jar
            .lock()
            .await
            .absorb_response(&host, response.headers());
        let text = response
            .text()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let parsed: Value =
            serde_json::from_str(&text).map_err(|e| QrError::Malformed(format!("query.do:{e}")))?;
        if parsed
            .get("hasError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(QrStatus::Failed("服务端 hasError".into()));
        }
        let data = parsed
            .get("content")
            .and_then(|c| c.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let status = data
            .get("qrCodeStatus")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        Ok(match status {
            "NEW" => QrStatus::New,
            "SCANED" => QrStatus::Scanned,
            "CONFIRMED" => {
                if data
                    .get("iframeRedirect")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    QrStatus::Verification {
                        url: data
                            .get("iframeRedirectUrl")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    }
                } else {
                    QrStatus::Confirmed
                }
            }
            "EXPIRED" => QrStatus::Expired,
            "CANCELED" => QrStatus::Canceled,
            other => QrStatus::Failed(format!("未知状态:{other}")),
        })
    }

    /// 确认后换正式 Cookie:GET im 页跟随全部重定向,收齐 Set-Cookie;必须含 unb。
    pub async fn complete(self) -> Result<ConfirmedLogin, QrError> {
        let mut jar = self.jar.lock().await.clone();
        let mut url = self.im_url.clone();
        // 手动跟随重定向(≤10 跳),沿途吸收全部 Set-Cookie(含 HttpOnly)
        for _ in 0..10 {
            let host = host_of(&url);
            let cookie_header = jar.header_value(&host, "/");
            let mut request = self
                .http
                .get(&url)
                .header(reqwest::header::REFERER, "https://www.goofish.com/")
                .header(reqwest::header::USER_AGENT, BROWSER_UA);
            if !cookie_header.is_empty() {
                request = request.header(reqwest::header::COOKIE, cookie_header);
            }
            let response = request
                .send()
                .await
                .map_err(|e| QrError::Network(e.to_string()))?;
            jar.absorb_response(&host, response.headers());
            match response.status() {
                reqwest::StatusCode::FOUND
                | reqwest::StatusCode::MOVED_PERMANENTLY
                | reqwest::StatusCode::SEE_OTHER
                | reqwest::StatusCode::TEMPORARY_REDIRECT => {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|l| l.to_str().ok())
                        .map(|s| s.to_string());
                    match location {
                        Some(loc) => {
                            url = resolve_url(&url, &loc)?;
                            continue;
                        }
                        None => break,
                    }
                }
                _ => break,
            }
        }
        let unb = jar
            .unb()
            .map(|s| s.to_string())
            .ok_or(QrError::MissingAccountIdentity)?;
        Ok(ConfirmedLogin {
            unb,
            cookie_jar_json: jar.to_json(),
        })
    }

    /// mini_login.htm:提取 window.viewData 的 loginFormData。
    async fn fetch_login_params(&mut self) -> Result<(), QrError> {
        let rnd: f64 = rand::random::<f64>();
        let url = format!(
            "{}/mini_login.htm?lang=zh_cn&appName=xianyu&appEntrance=web&styleType=vertical\
             &bizParams=&notLoadSsoView=false&notKeepLogin=false&isMobile=false\
             &qrCodeFirst=false&stie=77&rnd={rnd}",
            self.passport_base
        );
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::REFERER, "https://www.goofish.com/im")
            .header(reqwest::header::USER_AGENT, BROWSER_UA)
            .send()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let host = host_of(&url);
        self.jar
            .lock()
            .await
            .absorb_response(&host, response.headers());
        let html = response
            .text()
            .await
            .map_err(|e| QrError::Network(e.to_string()))?;
        let view_data = extract_view_data(&html)
            .ok_or_else(|| QrError::LoginParams("未找到 window.viewData".into()))?;
        let login_form = view_data
            .get("loginFormData")
            .cloned()
            .unwrap_or(Value::Null);
        let Some(obj) = login_form.as_object() else {
            return Err(QrError::LoginParams("loginFormData 不是对象".into()));
        };
        self.params = obj
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), value)
            })
            .collect();
        self.params.retain(|(k, _)| k != "umidTag");
        self.params.push(("umidTag".into(), "SERVER".into()));
        Ok(())
    }

    fn form_query(&self) -> String {
        self.params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// 从 HTML 提取 `window.viewData = {...};` 的 JSON 对象。
pub fn extract_view_data(html: &str) -> Option<Value> {
    let marker = "window.viewData";
    let start = html.find(marker)?;
    let brace = html[start..].find('{')? + start;
    // 平衡花括号扫描(值内可能含字符串里的花括号)
    let bytes = html.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(brace) {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&html[brace..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// 二维码内容 → SVG data URL(256px,无需 image 依赖)。
pub fn qr_svg_data_url(content: &str) -> String {
    let code = qrcode::QrCode::with_error_correction_level(content.as_bytes(), qrcode::EcLevel::M)
        .expect("二维码内容编码失败");
    let count = code.width();
    let scale = 8usize; // 256px / 32~57 模块
    let quiet = 2; // 静区(模块数)
    let dim = (count + quiet * 2) * scale;
    let mut rects = String::new();
    for y in 0..count {
        for x in 0..count {
            if code[(x, y)] == qrcode::Color::Dark {
                rects.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" w=\"1\"/>",
                    (x + quiet) * scale,
                    (y + quiet) * scale
                ));
            }
        }
    }
    // 用 SVG path 而非海量 rect:逐行合并连续暗模块
    let mut paths = String::new();
    for y in 0..count {
        let mut run_start: Option<usize> = None;
        for x in 0..=count {
            let dark = x < count && code[(x, y)] == qrcode::Color::Dark;
            match (run_start, dark) {
                (Some(s), false) => {
                    // 每段暗模块 = 宽(x-s)*scale、高 scale 的实心方块;
                    // 高度必须是 scale,否则渲染为横向细条,二维码无法识别
                    paths.push_str(&format!(
                        "M{} {}h{}v{}h-{}z ",
                        (s + quiet) * scale,
                        (y + quiet) * scale,
                        (x - s) * scale,
                        scale,
                        (x - s) * scale
                    ));
                    run_start = None;
                }
                (None, true) => run_start = Some(x),
                _ => {}
            }
        }
    }
    let _ = rects; // rects 仅用于说明,实际用 path
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{dim}\" height=\"{dim}\" \
         shape-rendering=\"crispEdges\"><rect width=\"{dim}\" height=\"{dim}\" fill=\"#fff\"/>\
         <path d=\"{paths}\" fill=\"#000\"/></svg>"
    );
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, svg.as_bytes());
    format!("data:image/svg+xml;base64,{b64}")
}

fn json_number_plain(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "www.goofish.com".into())
}

fn resolve_url(base: &str, location: &str) -> Result<String, QrError> {
    let base = url::Url::parse(base).map_err(|e| QrError::Malformed(e.to_string()))?;
    base.join(location)
        .map(|u| u.to_string())
        .map_err(|e| QrError::Malformed(e.to_string()))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn view_data_extraction_from_page() {
        let html = r#"<html><script>window.viewData = {"env":"prod","loginFormData":{"appId":"x","st":"77","fromSite":"77"}};</script></html>"#;
        let v = extract_view_data(html).unwrap();
        assert_eq!(v["loginFormData"]["appId"], "x");
        // 值内花括号/转义引号不破坏平衡扫描
        let tricky = r#"window.viewData = {"a":"he said \"hi\" {ok}","b":1};"#;
        let parsed = extract_view_data(tricky).unwrap();
        assert_eq!(parsed["b"], 1);
    }

    #[test]
    fn qr_svg_data_url_is_valid_svg() {
        let url = qr_svg_data_url("https://h5api.m.goofish.com/qr?token=abc");
        assert!(url.starts_with("data:image/svg+xml;base64,"));
        let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let svg = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let svg = String::from_utf8(svg).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("fill=\"#000\""));
    }

    /// 回归(实机走查发现):路径段高度必须是 scale(8px 方块),
    /// 曾误写 `v1` 渲染为横向细条,二维码无法被 App 识别。
    #[test]
    fn qr_svg_path_segments_are_square() {
        const SCALE: usize = 8;
        let url = qr_svg_data_url("https://h5api.m.goofish.com/qr?token=abc");
        let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let svg = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap(),
        )
        .unwrap();
        let path_start = svg.find("d=\"").expect("path d 属性") + 3;
        let path = &svg[path_start..svg[path_start..].find('"').unwrap() + path_start];
        for seg in path.split('M').skip(1) {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            // 形如 M{x} {y}h{w}v{h}h-{w}z
            let v = seg
                .split('v')
                .nth(1)
                .and_then(|rest| rest.split('h').next())
                .and_then(|h| h.trim().parse::<usize>().ok())
                .unwrap_or(0);
            assert_eq!(v, SCALE, "路径段高度必须为 {SCALE},实得 {v}");
        }
    }

    #[test]
    fn number_plain_avoids_scientific_notation() {
        assert_eq!(
            json_number_plain(Some(&serde_json::json!(1720000000000i64))),
            Some("1720000000000".into())
        );
    }

    #[tokio::test]
    async fn full_qr_flow_against_local_fixture() {
        // 夹具服务:mini_login → generate → query(NEW→SCANED→CONFIRMED)→ /im 重定向收 unb
        use axum::routing::{get, post};
        use std::sync::atomic::{AtomicU32, Ordering};
        let step = std::sync::Arc::new(AtomicU32::new(0));
        let s2 = step.clone();
        let app = axum::Router::new()
            .route(
                "/mini_login.htm",
                get(|| async {
                    r#"<script>window.viewData = {"loginFormData":{"appId":"xy","st":"77"}};</script>"#
                }),
            )
            .route(
                "/newlogin/qrcode/generate.do",
                get(|| async {
                    r#"{"content":{"success":true,"data":{"t":1720000000000,"ck":"ck1","codeContent":"https://qr.example/abc"}}}"#
                }),
            )
            .route(
                "/newlogin/qrcode/query.do",
                post(move || {
                    let s2 = s2.clone();
                    async move {
                        let n = s2.fetch_add(1, Ordering::SeqCst);
                        let mut h = reqwest::header::HeaderMap::new();
                        let (status, body) = match n {
                            0 => (
                                reqwest::StatusCode::OK,
                                r#"{"hasError":false,"content":{"data":{"qrCodeStatus":"NEW"}}}"#.to_string(),
                            ),
                            1 => (
                                reqwest::StatusCode::OK,
                                r#"{"hasError":false,"content":{"data":{"qrCodeStatus":"SCANED"}}}"#.to_string(),
                            ),
                            _ => {
                                h.insert(
                                    reqwest::header::LOCATION,
                                    "https://www.goofish.local/im".parse().unwrap(),
                                );
                                (
                                    reqwest::StatusCode::FOUND,
                                    r#"{"hasError":false,"content":{"data":{"qrCodeStatus":"CONFIRMED","iframeRedirect":false}}}"#.to_string(),
                                )
                            }
                        };
                        (status, h, body)
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // /im:下发 unb Cookie 表示换 Cookie 成功
        let im_app = axum::Router::new().route(
            "/im",
            get(|| async {
                let mut h = reqwest::header::HeaderMap::new();
                h.append(
                    reqwest::header::SET_COOKIE,
                    "unb=998877; Domain=.goofish.local; Path=/".parse().unwrap(),
                );
                h.append(
                    reqwest::header::SET_COOKIE,
                    "sess=s1; Domain=.goofish.local; Path=/; HttpOnly"
                        .parse()
                        .unwrap(),
                );
                (h, "ok")
            }),
        );
        let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr2 = listener2.local_addr().unwrap();
        let im_target = format!("http://{addr2}/im");
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::spawn(async move { axum::serve(listener2, im_app).await.unwrap() });

        let mut qr = QrLogin::new(&format!("http://{addr}"), &im_target);
        let material = qr.generate().await.unwrap();
        assert!(material.code_content.contains("qr.example"));
        assert!(material.image_data_url.starts_with("data:image/svg+xml"));
        assert_eq!(qr.poll().await.unwrap(), QrStatus::New);
        assert_eq!(qr.poll().await.unwrap(), QrStatus::Scanned);
        assert_eq!(qr.poll().await.unwrap(), QrStatus::Confirmed);
        let confirmed = qr.complete().await.unwrap();
        assert_eq!(confirmed.unb, "998877");
        let jar = CookieJar::from_json(&confirmed.cookie_jar_json).unwrap();
        assert!(jar.get("sess").is_some(), "完整属性 Cookie 已收齐");
        let _ = addr2;
    }
}
