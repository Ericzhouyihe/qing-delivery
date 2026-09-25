//! VerificationDriver 的 chromiumoxide 实现(003/T009,D1/D4):
//! 打开验证页 → 注入种子 Cookie → 滑块处置 → 读取完整 Cookie jar。
//! 平台 specifics(选择器/成功判定)为实账号校准项(R11),失败如实分类。

use std::sync::Arc;

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};

use crate::adapters::browser::manager::BrowserManager;
use crate::adapters::browser::slider::{SLIDER_SELECTORS, generate_trajectory};
use crate::application::verification::{SolveOutcome, VerificationDriver};

pub struct CdpVerifyDriver {
    manager: Arc<BrowserManager>,
}

impl CdpVerifyDriver {
    pub fn new(manager: Arc<BrowserManager>) -> Self {
        Self { manager }
    }

    /// 纯函数(可测):CDP Cookie → 平台 CookieJar 兼容 JSON。
    pub fn jar_json_from_cdp(
        cookies: &[chromiumoxide::cdp::browser_protocol::network::Cookie],
    ) -> String {
        use serde_json::json;
        let items: Vec<serde_json::Value> = cookies
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "value": c.value,
                    "domain": c.domain,
                    "path": c.path,
                    "expires_at_ms": if c.expires > 0.0 { Some((c.expires * 1000.0) as i64) } else { None },
                })
            })
            .collect();
        let text = serde_json::to_string(&serde_json::json!({ "cookies": items }))
            .unwrap_or_else(|_| "{\"cookies\":[]}".into());
        // 形状校验:必须能被 CookieJar::from_json 解析(防漂移)
        match crate::adapters::xianyu::cookies::CookieJar::from_json(&text) {
            Some(_) => text,
            None => "[]".to_string(),
        }
    }

    async fn seed_cookies(page: &Page, jar_json: &str) {
        let Ok(vec) = serde_json::from_str::<Vec<serde_json::Value>>(jar_json) else {
            return;
        };
        for c in vec {
            let (Some(name), Some(value)) = (
                c.get("name").and_then(|v| v.as_str()),
                c.get("value").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let domain = c
                .get("domain")
                .and_then(|v| v.as_str())
                .unwrap_or(".goofish.com");
            let params = chromiumoxide::cdp::browser_protocol::network::SetCookieParams::builder()
                .name(name)
                .value(value)
                .domain(domain)
                .build();
            if let Ok(p) = params {
                let _ = page.execute(p).await;
            }
        }
    }

    /// 滑块处置:定位 → 轨迹拖动 → 成功判定(实账号校准点 R11)。
    async fn solve_slider(page: &Page) -> Result<(), String> {
        let mut geometry: Option<(f64, f64, f64)> = None;
        for sel in SLIDER_SELECTORS {
            let expr = format!(
                "(()=>{{const e=document.querySelector('{sel}');if(!e)return null;const r=e.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2,r.width];}})()"
            );
            if let Ok(v) = page.evaluate(expr.as_str()).await
                && let Some(val) = v.value()
                && let Some(arr) = val.as_array()
                && arr.len() == 3
            {
                let nums: Vec<f64> = arr.iter().filter_map(|n| n.as_f64()).collect();
                if nums.len() == 3 {
                    geometry = Some((nums[0], nums[1], nums[2]));
                    break;
                }
            }
        }
        let Some((x, y, w)) = geometry else {
            return Err("未找到滑块元素(选择器待实账号校准)".into());
        };
        let track = w.max(220.0) * 1.35;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(7);
        let pts = generate_trajectory(x, y, x + track, y, seed, 64);
        let dispatch = |kind: DispatchMouseEventType, x: f64, y: f64| {
            let params = DispatchMouseEventParams::builder()
                .r#type(kind)
                .x(x)
                .y(y)
                .button(MouseButton::Left)
                .click_count(1)
                .build();
            async {
                match params {
                    Ok(p) => page.execute(p).await.map(|_| ()),
                    Err(e) => Err(chromiumoxide::error::CdpError::ChromeMessage(format!(
                        "参数构建失败:{e}"
                    ))),
                }
            }
        };
        dispatch(DispatchMouseEventType::MousePressed, x, y)
            .await
            .map_err(|e| format!("按下失败:{e}"))?;
        for pt in &pts {
            dispatch(DispatchMouseEventType::MouseMoved, pt.x, pt.y)
                .await
                .map_err(|e| format!("移动失败:{e}"))?;
            tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        }
        dispatch(DispatchMouseEventType::MouseReleased, x + track, y)
            .await
            .map_err(|e| format!("释放失败:{e}"))?;
        // 成功判定:验证容器消失(实账号校准)
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        let probe = "(()=>{const e=document.querySelector('.nc-container,[class*=baxia],[id^=nc_]');return e?'still':'gone';})()";
        if let Ok(v) = page.evaluate(probe).await
            && let Some(val) = v.value()
            && val.as_str() == Some("gone")
        {
            return Ok(());
        }
        Err("滑块未通过(平台仍显示验证)".into())
    }
}

impl VerificationDriver for CdpVerifyDriver {
    fn solve_boxed(
        &self,
        url: String,
        cookie_seed: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SolveOutcome> + Send>> {
        let manager = self.manager.clone();
        Box::pin(async move {
            let session = match manager.session().await {
                Ok(s) => s,
                Err(e) => {
                    return SolveOutcome::Failed {
                        reason: e.to_string(),
                        stage: "browser_open",
                    };
                }
            };
            let page = &session.page;
            if let Err(e) = page.goto(&url).await {
                return SolveOutcome::Failed {
                    reason: format!("打开验证页失败:{e}"),
                    stage: "browser_open",
                };
            }
            Self::seed_cookies(page, &cookie_seed).await;
            if !cookie_seed.is_empty()
                && let Err(e) = page.goto(&url).await
            {
                return SolveOutcome::Failed {
                    reason: format!("回访验证页失败:{e}"),
                    stage: "browser_open",
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            if let Err(reason) = Self::solve_slider(page).await {
                return SolveOutcome::Failed {
                    reason,
                    stage: "solving",
                };
            }
            match page.get_cookies().await {
                Ok(cookies) => SolveOutcome::Solved {
                    cookie_jar: Self::jar_json_from_cdp(&cookies),
                },
                Err(e) => SolveOutcome::Failed {
                    reason: format!("读取 Cookie 失败:{e}"),
                    stage: "updating_credential",
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_cookie_转_jar_json_形状可解析() {
        let mk = |name: &str, dom: &str| {
            chromiumoxide::cdp::browser_protocol::network::Cookie::builder()
                .name(name)
                .value("v")
                .domain(dom)
                .path("/")
                .expires(-1.0)
                .size(2)
                .http_only(false)
                .secure(false)
                .session(true)
                .priority(chromiumoxide::cdp::browser_protocol::network::CookiePriority::Medium)
                .same_site(chromiumoxide::cdp::browser_protocol::network::CookieSameSite::Lax)
                .source_scheme(
                    chromiumoxide::cdp::browser_protocol::network::CookieSourceScheme::Unset,
                )
                .source_port(80)
                .build()
                .unwrap()
        };
        let json =
            CdpVerifyDriver::jar_json_from_cdp(&[mk("a", ".goofish.com"), mk("b", ".taobao.com")]);
        let parsed = crate::adapters::xianyu::cookies::CookieJar::from_json(&json);
        assert!(parsed.is_some(), "必须能被 CookieJar 解析:{json}");
    }
}
