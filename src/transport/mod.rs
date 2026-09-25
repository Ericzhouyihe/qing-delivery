//! HTTP 传输层:DTO、路由、中间件与嵌入式静态页面。
//! 本层不执行 SQL、不解析平台协议(宪章 II);所有业务语义来自应用层。

pub mod accounts_api;
pub mod cards_api;
pub mod catalog_api;
pub mod dashboard_api;
#[cfg(feature = "dev-fixtures")]
pub mod dev;
pub mod error;
pub mod manual_api;
pub mod middleware;
pub mod orders_api;
pub mod restore_api;
pub mod routes;
pub mod state;
pub mod stats_api;
pub mod webui;
