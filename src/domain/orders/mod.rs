//! 订单域:事实完备性模型与快照类型。

pub mod snapshot;

pub use snapshot::{FieldStatus, OrderSnapshot, SnapshotBuilder};
