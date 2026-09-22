//! 备份与恢复(T079/T080):停机独占 CLI 语义以库函数实现,
//! serve 与 CLI 共用;加密归档 + DPAPI 数据密钥 + 恢复隔离。

pub mod archive;
pub mod restore;
