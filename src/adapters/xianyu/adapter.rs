//! 闲鱼平台适配器(T041/T044/T045 汇聚):实现 PlatformAdapter。
//! 协议细节全部封装于此(宪章 II);不写业务表、不决定规则。
//! 会话由运行时挂载:凭证 Jar + WS 句柄 + 订单→会话绑定(来自付款事件的 chatID)。

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};

use crate::adapters::xianyu::auth::qr::{QrCodeMaterial, QrLogin, QrStatus};
use crate::adapters::xianyu::cookies::CookieJar;
use crate::adapters::xianyu::events::{self, MessageKind};
use crate::adapters::xianyu::mtop::client::{MtopClient, MtopError};
use crate::adapters::xianyu::ws::real::{
    self, SessionEnd, SyncMessage, WsHandle, generate_msg_uuid,
};
use crate::application::ports::platform::{
    AuthorizationSession, ConfirmOutcome, ContentForSend, EventMeta, PlatformAdapter,
    PlatformError, ProductPage, ProductRecord, RequestContext, SendOutcome, SendProof, TraceReport,
};
use crate::domain::money::Money;
use crate::domain::orders::snapshot::{FieldStatus, OrderSnapshot, PlatformOrderState, TradeType};
use crate::domain::time_util::utc_now_ms;

/// 会话绑定:付款事件携带的 chatID/买家,供发送时解析会话
#[derive(Clone, Debug)]
pub struct ChatBinding {
    pub chat_id: String,
    pub buyer_id: Option<String>,
}

pub struct AccountSession {
    pub jar: Arc<tokio::sync::Mutex<CookieJar>>,
    pub external_id: String,
    pub device_id: String,
    pub credential_generation: i64,
    pub chats: Arc<std::sync::Mutex<HashMap<String, ChatBinding>>>,
    pub ws: Option<WsHandle>,
}

pub enum QrPollResult {
    New,
    Scanned,
    Confirmed {
        unb: String,
        cookie_jar_json: String,
    },
    Expired,
    Canceled,
    Verification {
        url: Option<String>,
    },
    Failed(String),
}

struct Inner {
    h5_base: String,
    passport_base: String,
    im_url: String,
    ws_url: String,
    sessions: tokio::sync::Mutex<HashMap<String, AccountSession>>,
    qr_logins: tokio::sync::Mutex<HashMap<String, QrLogin>>,
}

#[derive(Clone)]
pub struct XianyuAdapter {
    inner: Arc<Inner>,
}

impl Default for XianyuAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl XianyuAdapter {
    pub fn new() -> Self {
        Self::with_endpoints(
            crate::adapters::xianyu::mtop::client::DEFAULT_H5_BASE,
            crate::adapters::xianyu::auth::qr::DEFAULT_PASSPORT_BASE,
            crate::adapters::xianyu::auth::qr::DEFAULT_IM_URL,
            real::WS_URL,
        )
    }

