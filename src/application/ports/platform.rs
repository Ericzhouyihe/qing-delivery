//! 平台消费者接口(platform-adapter.md §1—§2、§6、§8)。
//! 语义接口,不要求逐项拆 Trait;但不得增加万能接口或让适配器接管业务状态机。
//! 外部动作返回互斥结果变体;裸 200/socket 写成功不构成 accepted。

use crate::domain::delivery::state::{ConfirmationState, ContentState};
use crate::domain::money::Money;
use crate::domain::orders::snapshot::OrderSnapshot;
use crate::domain::sku::SkuPart;

/// 请求上下文:每个平台调用携带操作 ID、取消期限与凭证代次;
/// 外部变更另含控制代次与应用签发的执行资格(由具体方法参数表达)。
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub operation_id: String,
    pub account_id: String,
    pub credential_generation: i64,
    pub control_generation: i64,
    pub deadline_ms: i64,
}

/// 错误分类(§2):动作提交状态由 SendOutcome/ConfirmOutcome 表达,
/// 不能仅凭错误名称决定重试。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("调用前取消")]
    CancelledBeforeSubmit,
    #[error("超时")]
    Timeout,
    #[error("限流")]
    RateLimited,
    #[error("网络不可用")]
    NetworkUnavailable,
    #[error("签名 Token 过期")]
    SigningTokenExpired,
    #[error("登录会话失效")]
    SessionExpired,
    #[error("需要平台验证")]
    VerificationRequired,
    #[error("平台业务拒绝:{0}")]
    BusinessRejected(String),
    #[error("不支持的能力/类型:{0}")]
    Unsupported(String),
    #[error("响应格式异常")]
    MalformedResponse,
    #[error("事实不完整:{0}")]
    IncompleteFacts(String),
    #[error("浏览器不可用")]
    BrowserUnavailable,
    #[error("本地持久化不可用,不开始新发送")]
    PersistenceUnavailable,
}

// ---------- 规范事件(§3) ----------

/// 不可变内容快照引用:发送必须使用已保存原文及其摘要,不截断不拆分。
#[derive(Clone, Debug, PartialEq)]
pub struct ContentForSend {
    pub snapshot_id: String,
    pub text: String,
    pub text_digest: String,
}

#[derive(Clone, Debug)]
pub struct EventMeta {
    pub account_id: String,
    pub credential_generation: i64,
    pub source_event_id: Option<String>,
    pub platform_event_at_ms: Option<i64>,
    pub received_at_ms: i64,
}

#[derive(Clone, Debug)]
pub enum PlatformEvent {
    /// 授权状态变化(等待扫码/已扫码/需验证/完成/失效/取消)
    AuthorizationChanged {
        meta: EventMeta,
        state: String,
        bound_user_id: Option<String>,
    },
    /// 账号运行状态变化(授权完成 ≠ 在线)
    AccountRuntimeChanged {
        meta: EventMeta,
        state: String,
        detail: Option<String>,
    },
    /// 系统付款候选:仅触发核验,普通聊天不得转换(§3)
    OrderPaymentSignal {
        meta: EventMeta,
        order_id: String,
        buyer_hint: Option<String>,
    },
    /// 发货/退款/取消等候选状态变化
    OrderStateSignal {
        meta: EventMeta,
        order_id: String,
        platform_state: String,
    },
    /// 与一次发送严格关联的接纳证据(§6)
    OutgoingMessageEvidence {
        meta: EventMeta,
        request_id: String,
        platform_message_id: Option<String>,
        content_digest: String,
        buyer_id: Option<String>,
    },
    /// 无法可靠处理的条目/追溯缺口/协议变化
    TraceGap {
        meta: EventMeta,
        reason: String,
    },
    ProtocolIssue {
        meta: EventMeta,
        reason: String,
    },
    /// 买家聊天消息(007 D5/T045):仅买家方向(direction="2"语义)产生;
    /// 进入聊天域(ChatService::ingest),绝不触发订单/交付(§3)。
    /// 系统/交易卡片消息仍走既有系统事件路径。
    ChatMessageReceived {
        meta: EventMeta,
        chat_id: Option<String>,
        buyer_id: Option<String>,
        /// 平台消息 ID(入站去重键);适配器缺失时以稳定内容摘要兜底
        message_id: String,
        msg_kind: crate::domain::chat::ChatMsgKind,
        text: Option<String>,
        image_url: Option<String>,
        item_id: Option<String>,
    },
}

