//! per-account 交付闸门(研究 D5):验证会话活跃期间延迟该账号新的交付触发;
//! 不触碰 account_control 持久化开关语义(FR-013)。

use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Default)]
pub struct DeliveryGate {
    held: Mutex<HashSet<String>>,
}

impl DeliveryGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// 会话开始:关闭该账号闸门。
    pub fn hold(&self, account_id: &str) {
        self.held.lock().unwrap().insert(account_id.to_string());
    }

    /// 会话终态:释放闸门。
    pub fn release(&self, account_id: &str) {
        self.held.lock().unwrap().remove(account_id);
    }

    /// handoff 前检查:true = 允许交付。
    pub fn allows(&self, account_id: &str) -> bool {
        !self.held.lock().unwrap().contains(account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 闸门只影响持门账号() {
        let g = DeliveryGate::new();
        assert!(g.allows("a1"));
        g.hold("a1");
        assert!(!g.allows("a1"));
        assert!(g.allows("a2"), "其他账号不受影响");
        g.release("a1");
        assert!(g.allows("a1"));
    }
}