    /// 夹具注入端点。
    pub fn with_endpoints(h5_base: &str, passport_base: &str, im_url: &str, ws_url: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                h5_base: h5_base.to_string(),
                passport_base: passport_base.to_string(),
                im_url: im_url.to_string(),
                ws_url: ws_url.to_string(),
                sessions: tokio::sync::Mutex::new(HashMap::new()),
                qr_logins: tokio::sync::Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 运行时挂载账号会话(凭证已由运行时解密);同一账号重复挂载覆盖旧会话。
    pub async fn attach(
        &self,
        account_id: &str,
        cookie_jar_json: &str,
        external_id: &str,
        credential_generation: i64,
    ) -> Result<(), PlatformError> {
        let jar = CookieJar::from_json(cookie_jar_json).ok_or(PlatformError::SessionExpired)?;
        let device_id = generate_device_id(external_id);
        let mut sessions = self.inner.sessions.lock().await;
        // 保留已积累的订单→会话绑定(重连不丢)
        let chats = sessions
            .get(account_id)
            .map(|s| s.chats.clone())
            .unwrap_or_default();
        sessions.insert(
            account_id.to_string(),
            AccountSession {
                jar: Arc::new(tokio::sync::Mutex::new(jar)),
                external_id: external_id.to_string(),
                device_id,
                credential_generation,
                chats,
                ws: None,
            },
        );
        Ok(())
    }

    pub async fn detach(&self, account_id: &str) {
        self.inner.sessions.lock().await.remove(account_id);
    }

    async fn session(&self, account_id: &str) -> Result<AccountSessionClone, PlatformError> {
        let guard = self.inner.sessions.lock().await;
        let session = guard.get(account_id).ok_or(PlatformError::SessionExpired)?;
        Ok(AccountSessionClone {
            jar: session.jar.clone(),
            external_id: session.external_id.clone(),
            device_id: session.device_id.clone(),
            credential_generation: session.credential_generation,
            chats: session.chats.clone(),
        })
    }

    async fn mtop(&self, session: &AccountSessionClone) -> MtopClient {
        MtopClient::with_base(session.jar.clone(), &self.inner.h5_base)
    }

    /// 建立 WS 连接:取 token → 连接/注册;返回句柄与任务 JoinHandle。
    /// 入站消息经 events_tx 转发为规范事件(提取在此完成)。
    pub async fn connect_ws(
        &self,
        account_id: &str,
        events_tx: tokio::sync::mpsc::Sender<crate::application::ports::platform::PlatformEvent>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(WsHandle, tokio::task::JoinHandle<SessionEnd>), PlatformError> {
        let session = self.session(account_id).await?;
        let mtop = self.mtop(&session).await;
        let token = mtop
            .ws_access_token(&session.device_id)
            .await
            .map_err(|e| e.to_platform())?;

        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(32);
        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<SyncMessage>(256);
        let params = real::SessionParams {
            account_id: account_id.to_string(),
            credential_generation: session.credential_generation,
            access_token: token.token,
            device_id: session.device_id.clone(),
            ws_url: self.inner.ws_url.clone(),
        };
        let handle_task = tokio::spawn(real::run_registered_session(
            params, msg_tx, cmd_rx, shutdown,
        ));

        // 消息泵:解码载荷 → 提取事件 → 更新订单会话绑定 → 转发
        let chats = session.chats.clone();
        let account = account_id.to_string();
        let generation = session.credential_generation;
        tokio::spawn(async move {
            while let Some(sync) = msg_rx.recv().await {
                match sync.inbound {
                    real::Inbound::Message(msg) => {
                        let extracted = events::extract(&msg);
                        if let (Some(chat), Some(order)) =
                            (extracted.chat_id.clone(), order_of(&extracted.kind))
                        {
                            chats.lock().unwrap().insert(
                                order,
                                ChatBinding {
                                    chat_id: chat,
                                    buyer_id: extracted.buyer_id.clone(),
                                },
                            );
                        }
                        for event in events::extract_events(&msg, &account, generation) {
                            if events_tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                    real::Inbound::DecodeError(reason) => {
                        let _ = events_tx
                            .send(
                                crate::application::ports::platform::PlatformEvent::ProtocolIssue {
                                    meta: EventMeta {
                                        account_id: account.clone(),
                                        credential_generation: generation,
                                        source_event_id: None,
                                        platform_event_at_ms: None,
                                        received_at_ms: utc_now_ms(),
                                    },
                                    reason,
                                },
                            )
                            .await;
                    }
                }
            }
        });

        let handle = WsHandle::new(cmd_tx);
        let mut sessions = self.inner.sessions.lock().await;
        if let Some(s) = sessions.get_mut(account_id) {
            s.ws = Some(handle.clone());
        }
        Ok((handle, handle_task))
    }

    // ---------- 扫码登录驱动(supervisor/HTTP 层调用) ----------

    pub async fn qr_create(&self) -> Result<(String, QrCodeMaterial), PlatformError> {
        let mut login = QrLogin::new(&self.inner.passport_base, &self.inner.im_url);
        let material = login.generate().await.map_err(qr_error)?;
        let flow_id = crate::domain::ids::new_request_key();
        self.inner
            .qr_logins
            .lock()
            .await
            .insert(flow_id.clone(), login);
        Ok((flow_id, material))
    }

    pub async fn qr_poll(&self, flow_id: &str) -> Result<QrPollResult, PlatformError> {
        let status = {
            let logins = self.inner.qr_logins.lock().await;
            let login = logins
                .get(flow_id)
                .ok_or(PlatformError::MalformedResponse)?;
            login.poll().await.map_err(qr_error)?
        };
        Ok(match status {
            QrStatus::New => QrPollResult::New,
            QrStatus::Scanned => QrPollResult::Scanned,
            QrStatus::Confirmed => {
                let login = self
                    .inner
                    .qr_logins
                    .lock()
                    .await
                    .remove(flow_id)
                    .ok_or(PlatformError::MalformedResponse)?;
                let confirmed = login.complete().await.map_err(qr_error)?;
                QrPollResult::Confirmed {
                    unb: confirmed.unb,
                    cookie_jar_json: confirmed.cookie_jar_json,
                }
            }
            QrStatus::Expired => QrPollResult::Expired,
            QrStatus::Canceled => QrPollResult::Canceled,
            QrStatus::Verification { url } => QrPollResult::Verification { url },
            QrStatus::Failed(m) => QrPollResult::Failed(m),
        })
    }

    // ---------- mtop 业务封装 ----------

    async fn order_detail(
        &self,
        session: &AccountSessionClone,
        external_order_id: &str,
    ) -> Result<Value, PlatformError> {
        let mtop = self.mtop(session).await;
        let data = json!({"tid": external_order_id}).to_string();
        let referer =
            format!("https://www.goofish.com/order-detail?orderId={external_order_id}&role=seller");
        mtop.call("mtop.idle.web.trade.order.detail", &data, &referer)
            .await
            .map_err(|e| e.to_platform())
    }
}

fn qr_error(e: crate::adapters::xianyu::auth::qr::QrError) -> PlatformError {
    use crate::adapters::xianyu::auth::qr::QrError;
    match e {
        QrError::Network(_) => PlatformError::NetworkUnavailable,
        QrError::MissingAccountIdentity => PlatformError::MalformedResponse,
        other => PlatformError::BusinessRejected(other.to_string()),
    }
}

fn order_of(kind: &MessageKind) -> Option<String> {
    match kind {
        MessageKind::PaidOrder { order_id, .. } => Some(order_id.clone()),
        MessageKind::OrderCompleted { order_id } => Some(order_id.clone()),
        MessageKind::Other => None,
    }
}

#[derive(Clone)]
struct AccountSessionClone {
    jar: Arc<tokio::sync::Mutex<CookieJar>>,
    external_id: String,
    device_id: String,
    credential_generation: i64,
    chats: Arc<std::sync::Mutex<HashMap<String, ChatBinding>>>,
}

/// 设备 ID:36 位 UUID 形态 + "-<unB>";仅凭证换代时重建(页面级轮换语义)。
pub fn generate_device_id(user_id: &str) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    let mut body = [0u8; 32];
    for slot in body.iter_mut() {
        *slot = CHARSET[rng.gen_range(0..CHARSET.len())];
    }
    body[12] = b'4'; // 版本位
    body[16] = b"89AB"[rng.gen_range(0..4)]; // 变体位
    let s: String = body.iter().map(|b| *b as char).collect();
    format!(
        "{}-{}-{}-{}-{}-{user_id}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

/// 中文订单状态 → 规范状态(上游 normalizeSoldOrderStatus 行为)。
pub fn normalize_order_status(raw: &str, in_refund: bool) -> PlatformOrderState {
    if in_refund {
        return PlatformOrderState::Refunding;
    }
    match raw {
        "待付款" | "未付款" => PlatformOrderState::Unpaid,
        "待发货" | "已付款" | "等待发货" | "等待卖家发货" => {
            PlatformOrderState::PendingShip
        }
        "已发货" | "卖家已发货" => PlatformOrderState::Shipped,
        "交易成功" | "已完成" | "已完成(款已结算)" => PlatformOrderState::Completed,
        "退款中" | "退款处理中" => PlatformOrderState::Refunding,
        "退款成功" | "已退款" => PlatformOrderState::Refunded,
        "交易关闭" | "已关闭" | "退款关闭" => PlatformOrderState::Canceled,
        _ => PlatformOrderState::Unknown,
    }
}

/// 元金额字符串("12.34")→ 分;解析失败返回 None(金额缺失不当 0)。
pub fn yuan_to_minor(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let mut minor: i64 = int_part.parse().ok()?;
    for (i, c) in frac_part.chars().enumerate() {
        if i >= 2 {
            break;
        }
        let digit = c.to_digit(10)?;
        minor = minor.checked_mul(10)?.checked_add(digit as i64)?;
    }
    match frac_part.chars().count() {
        0 => minor.checked_mul(100),
        1 => minor.checked_mul(10),
        _ => Some(minor),
    }
}

impl PlatformAdapter for XianyuAdapter {
    async fn start_authorization(
        &self,
        _ctx: &RequestContext,
    ) -> Result<AuthorizationSession, PlatformError> {
        let (flow_id, material) = self.qr_create().await?;
        Ok(AuthorizationSession {
            flow_id,
            qr_content: Some(material.code_content),
            expires_at_ms: material.expires_at_ms,
        })
    }

    async fn stop_account(&self, ctx: &RequestContext) -> Result<(), PlatformError> {
        self.detach(&ctx.account_id).await;
        Ok(())
    }

    async fn list_products(
        &self,
        ctx: &RequestContext,
        cursor: Option<&str>,
    ) -> Result<ProductPage, PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let page: u32 = cursor.and_then(|c| c.parse().ok()).unwrap_or(1);
        let mtop = self.mtop(&session).await;
        let data = json!({
            "needGroupInfo": false,
            "pageNumber": page,
            "pageSize": 20,
            "groupName": "在售",
            "groupId": "58877261",
            "defaultGroup": true,
            "userId": session.external_id,
        })
        .to_string();
        let resp = mtop
            .call(
                "mtop.idle.web.xyh.item.list",
                &data,
                "https://www.goofish.com/",
            )
            .await
            .map_err(|e: MtopError| e.to_platform())?;
        let cards = resp
            .get("cardList")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        let mut items = Vec::new();
        for card in cards {
            let Some(card_data) = card.get("cardData") else {
                continue;
            };
            let id = card_data
                .get("detailParams")
                .and_then(|d| d.get("itemId"))
                .and_then(|v| v.as_str())
                .or_else(|| card_data.get("id").and_then(|v| v.as_str()))
                .map(|s| s.to_string());
            let Some(id) = id else { continue };
            if id.starts_with("auto_") {
                continue; // 平台占位卡
            }
            items.push(ProductRecord {
                external_item_id: id,
                title: card_data
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                on_sale: true,
                // 商品列表不带规格维度:规格完备性由订单详情补足(§5)
                sku_parts: Vec::new(),
                sku_complete: false,
                single_sku_confirmed: false,
            });
        }
        let total_pages = resp.get("pageCount").and_then(|v| v.as_i64()).unwrap_or(1);
        let next_cursor = if (page as i64) < total_pages {
            Some((page + 1).to_string())
        } else {
            None
        };
        let empty_page = resp
            .get("expressesEmpty")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(ProductPage {
            items,
            next_cursor,
            expresses_empty: empty_page,
        })
    }

    async fn fetch_order_snapshot(
        &self,
        ctx: &RequestContext,
        external_order_id: &str,
    ) -> Result<OrderSnapshot, PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let detail = self.order_detail(&session, external_order_id).await?;

        // 递归扫描:状态/数量/规格/金额字段名不固定(上游行为)
        let order_status = find_str_recursive(&detail, &["orderStatus"]);
        let in_refund = find_str_recursive(&detail, &["inRefund"])
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let platform_state = order_status
            .as_deref()
            .map(|s| normalize_order_status(s, in_refund))
            .map(FieldStatus::Verified)
            .unwrap_or(FieldStatus::Missing);

        let item_info = find_object_recursive(&detail, "itemInfo");
        let quantity = item_info
            .as_ref()
            .and_then(|i| {
                first_of(i, &["buyAmount", "amount", "quantity"]).and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
            })
            .filter(|q| *q > 0)
            .map(|q| q as u32)
            .map(FieldStatus::Verified)
            .unwrap_or(FieldStatus::Missing);

        // 规格:specName/specValue 或 sku 别名成对出现才完整
        let sku_parts = sku_parts_of(item_info);
        let (sku_parts_status, sku_single) = match (&sku_parts, item_info.as_ref()) {
            (Some(parts), _) if !parts.is_empty() => (FieldStatus::Verified(parts.clone()), false),
            (_, Some(info)) => {
                // 无任何规格字段且平台表达单件:单规格事实
                let has_spec_field =
                    first_of(info, &["specName", "specValue", "skuId", "spec_text"]).is_some();
                if has_spec_field {
                    (FieldStatus::Missing, false)
                } else {
                    (FieldStatus::Verified(Vec::new()), true)
                }
            }
            _ => (FieldStatus::Missing, false),
        };

        let price_info = find_object_recursive(&detail, "priceInfo");
        let amount = price_info
            .and_then(|p| first_of(p, &["amount", "totalPrice", "confirmFee", "auctionPrice"]))
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_f64().map(|f| format!("{f:.2}")))
            })
            .and_then(|s| yuan_to_minor(&s))
            .and_then(|minor| Money::new(minor, "CNY").ok())
            .map(FieldStatus::Verified)
            .unwrap_or(FieldStatus::Missing);

        let buyer_id = find_str_recursive(&detail, &["buyerId", "buyerUserId"])
            .map(FieldStatus::Verified)
            .unwrap_or(FieldStatus::Missing);

        // 付款时间:仅明确的付款语义字段(payTime/gmtPayTime),创建时间不可替代
        let paid_at = find_time_ms(&detail, &["payTime", "gmtPayTime", "paymentTime"]);

        // 会话身份:订单会话绑定中的买家与订单买家一致即视为已核验
        let conversation_verified = {
            let chats = session.chats.lock().unwrap();
            chats
                .get(external_order_id)
                .zip(buyer_id.verified())
                .is_some_and(|(binding, order_buyer)| {
                    binding.buyer_id.as_deref() == Some(order_buyer.as_str())
                })
        };

        Ok(OrderSnapshot {
            platform_order_id: external_order_id.to_string(),
            seller_id: FieldStatus::Verified(session.external_id.clone()),
            buyer_id,
            item_id: find_str_recursive(&detail, &["itemId", "auctionId"])
                .map(FieldStatus::Verified)
                .unwrap_or(FieldStatus::Missing),
            trade_type: FieldStatus::Verified(TradeType::Ordinary),
            platform_state,
            paid_at_ms: paid_at
                .map(FieldStatus::Verified)
                .unwrap_or(FieldStatus::Missing),
            amount,
            quantity,
            sku_parts: sku_parts_status,
            sku_single,
            conversation_verified,
            source: "mtop.idle.web.trade.order.detail".into(),
            observed_at_ms: utc_now_ms(),
            browser_supplemented: false,
        })
    }

