//! 账号资料拉取(004/T003,D1):mtop.idle.web.user.page.nav GET;
//! 解析 module.base.{displayName→displayNick 兜底, avatar}(参考项目实证字段)。

use serde_json::Value;

use crate::adapters::xianyu::mtop::client::{MtopClient, MtopError};
use crate::application::ports::platform::PlatformError;

pub const PROFILE_API: &str = "mtop.idle.web.user.page.nav";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileResult {
    pub nickname: String,
    pub avatar_url: String,
}

/// 纯解析(data.module.base):displayName→displayNick 兜底链,空对象安全。
pub fn parse_profile(data: &Value) -> ProfileResult {
    let base = data
        .get("module")
        .and_then(|m| m.get("base"))
        .cloned()
        .unwrap_or(Value::Null);
    let get_str = |key: &str| -> String {
        base.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let mut nickname = get_str("displayName");
    if nickname.is_empty() {
        nickname = get_str("displayNick");
    }
    ProfileResult {
        nickname,
        avatar_url: get_str("avatar"),
    }
}

pub async fn fetch_profile(mtop: &MtopClient) -> Result<ProfileResult, PlatformError> {
    let data = mtop
        .call_get(PROFILE_API, &[], "https://www.goofish.com/")
        .await
        .map_err(|e: MtopError| e.to_platform())?;
    Ok(parse_profile(&data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_name_fallback_chain() {
        let d = json!({"module": {"base": {
            "displayName": " 主店 ", "displayNick": "别名", "avatar": "https://img/a.png"
        }}});
        let p = parse_profile(&d);
        assert_eq!(p.nickname, "主店");
        assert_eq!(p.avatar_url, "https://img/a.png");

        let d2 = json!({"module": {"base": {"displayNick": "只有别 nick", "avatar": ""}}});
        assert_eq!(parse_profile(&d2).nickname, "只有别 nick");

        assert_eq!(parse_profile(&json!({})), ProfileResult::default());
        // base 为数组等异常形态:安全空结果
        assert_eq!(
            parse_profile(&json!({"module": {"base": [1, 2]}})),
            ProfileResult::default()
        );
    }
}
