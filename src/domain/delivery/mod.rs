//! 交付状态机:内容交付、人工复核、平台确认三条独立结果轴(data-model)。
//! accepted 不因重试回退;review_state=required 不能覆盖底层 unknown/not_sent 分类。

pub mod state;

pub use state::{
    ConfirmationState, ContentState, ReviewState, confirmation_transition, content_transition,
    review_from,
};