    async fn trace_sold_orders(
        &self,
        ctx: &RequestContext,
        from_ms: i64,
        to_ms: i64,
        max_pages: u32,
    ) -> Result<(Vec<String>, TraceReport), PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let mtop = self.mtop(&session).await;
        let mut candidates = Vec::new();
        let mut observed_from: Option<i64> = None;
        let mut observed_to: Option<i64> = None;
        let mut complete = false;
        let mut stop_reason: Option<String> = None;
        let mut gaps: Vec<String> = Vec::new();
        let pages = max_pages.min(100);
        for page in 1..=pages {
            let data = json!({
                "pageNumber": page,
                "rowsPerPage": 30,
                "orderIds": "",
                "queryCode": "ALL",
                "orderSearchParam": "{}",
            })
            .to_string();
            let resp = mtop
                .call(
                    "mtop.taobao.idle.trade.merchant.sold.get",
                    &data,
                    "https://www.goofish.com/",
                )
                .await
                .map_err(|e: MtopError| e.to_platform())?;
            let module = resp.get("module").cloned().unwrap_or(Value::Null);
            let items = module
                .get("items")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                complete = true;
                break;
            }
            for item in &items {
                let common = item.get("commonData").cloned().unwrap_or(Value::Null);
                let Some(order_id) = common
                    .get("orderId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                else {
                    continue;
                };
                if let Some(created) = common
                    .get("orderCreateTime")
                    .and_then(|v| v.as_str())
                    .and_then(parse_epoch_ms)
                {
                    observed_from = Some(observed_from.map_or(created, |m: i64| m.min(created)));
                    observed_to = Some(observed_to.map_or(created, |m: i64| m.max(created)));
                    if created < from_ms {
                        // 已到时间窗之外:更早订单无需再翻
                        complete = true;
                        break;
                    }
                }
                candidates.push(order_id);
            }
            if complete {
                break;
            }
            if !module
                .get("nextPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                complete = true;
                break;
            }
        }
        if !complete {
            stop_reason = Some("分页截断".into());
            gaps.push("分页上限内的订单不可完全追溯".into());
        }
        Ok((
            candidates,
            TraceReport {
                requested_from_ms: from_ms,
                requested_to_ms: to_ms,
                observed_from_ms: observed_from,
                observed_to_ms: observed_to,
                coverage_complete: complete,
                last_cursor: None,
                stop_reason,
                gaps,
            },
        ))
    }

    async fn send_text(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        buyer_id: &str,
        content: &ContentForSend,
    ) -> Result<SendOutcome, PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let Some(ws) = self
            .inner
            .sessions
            .lock()
            .await
            .get(&ctx.account_id)
            .and_then(|s| s.ws.clone())
        else {
            return Err(PlatformError::NetworkUnavailable);
        };

        // 会话解析:付款事件绑定优先;缺失时以买家号构造(goofish 会话地址)
        let chat_id = {
            let chats = session.chats.lock().unwrap();
            chats
                .get(order_id)
                .map(|b| b.chat_id.clone())
                .unwrap_or_else(|| buyer_id.trim_end_matches("@goofish").to_string())
        };
        let inner = json!({"contentType": 1, "text": {"text": content.text}});
        let inner_b64 = base64::engine::general_purpose::STANDARD.encode(inner.to_string());
        let my_id = format!("{}@goofish", session.external_id);
        let body = json!([
            {
                "uuid": generate_msg_uuid(),
                "cid": format!("{chat_id}@goofish"),
                "conversationType": 1,
                "content": {"contentType": 101, "custom": {"type": 1, "data": inner_b64}},
                "redPointPolicy": 0,
                "extension": {"extJson": "{}"},
                "ctx": {"appVersion": "1.0", "platform": "web"},
                "mtags": {},
                "msgReadStatusSetting": 1,
            },
            {"actualReceivers": [format!("{}@goofish", buyer_id.trim_end_matches("@goofish")), my_id.clone()]}
        ]);
        let mid = real::generate_mid();
        let mut headers = HashMap::new();
        headers.insert("mid".to_string(), json!(mid.clone()));
        let response = ws
            .request("/r/MessageSend/sendByReceiverScope", headers, Some(body))
            .await
            .map_err(ws_rpc_to_platform)?;
        let code = response.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code == 200 {
            // 严格回显:裸 200 不构成接纳(§6)
            let body = response.get("body").cloned().unwrap_or(Value::Null);
            let message_id = body.get("messageId").and_then(|v| v.as_str()).unwrap_or("");
            let sender = body
                .get("extension")
                .and_then(|e| e.get("senderUserId"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let echoed = body
                .get("content")
                .and_then(|c| c.get("custom"))
                .and_then(|c| c.get("data"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let strict_echo = message_id.ends_with(".PNM")
                && sender == session.external_id
                && echoed == inner_b64;
            return Ok(if strict_echo {
                SendOutcome::Accepted(SendProof {
                    request_id: mid.clone(),
                    platform_message_id: Some(message_id.to_string()),
                    buyer_id: buyer_id.to_string(),
                    chat_id: Some(chat_id),
                    content_digest: content.text_digest.clone(),
                })
            } else {
                SendOutcome::Unknown {
                    hint: Some("200 响应缺少严格回显证据".into()),
                }
            });
        }
        // 408 及 5xx/超时类:可能已提交 → Unknown;其余 4xx:明确拒绝
        if (400..=499).contains(&code) && code != 408 {
            return Ok(SendOutcome::Rejected {
                safe_code: format!("HTTP_{code}"),
            });
        }
        Ok(SendOutcome::Unknown {
            hint: Some(format!("code={code}")),
        })
    }

    async fn confirm_shipment(
        &self,
        ctx: &RequestContext,
        order_id: &str,
        _prior_proof: &SendProof,
    ) -> Result<ConfirmOutcome, PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let mtop = self.mtop(&session).await;
        let data = json!({
            "orderId": order_id,
            "tradeText": "已发货,请查收",
            "picList": [],
            "newUnconsign": true,
        })
        .to_string();
        match mtop
            .call(
                "mtop.taobao.idle.logistic.consign.dummy",
                &data,
                "https://www.goofish.com/",
            )
            .await
        {
            Ok(_) => Ok(ConfirmOutcome::Accepted),
            Err(MtopError::Biz(m)) => Ok(ConfirmOutcome::Rejected { safe_code: m }),
            // 可能已提交但未获证实:核验确认步骤,绝不重发正文(FR-016)
            Err(MtopError::Timeout | MtopError::Network(_)) => Ok(ConfirmOutcome::Unknown),
            Err(e) => Err(e.to_platform()),
        }
    }

    async fn verify_shipment_state(
        &self,
        ctx: &RequestContext,
        order_id: &str,
    ) -> Result<bool, PlatformError> {
        let session = self.session(&ctx.account_id).await?;
        let detail = self.order_detail(&session, order_id).await?;
        let status = find_str_recursive(&detail, &["orderStatus"]).unwrap_or_default();
        let in_refund = find_str_recursive(&detail, &["inRefund"])
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        Ok(matches!(
            normalize_order_status(&status, in_refund),
            PlatformOrderState::Shipped | PlatformOrderState::Completed
        ))
    }
}

fn ws_rpc_to_platform(e: real::WsRpcError) -> PlatformError {
    match e {
        real::WsRpcError::Timeout => PlatformError::Timeout,
        _ => PlatformError::NetworkUnavailable,
    }
}

async fn emit_state(
    events: &tokio::sync::mpsc::Sender<crate::application::ports::platform::PlatformEvent>,
    account_id: &str,
    generation: i64,
    state: &str,
    detail: Option<String>,
) {
    let _ = events
        .send(
            crate::application::ports::platform::PlatformEvent::AccountRuntimeChanged {
                meta: EventMeta {
                    account_id: account_id.to_string(),
                    credential_generation: generation,
                    source_event_id: None,
                    platform_event_at_ms: None,
                    received_at_ms: utc_now_ms(),
                },
                state: state.to_string(),
                detail,
            },
        )
        .await;
}

/// 账号传输层:attach → 循环(取 token→连接注册→值守)→ 退避重连。
/// 状态变化经事件上报(supervisor 写库);授权失效即停,不紧密重连(FR-023 边界)。
#[async_trait::async_trait]
impl crate::runtime::supervisor::AccountTransport for XianyuAdapter {
    async fn run_account(
        &self,
        run: crate::runtime::supervisor::AccountRun,
        events: tokio::sync::mpsc::Sender<crate::application::ports::platform::PlatformEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let account = run.account_id.clone();
        macro_rules! emit {
            ($state:expr, $detail:expr) => {
                emit_state(
                    &events,
                    &account,
                    run.credential_generation,
                    $state,
                    $detail,
                )
                .await
            };
        }
        if self
            .attach(
                &run.account_id,
                &run.cookie_jar_json,
                &run.external_id,
                run.credential_generation,
            )
            .await
            .is_err()
        {
            emit!("auth_expired", Some("凭证不可解析".into()));
            return;
        }
        emit!("connecting", None);
        let mut attempt: u32 = 0;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self
                .connect_ws(&run.account_id, events.clone(), shutdown.clone())
                .await
            {
                Ok((_handle, task)) => {
                    attempt = 0;
                    emit!("online", None);
                    let end = task.await;
                    match end {
                        Ok(real::SessionEnd::Shutdown) | Err(_) => break,
                        Ok(real::SessionEnd::Disconnected) => {}
                        Ok(real::SessionEnd::AuthExpired) => {
                            emit!("auth_expired", Some("平台判定登录失效".into()));
                            break;
                        }
                        Ok(real::SessionEnd::ConnectLimit) => {
                            emit!("offline", Some("连接数超限,等待后重连".into()));
                        }
                    }
                }
                Err(crate::application::ports::platform::PlatformError::SessionExpired) => {
                    emit!("auth_expired", Some("登录会话失效".into()));
                    break;
                }
                Err(_) => {}
            }
            if *shutdown.borrow() {
                break;
            }
            attempt += 1;
            // 指数退避 2→60 秒带抖动;成功连接后重置
            let delay = crate::runtime::backoff_delay(attempt);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
        emit!("offline", None);
        self.detach(&run.account_id).await;
    }
}

/// 递归查找键(首个命中)的字符串值。
fn find_str_recursive(v: &Value, keys: &[&str]) -> Option<String> {
    match v {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key) {
                    if let Some(s) = found.as_str() {
                        return Some(s.to_string());
                    }
                    if let Some(n) = found.as_i64() {
                        return Some(n.to_string());
                    }
                }
            }
            for value in map.values() {
                if let Some(found) = find_str_recursive(value, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|i| find_str_recursive(i, keys)),
        _ => None,
    }
}

