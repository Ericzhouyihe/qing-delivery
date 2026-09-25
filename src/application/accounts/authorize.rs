//! 扫码授权驱动(T041/T093 应用层):连接 QrFlowService 状态机与真实协议。
//! 平台轮询 → 状态迁移;CONFIRMED 才收 Cookie,unb 身份核验后建账号、
//! 凭证加密落库(代次单调)并挂载适配器会话。晚到结果由代次绑定拒绝。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::credentials;
use crate::adapters::windows::keys::DataKey;
use crate::adapters::xianyu::adapter::{QrPollResult, XianyuAdapter};
use crate::application::accounts::qr::{QR_TTL_MS, QrFlowService};
use crate::domain::accounts::AuthFlowStatus;
use crate::domain::time_util::utc_now_ms;

pub struct QrPresentation {
    pub image_data_url: String,
    pub expires_at_ms: i64,
}

/// 扫码驱动抽象:live 提供;mock 构建为 None(端点明确报不支持)。
#[async_trait::async_trait]
pub trait QrDriver: Send + Sync {
    async fn create(&self, qr_id: &str) -> Result<QrPresentation, String>;
    async fn image(&self, qr_id: &str) -> Option<String>;
}

#[derive(Clone)]
struct FlowState {
    adapter_flow_id: String,
    image_data_url: String,
}

pub struct AuthorizationService {
    db: DbThread,
    key: DataKey,
    adapter: XianyuAdapter,
    /// 004:资料服务(授权完成后异步首拉;FR-001)
    profile: Option<std::sync::Arc<crate::application::accounts::profile::ProfileService>>,
    flows: tokio::sync::Mutex<HashMap<String, FlowState>>,
}

