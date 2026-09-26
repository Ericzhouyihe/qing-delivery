//! 集成测试汇总入口:cargo 仅自动发现 tests/ 顶层与目录 main.rs。

#[path = "../support/mod.rs"]
mod support;

mod cards;
mod catalog_rules;
mod chat;
mod delivery_core;
mod notify;
mod orders_sync;
mod query_backup;
mod recovery_manual;
mod replies;
mod rules_ext;
mod settings_ai;
mod templates;
mod ws_lifecycle;
