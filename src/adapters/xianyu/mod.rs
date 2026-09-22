//! 闲鱼适配器:协议签名、同步帧解码与订单/发送通道。
//! 真实平台兼容性以 SC-009 授权实单矩阵为准;确定性验证走本地假服务。

pub mod codec;
pub mod mtop;
pub mod ws;
