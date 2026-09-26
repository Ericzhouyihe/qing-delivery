//! 进程内假适配器:US1 确定性验收与 dev 场景共用。
//! 仅测试与 dev-fixtures 构建可用;不访问网络、不持有真实凭证。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::application::ports::platform::{
    AuthorizationSession, ChatPeer, ChatSendKind, ConfirmOutcome, ContentForSend, PlatformAdapter,
    PlatformError, ProductPage, RequestContext, SendOutcome, SendProof, TraceReport,
};
use crate::domain::orders::snapshot::OrderSnapshot;

#[derive(Default)]
struct FakeState {
    snapshots: HashMap<String, OrderSnapshot>,
    product_pages: VecDeque<Result<ProductPage, PlatformError>>,
    trace_script: VecDeque<(Vec<String>, TraceReport)>,
    send_script: VecDeque<ScriptedSend>,
    chat_send_script: VecDeque<SendOutcome>,
    confirm_script: VecDeque<ConfirmOutcome>,
    sent: Vec<SentRecord>,
    chat_sent: Vec<ChatSentRecord>,
    confirm_calls: Vec<String>,
    /// 007 T045:测试辅助注入的买家消息(经 chat_message_events 取出为事件)
    staged_chats: VecDeque<StagedBuyerChat>,
}

/// 注入的买家消息(mock 档 ChatMessageReceived 产出口径)。
#[derive(Clone, Debug)]
pub struct StagedBuyerChat {
    pub platform_message_id: String,
    pub chat_id: Option<String>,
    pub buyer_id: String,
    pub kind: crate::domain::chat::ChatMsgKind,
    pub text: Option<String>,
    pub image_url: Option<String>,
    pub item_id: Option<String>,
}

pub struct SentRecord {
    pub order_id: String,
    pub buyer_id: String,
    pub text: String,
    pub text_digest: String,
    pub request_id: String,
}

/// 聊天发送留痕(007 T035/T036/T037 断言用):不绑定订单。
pub struct ChatSentRecord {
    pub buyer_id: String,
    pub chat_id: Option<String>,
    /// 发送种类(Text | Image{url})
    pub kind: ChatSendKind,
    pub text: String,
    pub request_id: String,
}

enum ScriptedSend {
    Outcome(SendOutcome),
}

#[derive(Clone)]
pub struct FakeAdapter {
    state: Arc<Mutex<FakeState>>,
}

