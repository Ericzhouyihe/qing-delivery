//! 发货模板域(T020/T022):占位符语法解析/校验、消息列表边界、渲染。
//! 纯业务决策:不接触 SQL/HTTP/存储;卡密真值只经调用方注入渲染上下文,
//! 本模块不落任何持久化(卡密真值只进加密快照,data-model 约定)。

use std::collections::HashMap;

/// 每模板消息条数上限(data-model:≤10 条)
pub const MAX_MESSAGES: usize = 10;
/// 单条消息长度上限(与规则内容同限,check_content 口径)
pub const MAX_MESSAGE_SCALARS: usize = 1000;
pub const MAX_MESSAGE_BYTES: usize = 4000;

/// 系统变量全集(data-model 占位符语法约定)
const SYSTEM_VARS: [&str; 4] = ["buyer_nickname", "order_id", "buyer_id", "card_name"];

/// 占位符(解析产物;cards/custom 携带变量名)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placeholder {
    BuyerNickname,
    OrderId,
    BuyerId,
    CardName,
    Cards(String),
    Custom(String),
}

impl Placeholder {
    pub fn name(&self) -> String {
        match self {
            Placeholder::BuyerNickname => "buyer_nickname".into(),
            Placeholder::OrderId => "order_id".into(),
            Placeholder::BuyerId => "buyer_id".into(),
            Placeholder::CardName => "card_name".into(),
            Placeholder::Cards(key) => format!("cards.{key}"),
            Placeholder::Custom(key) => format!("custom.{key}"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TemplateError {
    #[error("消息列表必须为 1—{MAX_MESSAGES} 条,当前 {count} 条")]
    MessageCount { count: usize },
    #[error("第 {index} 条消息为空(去除首尾空白后不得为空)")]
    EmptyMessage { index: usize },
    #[error(
        "第 {index} 条消息超限:{scalars} 标量/上限 {MAX_MESSAGE_SCALARS},{bytes} 字节/上限 {MAX_MESSAGE_BYTES};不截断"
    )]
    MessageTooLong {
        index: usize,
        scalars: usize,
        bytes: usize,
    },
    #[error("第 {index} 条消息第 {column} 字符处占位符非法:{reason}")]
    BadPlaceholder {
        index: usize,
        column: usize,
        reason: String,
    },
    #[error("变量 {{{name}}} 未提供取值(卡密绑定或自定义赋值缺失)")]
    Unresolved { name: String },
}

/// 变量名合法性:key 限 `[A-Za-z0-9_-]+`(data-model 约定,至少一个字符)。
fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("cards./custom. 后缺少变量名".to_string());
    }
    if key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(format!(
            "变量名“{key}”含非法字符(仅限字母/数字/下划线/连字符)"
        ))
    }
}

/// 解析变量名 → 占位符;未知系统变量/非法 key 返回可读原因。
fn parse_var(name: &str) -> Result<Placeholder, String> {
    if let Some(key) = name.strip_prefix("cards.") {
        return check_key(key).map(|_| Placeholder::Cards(key.to_string()));
    }
    if let Some(key) = name.strip_prefix("custom.") {
        return check_key(key).map(|_| Placeholder::Custom(key.to_string()));
    }
    match name {
        "buyer_nickname" => Ok(Placeholder::BuyerNickname),
        "order_id" => Ok(Placeholder::OrderId),
        "buyer_id" => Ok(Placeholder::BuyerId),
        "card_name" => Ok(Placeholder::CardName),
        other => Err(format!(
            "未知变量“{other}”(可用:{}/{}/{}/{}/cards.<key>/custom.<key>)",
            SYSTEM_VARS[0], SYSTEM_VARS[1], SYSTEM_VARS[2], SYSTEM_VARS[3]
        )),
    }
}

