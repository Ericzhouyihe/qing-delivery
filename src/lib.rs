//! 轻交付:闲鱼虚拟商品自动发货本地管理工具。
//!
//! 分层约定(宪章 II):domain 不依赖 HTTP/SQLite/浏览器;application 定义窄仓储与
//! 平台接口;adapters 实现协议与持久化;transport 只做 DTO 与路由,不执行 SQL 或协议。

pub mod adapters;
pub mod application;
pub mod domain;
pub mod runtime;
pub mod transport;
