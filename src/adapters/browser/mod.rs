//! 浏览器边界(003):按需启动的本机 Edge/Chrome 专用受限 profile
//! (不读用户日常 profile);驱动安全验证自动化(D1/D4/D7)。

pub mod cdp;
pub mod discover;
pub mod manager;
pub mod slider;

pub use manager::{BrowserManager, BrowserUnavailable};