/// 解析单条消息占位符;失败返回 (1-based 字符列, 原因)。
/// 纯文本(无 `{{`)合法;`{{` 未闭合/内部非法均报错并定位到起始列。
fn parse_body(body: &str) -> Result<Vec<Placeholder>, (usize, String)> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '{' {
            let start = i;
            // 从 "{{" 之后找首个 "}}" 作为配对闭合
            let mut j = i + 2;
            while j < chars.len()
                && !(chars[j] == '}' && j + 1 < chars.len() && chars[j + 1] == '}')
            {
                j += 1;
            }
            if j >= chars.len() {
                return Err((start + 1, "占位符未闭合(缺少 }})".to_string()));
            }
            let inner: String = chars[i + 2..j].iter().collect();
            let trimmed = inner.trim();
            if trimmed.is_empty() {
                return Err((start + 1, "占位符为空".to_string()));
            }
            let ph = parse_var(trimmed).map_err(|reason| (start + 1, reason))?;
            out.push(ph);
            i = j + 2;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// 校验模板消息列表(保存与执行前同源):1..=MAX_MESSAGES 条、
/// 单条 trim 后非空、单条 ≤1000 unicode 标量/≤4000 UTF-8 字节、占位符合法。
pub fn validate_messages(messages: &[String]) -> Result<(), TemplateError> {
    if messages.is_empty() || messages.len() > MAX_MESSAGES {
        return Err(TemplateError::MessageCount {
            count: messages.len(),
        });
    }
    for (i, body) in messages.iter().enumerate() {
        let index = i + 1;
        if body.trim().is_empty() {
            return Err(TemplateError::EmptyMessage { index });
        }
        let scalars = body.chars().count();
        let bytes = body.len();
        if scalars > MAX_MESSAGE_SCALARS || bytes > MAX_MESSAGE_BYTES {
            return Err(TemplateError::MessageTooLong {
                index,
                scalars,
                bytes,
            });
        }
        if let Err((column, reason)) = parse_body(body) {
            return Err(TemplateError::BadPlaceholder {
                index,
                column,
                reason,
            });
        }
    }
    Ok(())
}

/// 模板引用的变量名(cards/custom 分组,按出现顺序去重)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateKeys {
    pub cards: Vec<String>,
    pub custom: Vec<String>,
}

/// 有序去重追加(保持首次出现顺序)。
fn push_unique(list: &mut Vec<String>, key: String) {
    if !list.contains(&key) {
        list.push(key);
    }
}

/// 提取消息列表全部占位符变量名(假定已通过 validate_messages;
/// 无法解析的占位符跳过,不失败)。
pub fn extract_keys(messages: &[String]) -> TemplateKeys {
    let mut keys = TemplateKeys::default();
    for body in messages {
        for ph in parse_body(body).unwrap_or_default() {
            match ph {
                Placeholder::Cards(key) => push_unique(&mut keys.cards, key),
                Placeholder::Custom(key) => push_unique(&mut keys.custom, key),
                _ => {}
            }
        }
    }
    keys
}

/// 卡密变量取值:真值 + 张数(掩码模式显示 [卡密内容 ×N])。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderCard {
    /// 卡密真值(多张按预留顺序换行拼接);只进加密快照,永不落模板表
    pub text: String,
    /// 该变量预留的卡密张数(掩码计数)
    pub count: usize,
}

/// 渲染上下文:渲染函数只接收已取到的值(取卡发生在 freeze 阶段,T024;
/// 预览端点注入样例值 + 掩码,与实际发送共用同一函数)。
#[derive(Clone, Debug, Default)]
pub struct RenderContext {
    pub buyer_nickname: String,
    pub order_id: String,
    pub buyer_id: String,
    pub card_name: String,
    pub cards: HashMap<String, RenderCard>,
    pub custom: HashMap<String, String>,
}

/// 渲染单条消息:mask=true 时卡密值替换为 [卡密内容 ×N],其余取真值;
/// 引用的变量缺失取值 → Unresolved。
pub fn render_message(
    body: &str,
    ctx: &RenderContext,
    mask: bool,
) -> Result<String, TemplateError> {
    render_indexed(body, ctx, mask, 0)
}

/// 渲染全部消息(顺序保持;错误携带 1-based 消息序号)。
pub fn render_messages(
    messages: &[String],
    ctx: &RenderContext,
    mask: bool,
) -> Result<Vec<String>, TemplateError> {
    messages
        .iter()
        .enumerate()
        .map(|(i, body)| render_indexed(body, ctx, mask, i + 1))
        .collect()
}