impl FakeAdapter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
        }
    }

    /// 预置订单快照(以平台订单号为键)。
    pub fn stage_snapshot(&self, snapshot: OrderSnapshot) {
        self.state
            .lock()
            .unwrap()
            .snapshots
            .insert(snapshot.platform_order_id.clone(), snapshot);
    }

    /// 脚本化下一次 send_text 结果;未脚本化时默认返回严格关联的 Accepted。
    pub fn script_send(&self, outcome: SendOutcome) {
        self.state
            .lock()
            .unwrap()
            .send_script
            .push_back(ScriptedSend::Outcome(outcome));
    }

    /// 脚本化下一次 send_chat_message 结果(独立队列,不影响 send_text 脚本);
    /// 未脚本化时默认返回严格关联的 Accepted。
    pub fn script_chat_send(&self, outcome: SendOutcome) {
        self.state
            .lock()
            .unwrap()
            .chat_send_script
            .push_back(outcome);
    }

    pub fn script_confirm(&self, outcome: ConfirmOutcome) {
        self.state.lock().unwrap().confirm_script.push_back(outcome);
    }

    /// 脚本化一页商品(按调用顺序消费);同步作业逐页拉取。
    pub fn stage_product_page(&self, page: Result<ProductPage, PlatformError>) {
        self.state.lock().unwrap().product_pages.push_back(page);
    }

    /// 脚本化一次追溯结果(候选订单号 + 覆盖报告)。
    pub fn stage_trace(&self, candidates: Vec<String>, coverage_complete: bool) {
        let report = TraceReport {
            requested_from_ms: 0,
            requested_to_ms: 0,
            observed_from_ms: None,
            observed_to_ms: None,
            coverage_complete,
            last_cursor: None,
            stop_reason: if coverage_complete {
                None
            } else {
                Some("分页截断".into())
            },
            gaps: if coverage_complete {
                Vec::new()
            } else {
                vec!["窗口外订单不可追溯".into()]
            },
        };
        self.state
            .lock()
            .unwrap()
            .trace_script
            .push_back((candidates, report));
    }

    /// 已发送记录(断言用):每单发送次数、买家、原文。
    pub fn sent_records(&self) -> Vec<SentRecord> {
        self.state
            .lock()
            .unwrap()
            .sent
            .iter()
            .map(|r| SentRecord {
                order_id: r.order_id.clone(),
                buyer_id: r.buyer_id.clone(),
                text: r.text.clone(),
                text_digest: r.text_digest.clone(),
                request_id: r.request_id.clone(),
            })
            .collect()
    }

    pub fn confirm_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().confirm_calls.clone()
    }

    /// 聊天发送留痕(断言用):对端、种类、原文与请求 ID。
    pub fn chat_sent_records(&self) -> Vec<ChatSentRecord> {
        self.state
            .lock()
            .unwrap()
            .chat_sent
            .iter()
            .map(|r| ChatSentRecord {
                buyer_id: r.buyer_id.clone(),
                chat_id: r.chat_id.clone(),
                kind: r.kind.clone(),
                text: r.text.clone(),
                request_id: r.request_id.clone(),
            })
            .collect()
    }

    /// 007 T045 测试辅助:注入一条买家消息(mock 档"模拟买家来消息")。
    pub fn stage_buyer_chat(&self, chat: StagedBuyerChat) {
        self.state.lock().unwrap().staged_chats.push_back(chat);
    }

    /// 007 T045 测试辅助:取出全部已注入买家消息,包装为 ChatMessageReceived
    /// 事件(mock 适配器的聊天事件产出口径;消费方=事件分发/ChatService::ingest)。
    pub fn chat_message_events(
        &self,
        account_id: &str,
        generation: i64,
    ) -> Vec<crate::application::ports::platform::PlatformEvent> {
        let mut state = self.state.lock().unwrap();
        state
            .staged_chats
            .drain(..)
            .map(|c| {
                crate::application::ports::platform::PlatformEvent::ChatMessageReceived {
                    meta: crate::application::ports::platform::EventMeta {
                        account_id: account_id.to_string(),
                        credential_generation: generation,
                        source_event_id: Some(format!("chat:{}", c.platform_message_id)),
                        platform_event_at_ms: None,
                        received_at_ms: 0,
                    },
                    chat_id: c.chat_id,
                    buyer_id: Some(c.buyer_id),
                    message_id: c.platform_message_id,
                    msg_kind: c.kind,
                    text: c.text,
                    image_url: c.image_url,
                    item_id: c.item_id,
                }
            })
            .collect()
    }
}

impl Default for FakeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformAdapter for FakeAdapter {
    async fn start_authorization(
        &self,
        _ctx: &RequestContext,
    ) -> Result<AuthorizationSession, PlatformError> {
        Err(PlatformError::Unsupported(
            "假适配器不提供扫码(US2 实现)".into(),
        ))
    }

    async fn stop_account(&self, _ctx: &RequestContext) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn list_products(
        &self,
        _ctx: &RequestContext,
        _cursor: Option<&str>,
    ) -> Result<ProductPage, PlatformError> {
        let mut state = self.state.lock().unwrap();
        match state.product_pages.pop_front() {
            Some(page) => page,
            // 未脚本化:成功省略列表 = 正常空页(§5)
            None => Ok(ProductPage {
                items: Vec::new(),
                next_cursor: None,
                expresses_empty: true,
            }),
        }
    }

    async fn fetch_order_snapshot(
        &self,
        _ctx: &RequestContext,
        external_order_id: &str,
    ) -> Result<OrderSnapshot, PlatformError> {
        let state = self.state.lock().unwrap();
        state
            .snapshots
            .get(external_order_id)
            .cloned()
            .ok_or_else(|| {
                PlatformError::IncompleteFacts(format!("订单 {external_order_id} 未预置"))
            })
    }

