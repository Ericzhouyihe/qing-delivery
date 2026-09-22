//! 进程内模拟适配器与开发场景(dev-fixtures feature 专用)。
//! 仅开发/测试构建存在;发行版禁止启用(cli.md mock profile)。
//! 禁止访问真实平台网络、真实二维码/Cookie/Token、live 数据目录与用户浏览器 profile。

/// 固定场景 allowlist:不接收任意 JS、URL、文件路径或原始凭证。
pub const SCENARIOS: &[&str] = &[
    "ordinary_payment", // 普通付款交付
    "multi_sku",        // 完整多规格匹配
    "duplicate_events", // 重复/乱序事件去重
    "send_unknown",     // 发送结果未知
    "cancel_refund",    // 取消/退款事实
    "restore_gap",      // 恢复缺口
];

pub fn is_allowed(scenario: &str) -> bool {
    SCENARIOS.contains(&scenario)
}

/// 进程内假适配器:集成测试与 dev 场景共用。
/// 始终编译(集成测试无法启用 lib feature),但除测试/mock profile 外无构造路径;
/// 发行版的路由挂载仍由 dev-fixtures feature 门控(transport::dev)。
pub mod fake;