fn find_object_recursive<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            if let Some(found) = map.get(key).filter(|f| f.is_object()) {
                return Some(found);
            }
            map.values()
                .find_map(|value| find_object_recursive(value, key))
        }
        Value::Array(items) => items.iter().find_map(|i| find_object_recursive(i, key)),
        _ => None,
    }
}

fn first_of<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| obj.get(*k))
}

fn sku_parts_of(item_info: Option<&Value>) -> Option<Vec<crate::domain::sku::SkuPart>> {
    let info = item_info?;
    let names = info
        .get("specName")
        .or_else(|| info.get("skuText"))?
        .as_str()?;
    let values = info
        .get("specValue")
        .or_else(|| info.get("skuValue"))?
        .as_str()?;
    let name_parts: Vec<&str> = names.split('#').collect();
    let value_parts: Vec<&str> = values.split('#').collect();
    if name_parts.len() != value_parts.len() || name_parts.is_empty() {
        return None;
    }
    Some(
        name_parts
            .iter()
            .zip(value_parts.iter())
            .enumerate()
            .map(|(i, (n, v))| crate::domain::sku::SkuPart {
                property_id: format!("spec_{i}"),
                value_id: v.to_string(),
                property_label: n.to_string(),
                value_label: v.to_string(),
            })
            .collect(),
    )
}