fn render_indexed(
    body: &str,
    ctx: &RenderContext,
    mask: bool,
    index: usize,
) -> Result<String, TemplateError> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '{' {
            let start = i;
            let mut j = i + 2;
            while j < chars.len() && !(chars[j] == '}' && j + 1 < chars.len() && chars[j + 1] == '}')
            {
                j += 1;
            }
            if j >= chars.len() {
                return Err(TemplateError::BadPlaceholder {
                    index,
                    column: start + 1,
                    reason: "占位符未闭合(缺少 }})".to_string(),
                });
            }
            let inner: String = chars[i + 2..j].iter().collect();
            let name = inner.trim();
            if name.is_empty() {
                return Err(TemplateError::BadPlaceholder {
                    index,
                    column: start + 1,
                    reason: "占位符为空".to_string(),
                });
            }
            let ph = parse_var(name).map_err(|reason| TemplateError::BadPlaceholder {
                index,
                column: start + 1,
                reason,
            })?;
            let value = match &ph {
                Placeholder::BuyerNickname => ctx.buyer_nickname.clone(),
                Placeholder::OrderId => ctx.order_id.clone(),
                Placeholder::BuyerId => ctx.buyer_id.clone(),
                Placeholder::CardName => ctx.card_name.clone(),
                Placeholder::Cards(key) => {
                    let card = ctx.cards.get(key).ok_or_else(|| TemplateError::Unresolved {
                        name: ph.name(),
                    })?;
                    if mask {
                        format!("[卡密内容 ×{}]", card.count)
                    } else {
                        card.text.clone()
                    }
                }
                Placeholder::Custom(key) => ctx.custom.get(key).ok_or_else(|| {
                    TemplateError::Unresolved { name: ph.name() }
                })?
                .clone(),
            };
            out.push_str(&value);
            i = j + 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// rules.template_bindings / rule_variants.template_bindings 的 JSON 结构
/// (data-model:`{cards:[{key,pool_id,units}], custom:{k:v}}`)。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TemplateBindings {
    #[serde(default)]
    pub cards: Vec<CardBinding>,
    #[serde(default)]
    pub custom: HashMap<String, String>,
}

/// 单个卡密变量绑定:变量名 → 卡密组 × 每件份数。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CardBinding {
    pub key: String,
    pub pool_id: String,
    #[serde(default = "default_units")]
    pub units: i64,
}

