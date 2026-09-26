//! 纯业务决策与值类型:身份、时间、金额、SKU 规范键、加密信封格式、
//! 订单事实完备性与交付状态机。
//! 本模块不得依赖 HTTP、SQLite 或浏览器(宪章 II)。

pub mod accounts;
pub mod cards;
pub mod chat;
pub mod crypto;
pub mod delivery;
pub mod ids;
pub mod money;
pub mod notify;
pub mod orders;
pub mod rules_ext;
pub mod sku;
pub mod stats;
pub mod templates;
pub mod time_util;
