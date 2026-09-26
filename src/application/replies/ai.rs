//! 007 US7 AI 回复分支实装(T077,research D7):HttpAiProvider 实现
//! AiReplyProvider(T036 占位转正)。每次调用读账号级配置:
//! - 账号 ai_reply_enabled=false → Ok(None)(不调 AI,直落默认回复);
//! - 系统 AI 配置缺失(未配地址/Key)→ Ok(None) 等价 NoAi,tracing 留痕
//!   一次(不逐条消息刷日志);
//! - 组装 system 消息 = 账号 ai_prompt(空则内置默认)+ 长度约束,
//!   user 消息 = 买家文本;输出截断 2000 字由 ReplyService 统一执行。
//! 网络调用(AiHttpClient)在 DbThread 事务之外;上游错误以 Err 返回,
//! 由 ReplyService 静默降级(不阻塞聊天摄取,零订单/交付写入)。

use std::sync::atomic::{AtomicBool, Ordering};

use crate::adapters::aiclient::{AiHttpClient, ChatMessage};
use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::accounts;
use crate::application::replies::AiReplyProvider;
use crate::application::settings_sys::SettingsService;

/// 账号未自定义提示词时的内置 system 提示。
const DEFAULT_SYSTEM_PROMPT: &str =
    "你是店铺的客服助手,用简体中文礼貌、简洁地回复买家关于商品的咨询。";
/// 长度与边界约束(拼在 system 消息尾部;FR-071:仅客服文案)。
const SYSTEM_CONSTRAINT: &str =
    "回复是发给买家的纯文本客服消息,不超过 2000 字;不要编造订单、发货或退款信息,不要发送任何卡密或发货内容。";

pub struct HttpAiProvider {
    db: DbThread,
    settings: SettingsService,
    client: AiHttpClient,
    /// "系统未配置 AI"只留痕一次(避免每条消息刷日志)。
    unconfigured_logged: AtomicBool,
}

impl HttpAiProvider {
    pub fn new(db: DbThread, settings: SettingsService) -> Self {
        Self {
            db,
            settings,
            client: AiHttpClient::new(),
            unconfigured_logged: AtomicBool::new(false),
        }
    }

    fn log_unconfigured_once(&self) {
        if !self.unconfigured_logged.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "AI 自动回复未启用:系统未配置 AI 地址/Key,未命中关键词的消息直落默认回复"
            );
        }
    }
}

#[async_trait::async_trait]
impl AiReplyProvider for HttpAiProvider {
    async fn reply(
        &self,
        account_id: &str,
        _item_id: Option<&str>,
        _buyer_id: &str,
        text: &str,
    ) -> Result<Option<String>, String> {
        // ① 账号级门禁:ai_reply_enabled=false 直接 None(不发起任何 AI 请求)
        let account_cfg = self
            .db
            .call({
                let account_id = account_id.to_string();
                move |conn| accounts::get_ai_settings(conn, &account_id)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let Some((enabled, prompt)) = account_cfg else {
            return Err(format!("账号不存在:{account_id}"));
        };
        if !enabled {
            return Ok(None);
        }
        // ② 系统 AI 配置(缺地址/Key → 等价 NoAi,静默留痕一次)
        let Some(cfg) = self
            .settings
            .load_ai_config()
            .await
            .map_err(|e| e.to_string())?
        else {
            self.log_unconfigured_once();
            return Ok(None);
        };
        // ③ 组装消息:system = 账号提示词 + 约束;user = 买家文本
        let base_prompt = prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let messages = [
            ChatMessage::new("system", format!("{base_prompt}\n{SYSTEM_CONSTRAINT}")),
            ChatMessage::new("user", text.to_string()),
        ];
        // ④ 网络调用(事务外;错误上抛 → ReplyService 静默降级留痕)
        match self.client.chat(&cfg, &messages).await {
            Ok(content) => Ok(Some(content)),
            Err(e) => Err(e.to_string()),
        }
    }
}
