#![allow(clippy::too_many_arguments)]
//! 仓储函数:应用层经 DbThread 闭包调用的窄 SQL 边界。
//! SQL 只出现在本目录(宪章 II:transport/application 不执行 SQL)。

pub mod accounts;
pub mod admin;
pub mod auth_flows;
pub mod credentials;
pub mod deliveries;
pub mod inbound;
pub mod issues;
pub mod items;
pub mod jobs;
pub mod manual_actions;
pub mod orders;
pub mod rules;
pub mod sessions;
