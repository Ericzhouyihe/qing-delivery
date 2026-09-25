//! 应用层:账号、认证、任务幂等等用例;通过窄仓储函数访问持久化,
//! 通过消费者接口访问平台(宪章 II)。本层不写裸 SQL。

pub mod accounts;
pub mod auth;
pub mod backup;
pub mod cards;
pub mod catalog;
pub mod delivery;
pub mod events;
pub mod idempotency;
pub mod jobs;
pub mod manual;
pub mod ports;
pub mod recovery;
pub mod stats;
pub mod verification;