    async fn trace_sold_orders(
        &self,
        _ctx: &RequestContext,
        _from_ms: i64,
        _to_ms: i64,
        _max_pages: u32,
    ) -> Result<(Vec<String>, TraceReport), PlatformError> {
        let mut state = self.state.lock().unwrap();
        match state.trace_script.pop_front() {
            Some((ids, mut report)) => {
                report.requested_from_ms = _from_ms;
                report.requested_to_ms = _to_ms;
                Ok((ids, report))
            }
            None => Ok((
                Vec::new(),
                TraceReport {
                    requested_from_ms: _from_ms,
                    requested_to_ms: _to_ms,
                    observed_from_ms: None,
                    observed_to_ms: None,
                    coverage_complete: true,
                    last_cursor: None,
                    stop_reason: None,
                    gaps: Vec::new(),
                },
            )),
        }
    }

    async fn send_text(
        &self,
        _ctx: &RequestContext,
        order_id: &str,
        buyer_id: &str,
        content: &ContentForSend,
    ) -> Result<SendOutcome, PlatformError> {
        let mut state = self.state.lock().unwrap();
        let request_id = crate::domain::ids::new_request_key();
        state.sent.push(SentRecord {
            order_id: order_id.to_string(),
            buyer_id: buyer_id.to_string(),
            text: content.text.clone(),
            text_digest: content.text_digest.clone(),
            request_id: request_id.clone(),
        });
        let scripted = state.send_script.pop_front();
        match scripted {
            Some(ScriptedSend::Outcome(outcome)) => Ok(match outcome {
                // Accepted 证明统一换成当前 request_id,保证严格关联
                SendOutcome::Accepted(mut proof) => {
                    proof.request_id = request_id;
                    proof.content_digest = content.text_digest.clone();
                    proof.buyer_id = buyer_id.to_string();
                    SendOutcome::Accepted(proof)
                }
                other => other,
            }),
            None => Ok(SendOutcome::Accepted(SendProof {
                request_id,
                platform_message_id: Some(format!("msg-{order_id}")),
                buyer_id: buyer_id.to_string(),
                chat_id: None,
                content_digest: content.text_digest.clone(),
            })),
        }
    }

    /// 聊天发送(007 T035):与 send_text 同语义但不绑定订单;
    /// 脚本化优先,未脚本化默认严格关联 Accepted。
    async fn send_chat_message(
        &self,
        _ctx: &RequestContext,
        peer: &ChatPeer,
        kind: ChatSendKind,
        content: &ContentForSend,
    ) -> Result<SendOutcome, PlatformError> {
        let mut state = self.state.lock().unwrap();
        let request_id = crate::domain::ids::new_request_key();
        state.chat_sent.push(ChatSentRecord {
            buyer_id: peer.buyer_id.clone(),
            chat_id: peer.chat_id.clone(),
            kind: kind.clone(),
            text: content.text.clone(),
            request_id: request_id.clone(),
        });
        match state.chat_send_script.pop_front() {
            // Accepted 证明统一换成当前 request_id,保证严格关联
            Some(SendOutcome::Accepted(mut proof)) => {
                proof.request_id = request_id;
                proof.content_digest = content.text_digest.clone();
                proof.buyer_id = peer.buyer_id.clone();
                proof.chat_id = peer.chat_id.clone();
                Ok(SendOutcome::Accepted(proof))
            }
            Some(other) => Ok(other),
            None => Ok(SendOutcome::Accepted(SendProof {
                request_id,
                platform_message_id: Some(format!(
                    "chat-msg-{}",
                    peer.chat_id.as_deref().unwrap_or(&peer.buyer_id)
                )),
                buyer_id: peer.buyer_id.clone(),
                chat_id: peer.chat_id.clone(),
                content_digest: content.text_digest.clone(),
            })),
        }
    }

    async fn confirm_shipment(
        &self,
        _ctx: &RequestContext,
        order_id: &str,
        _prior_proof: &SendProof,
    ) -> Result<ConfirmOutcome, PlatformError> {
        let mut state = self.state.lock().unwrap();
        state.confirm_calls.push(order_id.to_string());
        Ok(state
            .confirm_script
            .pop_front()
            .unwrap_or(ConfirmOutcome::Accepted))
    }