// ---------- 发送与确认结果(§6、§8) ----------

#[derive(Clone, Debug, PartialEq)]
pub struct SendProof {
    pub request_id: String,
    pub platform_message_id: Option<String>,
    pub buyer_id: String,
    pub chat_id: Option<String>,
    pub content_digest: String,
}

/// send_text 互斥结果:未提交/拒绝/接纳/未知。
/// socket 写成功、裸 200、心跳 ACK、历史同文消息均不构成 Accepted。
#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    /// 参数校验或取消证明未进入不可收回提交;仅暂时故障可按预算重试
    NotSubmitted { retryable: bool },
    /// 平台明确拒绝;永久拒绝转待处理
    Rejected { safe_code: String },
    /// 严格关联的接纳证据(当前请求 ID+买家+会话+正文摘要匹配)
    Accepted(SendProof),
    /// 可能已提交但缺完整证据:禁止自动重发与自动确认
    Unknown { hint: Option<String> },
}

/// confirm_shipment 独立结果;失败/未知绝不触发正文发送。
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmOutcome {
    NotSubmitted,
    Rejected { safe_code: String },
    Accepted,
    Unknown,
}

// ---------- 商品/订单数据 ----------

/// 聊天发送对端(007 D6):买家号必有;会话地址缺失时适配器按买家号兜底构造。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatPeer {
    pub buyer_id: String,
    /// goofish 会话地址;None → 适配器以 buyer_id 兜底(与 send_text 回退一致)
    pub chat_id: Option<String>,
}

/// 聊天消息种类:Text 即时可用;Image 由 US4(T051)实装并按能力门禁开放。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatSendKind {
    Text,
    /// 图片消息:URL 随变体携带(ContentForSend 摘要仍覆盖全文)。
    /// TODO(US4/T051):上传文件存储与 chat_send_image 能力门禁后启用。
    Image { url: String },
}

/// 适配器能力声明(007 D6/T047):transport capabilities 端点输出,
/// 前端按此门禁图片入口与历史说明(FR-042)。US3 落 chat_send_image /
/// chat_history_backfill 两项:live(闲鱼)未验证 → false;mock 如实 → true。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilitySet {
    /// 是否支持发送图片消息(否 → 前端隐藏图片入口,适配器 Image 返回 Unsupported)
    pub chat_send_image: bool,
    /// 是否支持历史会话回填(否 → 仅展示接入后收到的会话)
    pub chat_history_backfill: bool,
}

impl CapabilitySet {
    /// live/闲鱼:图片发送与历史回填均未验证,如实声明 false(T051 前不开放)。
    pub const LIVE: CapabilitySet = CapabilitySet {
        chat_send_image: false,
        chat_history_backfill: false,
    };

    /// mock:两项均实现,供开发/测试验证前端能力门禁路径。
    pub const MOCK: CapabilitySet = CapabilitySet {
        chat_send_image: true,
        chat_history_backfill: true,
    };
}


#[derive(Clone, Debug)]
pub struct ProductPage {
    pub items: Vec<ProductRecord>,
    pub next_cursor: Option<String>,
    /// 成功但省略列表内容且表达无在售商品 → 正常空页(§5)
    pub expresses_empty: bool,
}

#[derive(Clone, Debug)]
pub struct ProductRecord {
    pub external_item_id: String,
    pub title: String,
    pub on_sale: bool,
    pub sku_parts: Vec<SkuPart>,
    pub sku_complete: bool,
    pub single_sku_confirmed: bool,
}

