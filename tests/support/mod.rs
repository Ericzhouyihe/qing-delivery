//! 测试支撑模块:本地假平台服务、脱敏合成协议夹具与契约测试应用。
//! 仅测试/开发构建使用;不使用真实域名、真实 Cookie 或生产数据目录。
//! 不同测试 crate 只消费其中一部分,整体允许未引用项。

#![allow(dead_code)]

pub mod app;
pub mod fake_platform;
pub mod fixtures;