fn default_units() -> i64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx() -> RenderContext {
        let mut cards = HashMap::new();
        cards.insert(
            "key1".to_string(),
            RenderCard {
                text: "CARD-A\nCARD-B".into(),
                count: 2,
            },
        );
        let mut custom = HashMap::new();
        custom.insert("note".to_string(), "谢谢支持".into());
        RenderContext {
            buyer_nickname: "小鱼干".into(),
            order_id: "ORD-77".into(),
            buyer_id: "buyer-9".into(),
            card_name: "考研资料".into(),
            cards,
            custom,
        }
    }

    #[test]
    fn 合法占位符全类型解析与keys提取() {
        let msgs = vec![
            "{{buyer_nickname}}购买{{card_name}}".to_string(),
            "{{order_id}}/{{buyer_id}}".to_string(),
            "{{cards.key_1}}与{{custom.note}}".to_string(),
            "纯文本消息,无占位符".to_string(),
        ];
        assert!(validate_messages(&msgs).is_ok(), "全部占位符类型应合法");
        let keys = extract_keys(&msgs);
        assert_eq!(keys.cards, vec!["key_1".to_string()]);
        assert_eq!(keys.custom, vec!["note".to_string()]);
    }

    #[test]
    fn 未知系统变量报错并定位() {
        let err = validate_messages(&["你好{{buyer}}!".to_string()]).unwrap_err();
        match err {
            TemplateError::BadPlaceholder {
                index,
                column,
                reason,
            } => {
                assert_eq!(index, 1, "第 1 条消息");
                assert_eq!(column, 3, "你好之后第 3 字符起");
                assert!(reason.contains("未知变量"), "应指出未知变量:{reason}");
            }
            other => panic!("应报占位符错误,实际 {other:?}"),
        }
    }

    #[test]
    fn 坏括号与非法key报错并定位() {
        // key 含空格(quickstart US2-2 用例)
        let err = validate_messages(&["{{cards.非 法}}".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::BadPlaceholder {
                index: 1,
                column: 1,
                ..
            }
        ));
        // 未闭合
        let err = validate_messages(&["前缀{{order_id".to_string()]).unwrap_err();
        match err {
            TemplateError::BadPlaceholder { column: 3, reason, .. } => {
                assert!(reason.contains("未闭合"), "{reason}");
            }
            other => panic!("应报未闭合,实际 {other:?}"),
        }
        // 空占位符
        assert!(matches!(
            validate_messages(&["{{}}".to_string()]).unwrap_err(),
            TemplateError::BadPlaceholder { index: 1, column: 1, .. }
        ));
        // cards. 后缺少 key
        assert!(validate_messages(&["{{cards.}}".to_string()]).is_err());
        // key 含中文/点号拒绝(data-model:key 限 [A-Za-z0-9_-])
        assert!(validate_messages(&["{{custom.备注}}".to_string()]).is_err());
        assert!(validate_messages(&["{{cards.a.b}}".to_string()]).is_err());
        // 首尾空白容错:{{ order_id }} 合法
        assert!(validate_messages(&["{{ order_id }}".to_string()]).is_ok());
    }

    #[test]
    fn 消息列表边界() {
        // 0 条拒绝
        assert!(matches!(
            validate_messages(&[]).unwrap_err(),
            TemplateError::MessageCount { count: 0 }
        ));
        // 11 条拒绝;10 条合法
        let eleven: Vec<String> = (0..11).map(|i| format!("消息{i}")).collect();
        assert!(matches!(
            validate_messages(&eleven).unwrap_err(),
            TemplateError::MessageCount { count: 11 }
        ));
        let ten: Vec<String> = (0..10).map(|i| format!("消息{i}")).collect();
        assert!(validate_messages(&ten).is_ok());
        // trim 后为空拒绝
        assert!(matches!(
            validate_messages(&["   ".to_string()]).unwrap_err(),
            TemplateError::EmptyMessage { index: 1 }
        ));
        // 1000 标量合法;1001 拒绝(1000×4 字节=4000,标量先触界)
        let ok_len = "a".repeat(1000);
        assert!(validate_messages(&[ok_len]).is_ok());
        let bad = "a".repeat(1001);
        assert!(matches!(
            validate_messages(&[bad]).unwrap_err(),
            TemplateError::MessageTooLong {
                index: 1,
                scalars: 1001,
                ..
            }
        ));
    }

    #[test]
    fn 渲染真值与掩码() {
        let messages = vec![
            "感谢{{buyer_nickname}}购买{{card_name}}".to_string(),
            "您的卡密:{{cards.key1}}".to_string(),
            "订单{{order_id}}备查,备注:{{custom.note}}".to_string(),
        ];
        let real = render_messages(&messages, &ctx(), false).unwrap();
        assert_eq!(
            real,
            vec![
                "感谢小鱼干购买考研资料".to_string(),
                "您的卡密:CARD-A\nCARD-B".to_string(),
                "订单ORD-77备查,备注:谢谢支持".to_string(),
            ]
        );
        // 掩码:仅卡密值替换为 [卡密内容 ×N],其余逐字一致(预览=发送,除掩码外)
        let masked = render_messages(&messages, &ctx(), true).unwrap();
        assert_eq!(
            masked,
            vec![
                "感谢小鱼干购买考研资料".to_string(),
                "您的卡密:[卡密内容 ×2]".to_string(),
                "订单ORD-77备查,备注:谢谢支持".to_string(),
            ]
        );
    }

    #[test]
    fn 渲染缺值报错() {
        let err = render_messages(&["{{cards.missing}}".to_string()], &ctx(), false).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::Unresolved { ref name } if name == "cards.missing"
        ));
        let err = render_messages(&["{{custom.missing}}".to_string()], &ctx(), false).unwrap_err();
        assert!(matches!(
            err,
            TemplateError::Unresolved { ref name } if name == "custom.missing"
        ));
    }

    #[test]
    fn 绑定json往返与缺省() {
        let raw = r#"{"cards":[{"key":"key1","pool_id":"pool-1","units":2}],"custom":{"note":"谢谢"}}"#;
        let b: TemplateBindings = serde_json::from_str(raw).unwrap();
        assert_eq!(b.cards.len(), 1);
        assert_eq!(b.cards[0].key, "key1");
        assert_eq!(b.cards[0].pool_id, "pool-1");
        assert_eq!(b.cards[0].units, 2);
        assert_eq!(b.custom.get("note").map(String::as_str), Some("谢谢"));
        // units 缺省 1;空对象合法
        let b: TemplateBindings =
            serde_json::from_str(r#"{"cards":[{"key":"k","pool_id":"p"}]}"#).unwrap();
        assert_eq!(b.cards[0].units, 1);
        assert_eq!(TemplateBindings::default(), serde_json::from_str("{}").unwrap());
        // 往返
        let s = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<TemplateBindings>(&s).unwrap(), b);
    }

    #[test]
    fn keys提取去重保持出现顺序() {
        let messages = vec![
            "{{cards.b}} {{cards.a}}".to_string(),
            "{{custom.z}} {{cards.b}} {{custom.a}}".to_string(),
        ];
        let keys = extract_keys(&messages);
        assert_eq!(keys.cards, vec!["b".to_string(), "a".to_string()]);
        assert_eq!(keys.custom, vec!["z".to_string(), "a".to_string()]);
    }
}