/// 时间字段:秒/毫秒数字或字符串 → 毫秒。
fn find_time_ms(v: &Value, keys: &[&str]) -> Option<i64> {
    let raw = find_str_recursive(v, keys)?;
    parse_epoch_ms(&raw)
}

fn parse_epoch_ms(s: &str) -> Option<i64> {
    let n: i64 = s.parse().ok()?;
    Some(if n > 10_000_000_000 { n } else { n * 1000 })
}

/// 对象安全封装(transport 经 AppState 触发同步;AFIT 主 trait 不动)。
impl crate::application::ports::platform::ItemSyncDriver for XianyuAdapter {
    fn list_products_boxed(
        &self,
        ctx: crate::application::ports::platform::RequestContext,
        cursor: Option<String>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        crate::application::ports::platform::ProductPage,
                        crate::application::ports::platform::PlatformError,
                    >,
                > + Send,
        >,
    > {
        // AFIT future 借用参数;克隆自身与入参以获得 'static future
        let this = self.clone();
        Box::pin(async move {
            <Self as PlatformAdapter>::list_products(&this, &ctx, cursor.as_deref()).await
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_shape_and_stability_rules() {
        let d = generate_device_id("12345");
        assert!(d.ends_with("-12345"));
        let body = d.strip_suffix("-12345").unwrap();
        assert_eq!(body.len(), 36);
        let segs: Vec<&str> = body.split('-').collect();
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[2].chars().next().unwrap(), '4', "版本位");
    }

    #[test]
    fn yuan_to_minor_string_math() {
        assert_eq!(yuan_to_minor("12.34"), Some(1234));
        assert_eq!(yuan_to_minor("0.1"), Some(10));
        assert_eq!(yuan_to_minor("5"), Some(500));
        assert_eq!(yuan_to_minor(""), None);
        assert_eq!(yuan_to_minor("abc"), None);
    }

    #[test]
    fn order_status_normalization() {
        assert_eq!(
            normalize_order_status("待发货", false),
            PlatformOrderState::PendingShip
        );
        assert_eq!(
            normalize_order_status("已发货", false),
            PlatformOrderState::Shipped
        );
        assert_eq!(
            normalize_order_status("随便", false),
            PlatformOrderState::Unknown
        );
        assert_eq!(
            normalize_order_status("待发货", true),
            PlatformOrderState::Refunding
        );
    }

    #[test]
    fn recursive_field_finding() {
        let detail = json!({
            "data": {
                "orderInfo": {"utArgs": {"orderStatus": "待发货"}},
                "itemInfo": {"buyAmount": 2, "specName": "版本#颜色", "specValue": "标准#红"},
                "priceInfo": {"amount": {"value": "10.50"}},
                "buyerInfoVO": {"buyerId": "b-1"},
            }
        });
        assert_eq!(
            find_str_recursive(&detail, &["orderStatus"]).as_deref(),
            Some("待发货")
        );
        assert_eq!(
            find_str_recursive(&detail, &["buyerId"]).as_deref(),
            Some("b-1")
        );
        let item = find_object_recursive(&detail, "itemInfo").unwrap();
        let parts = sku_parts_of(Some(item)).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].value_label, "标准");

        // 无 payTime 字段时不得臆造时间
        assert_eq!(find_time_ms(&detail, &["payTime"]), None);
    }
}
