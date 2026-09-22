//! 进程内假适配器:US1 确定性验收与 dev 场景共用。
//! 仅测试与 dev-fixtures 构建可用;不访问网络、不持有真实凭证。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::application::ports::platform::{
    AuthorizationSession, ConfirmOutcome, ContentForSend, PlatformAdapter, PlatformError,
    ProductPage, RequestContext, SendOutcome, SendProof, TraceReport,
};
use crate::domain::orders::snapshot::OrderSnapshot;

#[derive(Default)]
struct FakeState {
    snapshots: HashMap<String, OrderSnapshot>,
    product_pages: VecDeque<Result<ProductPage, PlatformError>>,
    trace_script: VecDeque<(Vec<String>, TraceReport)>,
    send_script: VecDeque<ScriptedSend>,
    confirm_script: VecDeque<ConfirmOutcome>,
    sent: Vec<SentRecord>,
    confirm_calls: Vec<String>,
}

pub struct SentRecord {
    pub order_id: String,
    pub buyer_id: String,
    pub text: String,
    pub text_digest: String,
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