    async fn verify_shipment_state(
        &self,
        _ctx: &RequestContext,
        _order_id: &str,
    ) -> Result<bool, PlatformError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RequestContext {
        RequestContext {
            operation_id: "op-test".into(),
            account_id: "acct-1".into(),
            credential_generation: 1,
            control_generation: 1,
            deadline_ms: i64::MAX,
        }
    }

    fn peer() -> ChatPeer {
        ChatPeer {
            buyer_id: "buyer-1".into(),
            chat_id: Some("chat-9".into()),
        }
    }

    fn content() -> ContentForSend {
        ContentForSend {
            snapshot_id: "snap-1".into(),
            text: "你好".into(),
            text_digest: "digest-1".into(),
        }
    }

    /// T035:未脚本化默认严格关联 Accepted,留痕携带对端/种类/原文。
    #[tokio::test]
    async fn chat_send_defaults_to_strict_accepted_and_records() {
        let fake = FakeAdapter::new();
        let out = fake
            .send_chat_message(&ctx(), &peer(), ChatSendKind::Text, &content())
            .await
            .unwrap();
        match out {
            SendOutcome::Accepted(proof) => {
                assert_eq!(proof.buyer_id, "buyer-1");
                assert_eq!(proof.chat_id.as_deref(), Some("chat-9"));
                assert_eq!(proof.content_digest, "digest-1");
                assert!(!proof.request_id.is_empty());
            }
            other => panic!("默认应返回 Accepted,实际 {other:?}"),
        }
        let sent = fake.chat_sent_records();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].buyer_id, "buyer-1");
        assert_eq!(sent[0].chat_id.as_deref(), Some("chat-9"));
        assert_eq!(sent[0].text, "你好");
        assert!(matches!(sent[0].kind, ChatSendKind::Text));
    }

    /// T035:脚本化结果透传四分类(Accepted 归一为当前请求,其余原样)。
    #[tokio::test]
    async fn chat_send_passes_through_all_four_outcomes() {
        let fake = FakeAdapter::new();
        // NotSubmitted
        fake.script_chat_send(SendOutcome::NotSubmitted { retryable: true });
        let out = fake
            .send_chat_message(&ctx(), &peer(), ChatSendKind::Text, &content())
            .await
            .unwrap();
        assert_eq!(out, SendOutcome::NotSubmitted { retryable: true });
        // Rejected
        fake.script_chat_send(SendOutcome::Rejected {
            safe_code: "HTTP_403".into(),
        });
        let out = fake
            .send_chat_message(&ctx(), &peer(), ChatSendKind::Text, &content())
            .await
            .unwrap();
        assert_eq!(
            out,
            SendOutcome::Rejected {
                safe_code: "HTTP_403".into()
            }
        );
        // Unknown
        fake.script_chat_send(SendOutcome::Unknown {
            hint: Some("超时".into()),
        });
        let out = fake
            .send_chat_message(&ctx(), &peer(), ChatSendKind::Text, &content())
            .await
            .unwrap();
        assert_eq!(
            out,
            SendOutcome::Unknown {
                hint: Some("超时".into())
            }
        );
        // Accepted(证明归一到当前请求与摘要)
        fake.script_chat_send(SendOutcome::Accepted(SendProof {
            request_id: "stale".into(),
            platform_message_id: Some("m-1".into()),
            buyer_id: "other".into(),
            chat_id: None,
            content_digest: "stale".into(),
        }));
        let out = fake
            .send_chat_message(
                &ctx(),
                &peer(),
                ChatSendKind::Image {
                    url: "https://img".into(),
                },
                &content(),
            )
            .await
            .unwrap();
        match out {
            SendOutcome::Accepted(proof) => {
                assert_ne!(proof.request_id, "stale");
                assert_eq!(proof.content_digest, "digest-1");
                assert_eq!(proof.buyer_id, "buyer-1");
            }
            other => panic!("应返回 Accepted,实际 {other:?}"),
        }
        assert_eq!(fake.chat_sent_records().len(), 4, "每次发送均留痕");
        // 聊天脚本与 send_text 脚本互不干扰:未脚本化的 send_text 仍默认 Accepted
        let out = fake
            .send_text(&ctx(), "ORD-1", "buyer-1", &content())
            .await
            .unwrap();
        assert!(matches!(out, SendOutcome::Accepted(_)));
        assert_eq!(fake.sent_records().len(), 1);
    }
}