#[derive(Clone, Debug)]
pub struct TraceReport {
    pub requested_from_ms: i64,
    pub requested_to_ms: i64,
    pub observed_from_ms: Option<i64>,
    pub observed_to_ms: Option<i64>,
    pub coverage_complete: bool,
    pub last_cursor: Option<String>,
    pub stop_reason: Option<String>,
    pub gaps: Vec<String>,
}

/// 同生命周期能力合并为一个接口;业务事务、重试预算与人工授权不在此层。
pub trait PlatformAdapter: Send + Sync {
    /// 建立授权会话,返回二维码内容/会话引用。
    fn start_authorization(
        &self,
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<AuthorizationSession, PlatformError>> + Send;

    /// 停止账号连接;必须等待后台任务结束。
    fn stop_account(
        &self,
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<(), PlatformError>> + Send;

    /// 分页商品;失败/不完整由 Err/coverage 表达,不返回伪造完整空结果。
    fn list_products(
        &self,
        ctx: &RequestContext,
        cursor: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ProductPage, PlatformError>> + Send;

    /// 协议优先的订单详情;必要时浏览器补足,browser_supplemented 标注来源。
    fn fetch_order_snapshot(
        &self,
        ctx: &RequestContext,
        external_order_id: &str,
    ) -> impl std::future::Future<Output = Result<OrderSnapshot, PlatformError>> + Send;

    /// 已售订单追溯(只读;不是发货指令)。
    fn trace_sold_orders(
        &self,
        ctx: &RequestContext,
        from_ms: i64,
        to_ms: i64,
        max_pages: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<String>, TraceReport), PlatformError>> + Send;

    /// 一次消息提交:不内建盲目重试;预算由应用保存。
    fn send_text(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        buyer_id: &str,
        content: &ContentForSend,
    ) -> impl std::future::Future<Output = Result<SendOutcome, PlatformError>> + Send;

    /// 聊天消息提交(007 D6):与 send_text 共用发送可靠性语义
    /// (SendOutcome 四分类、严格回显接纳),但不绑定订单/交付状态;
    /// 关键词/AI/默认回复与求评计划经此端口发送。
    /// 默认实现 = 未声明能力(PlatformError::Unsupported),新增实现零破坏。
    fn send_chat_message(
        &self,
        ctx: &RequestContext,
        peer: &ChatPeer,
        kind: ChatSendKind,
        content: &ContentForSend,
    ) -> impl std::future::Future<Output = Result<SendOutcome, PlatformError>> + Send {
        let _ = (ctx, peer, kind, content);
        std::future::ready(Err(PlatformError::Unsupported(
            "send_chat_message 未实现(该适配器未声明聊天发送能力)".into(),
        )))
    }

    /// 独立平台发货确认:只接受应用授予的资格与原交付证明。
    fn confirm_shipment(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        prior_proof: &SendProof,
    ) -> impl std::future::Future<Output = Result<ConfirmOutcome, PlatformError>> + Send;

    /// 只读复核当前发货状态;不能重新发送正文。
    fn verify_shipment_state(
        &self,
        ctx: &RequestContext,
        order_id: &str,
    ) -> impl std::future::Future<Output = Result<bool, PlatformError>> + Send;
}

#[derive(Clone, Debug)]
pub struct AuthorizationSession {
    pub flow_id: String,
    pub qr_content: Option<String>,
    pub expires_at_ms: i64,
}

/// 金额/数量/状态类型别名,便于实现侧对齐 domain。
pub type AmountRef = Money;
pub type ContentAxis = ContentState;
pub type ConfirmAxis = ConfirmationState;

/// 商品同步驱动(对象安全形态):`PlatformAdapter::list_products` 为 AFIT,
/// 不能造 trait 对象;transport 经本端口触发真实平台同步而不依赖具体适配器类型。
/// 实现须把失败如实分类返回,不伪造空结果(§5)。
pub trait ItemSyncDriver: Send + Sync {
    fn list_products_boxed(
        &self,
        ctx: RequestContext,
        cursor: Option<String>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProductPage, PlatformError>> + Send>,
    >;
}
