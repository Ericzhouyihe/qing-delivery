//! 浏览器边界(T050/T051):按需启动的本机 Edge/Chrome 专用受限 profile。
//! 不读取用户日常 profile;不自动滑块/绕过验证;缺浏览器返回明确不可用。

pub mod manager;

pub use manager::{BrowserManager, BrowserUnavailable};
