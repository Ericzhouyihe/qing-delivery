//! 适配器:平台协议、浏览器、持久化与 Windows 系统能力。
//! 平台适配器不得直接写业务表或决定业务规则(宪章 II)。

pub mod browser;
pub mod mock;
pub mod sqlite;
pub mod windows;
pub mod xianyu;
