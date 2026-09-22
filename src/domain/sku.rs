//! SKU 规范键:完整属性键值对按平台属性 ID/值 ID 排序生成规范键。
//! 缺任一维度标 incomplete,不能与单规格空组合混同(data-model);
//! 首版不做子串、标题或部分规格匹配。

/// 单规格事实键:仅在平台明确单规格时使用。
pub const SKU_KEY_SINGLE: &str = "single";
/// 规格不完整/未知键:不能进入完整匹配。
pub const SKU_KEY_INCOMPLETE: &str = "incomplete";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkuPart {
    pub property_id: String,
    pub value_id: String,
    pub property_label: String,
    pub value_label: String,
}

fn escape_component(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace('=', "\\=")
}

/// 组合键:按 (property_id, value_id) 排序后 `p=v;p=v`,维度重复即冲突。
pub fn combo_key(parts: &[SkuPart]) -> Result<String, SkuError> {
    let mut sorted: Vec<&SkuPart> = parts.iter().collect();
    sorted.sort_by(|a, b| {
        (a.property_id.as_str(), a.value_id.as_str())
            .cmp(&(b.property_id.as_str(), b.value_id.as_str()))
    });
    for pair in sorted.windows(2) {
        if pair[0].property_id == pair[1].property_id {
            return Err(SkuError::DuplicateProperty(pair[0].property_id.clone()));
        }
    }
    Ok(format!(
        "combo:{}",
        sorted
            .iter()
            .map(|p| format!(
                "{}={}",
                escape_component(&p.property_id),
                escape_component(&p.value_id)
            ))
            .collect::<Vec<_>>()
            .join(";")
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum SkuError {
    #[error("规格维度重复:{0}")]
    DuplicateProperty(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(pid: &str, vid: &str) -> SkuPart {
        SkuPart {
            property_id: pid.into(),
            value_id: vid.into(),
            property_label: String::new(),
            value_label: String::new(),
        }
    }

    #[test]
    fn combo_key_is_order_independent() {
        let a = combo_key(&[part("10", "20"), part("2", "3")]).unwrap();
        let b = combo_key(&[part("2", "3"), part("10", "20")]).unwrap();
        assert_eq!(a, b);
        // 属性 ID 按字典序排序,保证稳定比较
        assert!(a.starts_with("combo:10=20;2=3"));
    }

    #[test]
    fn duplicate_property_is_rejected() {
        assert!(combo_key(&[part("1", "1"), part("1", "2")]).is_err());
    }

    #[test]
    fn escaped_components_do_not_collide() {
        let a = combo_key(&[part("a;b", "c")]).unwrap();
        let b = combo_key(&[part("a", "b;c")]).unwrap();
        assert_ne!(a, b);
    }
}