impl AuthorizationService {
    pub fn new(
        db: DbThread,
        key: DataKey,
        adapter: XianyuAdapter,
        profile: Option<std::sync::Arc<crate::application::accounts::profile::ProfileService>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            key,
            adapter,
            profile,
            flows: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// 启动后台轮询(每 3 秒);服务停止由运行时整体终止。
    pub fn spawn(self: &Arc<Self>) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                svc.poll_once().await;
            }
        });
    }

    async fn poll_once(&self) {
        let ids: Vec<String> = self.flows.lock().await.keys().cloned().collect();
        for qr_id in ids {
            self.drive(qr_id).await;
        }
    }

    async fn drive(&self, qr_id: String) {
        let Some(flow) = self.flows.lock().await.get(&qr_id).cloned() else {
            return;
        };
        let qr_service = QrFlowService::new(self.db.clone());
        let Ok(session) = qr_service.observe(&qr_id).await else {
            self.flows.lock().await.remove(&qr_id);
            return;
        };
        // 本地终态/新代次:不再驱动
        if session.status != AuthFlowStatus::Pending && session.status != AuthFlowStatus::Scanned {
            self.flows.lock().await.remove(&qr_id);
            return;
        }
        if utc_now_ms() > session.expires_at && session.expires_at != 0 {
            // 本地 observe 已按 TTL 标记过期;清理驱动侧映射
            self.flows.lock().await.remove(&qr_id);
            return;
        }
        let result = match self.adapter.qr_poll(&flow.adapter_flow_id).await {
            Ok(r) => r,
            Err(_) => return, // 网络抖动:下一轮再试;连续失败由 TTL 收敛
        };
        let generation = session.generation;
        macro_rules! transition {
            ($to:expr, $bound:expr) => {
                let _ = qr_service
                    .apply_platform_transition(&qr_id, generation, $to, $bound)
                    .await;
            };
        }
        match result {
            QrPollResult::New => {}
            QrPollResult::Scanned => {
                transition!(AuthFlowStatus::Scanned, None);
            }
            QrPollResult::Confirmed {
                unb,
                cookie_jar_json,
            } => {
                transition!(AuthFlowStatus::Confirmed, Some(&unb));
                // 身份核验+建账号;complete 幂等(同身份同账号)
                let display = format!("闲鱼账号 {unb}");
                let Ok(account_id) = qr_service.complete(&qr_id, &unb, &display).await else {
                    return;
                };
                let cred_generation = utc_now_ms(); // 单调:时间基恒大于历史值
                let key = self.key.clone();
                let account = account_id.clone();
                let jar = cookie_jar_json.clone();
                let saved = self
                    .db
                    .call(move |conn| {
                        credentials::save(
                            conn,
                            &account,
                            &key.key,
                            &key.key_id,
                            &jar,
                            cred_generation,
                        )
                    })
                    .await;
                if let Ok(Ok(true)) = saved {
                    let _ = self
                        .adapter
                        .attach(&account_id, &cookie_jar_json, &unb, cred_generation)
                        .await;
                    // 004 FR-001:首拉资料(异步,失败仅日志不阻塞接入)
                    if let Some(profile) = self.profile.clone() {
                        let acc = account_id.clone();
                        tokio::spawn(async move {
                            match profile.refresh(&acc).await {
                                Ok(crate::application::accounts::profile::ProfileRefresh::Updated { nickname, .. }) => {
                                    tracing::info!(account = %acc, nickname, "账号资料首拉完成");
                                }
                                Ok(other) => tracing::info!(account = %acc, ?other, "资料首拉未更新(可手动刷新)"),
                                Err(e) => tracing::warn!(account = %acc, error = %e, "资料首拉失败"),
                            }
                        });
                    }
                }
                self.flows.lock().await.remove(&qr_id);
            }
            QrPollResult::Expired => {
                transition!(AuthFlowStatus::Expired, None);
                self.flows.lock().await.remove(&qr_id);
            }
            QrPollResult::Canceled => {
                transition!(AuthFlowStatus::Canceled, None);
                self.flows.lock().await.remove(&qr_id);
            }
            QrPollResult::Verification { .. } => {
                // 检测到验证要求:暂停该账号新外部动作(T050 完成前给出口)
                transition!(AuthFlowStatus::NeedsVerification, None);
                self.flows.lock().await.remove(&qr_id);
            }
            QrPollResult::Failed(_) => {
                transition!(AuthFlowStatus::Failed, None);
                self.flows.lock().await.remove(&qr_id);
            }
        }
    }

    async fn create_flow(&self, qr_id: &str) -> Result<QrPresentation, String> {
        let (adapter_flow_id, material) =
            self.adapter.qr_create().await.map_err(|e| e.to_string())?;
        self.flows.lock().await.insert(
            qr_id.to_string(),
            FlowState {
                adapter_flow_id,
                image_data_url: material.image_data_url.clone(),
            },
        );
        Ok(QrPresentation {
            image_data_url: material.image_data_url,
            expires_at_ms: material.expires_at_ms.min(utc_now_ms() + QR_TTL_MS),
        })
    }

    async fn image_of(&self, qr_id: &str) -> Option<String> {
        self.flows
            .lock()
            .await
            .get(qr_id)
            .map(|f| f.image_data_url.clone())
    }
}

#[async_trait::async_trait]
impl QrDriver for AuthorizationService {
    async fn create(&self, qr_id: &str) -> Result<QrPresentation, String> {
        self.create_flow(qr_id).await
    }

    async fn image(&self, qr_id: &str) -> Option<String> {
        self.image_of(qr_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;

    async fn setup() -> (tempfile::TempDir, DbThread, DataKey) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("authz.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        let key = DataKey {
            key_id: "k1".into(),
            key: [7u8; 32],
        };
        (dir, db, key)
    }

    #[tokio::test]
    async fn qr_flow_service_state_machine_defaults() {
        // 授权服务依赖 QrFlowService 状态机:验证默认状态与 TTL 语义不变
        let (_d, db, _k) = setup().await;
        let svc = QrFlowService::new(db);
        let s = svc.create(None).await.unwrap();
        assert_eq!(s.status, AuthFlowStatus::Pending);
        assert!(s.expires_at > utc_now_ms());
    }
}
