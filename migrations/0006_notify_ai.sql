-- 007 增量 3(US5 通知渠道 + US7 系统与AI,data-model.md 增量 3)。
-- 时间戳沿 007 约定:TEXT(RFC3339 UTC,字典序可排序)。

-- ===== 通知渠道 =====

CREATE TABLE notification_channels (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('webhook','email','dingtalk','feishu','wecom','bark','telegram')),
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    -- 非机密字段明文 JSON:URL、chat_id、from_name、端口、加密方式等
    config TEXT NOT NULL,
    -- 机密字典整包信封(token/secret/password/smtp_user 等);
    -- 明文只在内存与解密后的 ChannelConfig,API 永不回显(只 *_configured)
    secrets_ciphertext BLOB,
    secrets_nonce BLOB,
    secrets_key_id TEXT,
    secrets_format_version INTEGER,
    -- JSON 数组(空=订阅全部事件)
    event_types TEXT NOT NULL DEFAULT '[]',
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE notification_bindings (
    account_id TEXT NOT NULL,
    channel_id TEXT NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, channel_id)
);

CREATE TABLE notification_deliveries (
    id TEXT PRIMARY KEY,
    -- 渠道删除后保留审计(无 FK,残留 channel_id 无害)
    channel_id TEXT,
    event_kind TEXT NOT NULL,
    subject TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('accepted','not_sent','unknown')),
    error_hint TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_notification_deliveries_created
    ON notification_deliveries(created_at DESC);

-- ===== 系统设置与系统秘密 =====

CREATE TABLE system_settings (
    -- 明文键值:ai_api_url、ai_model、smtp_host/smtp_port/smtp_encryption/
    -- smtp_from_address/smtp_from_name/smtp_user 等
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE system_secrets (
    -- 信封四件套 NOT NULL:ai_api_key、smtp_password 等;
    -- 值仅内部消费(发送/AI 客户端),读取端点只回 *_configured
    key TEXT PRIMARY KEY,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    key_id TEXT NOT NULL,
    format_version INTEGER NOT NULL
);

-- ===== accounts 扩展(US7 AI 回复;本增量只加列) =====

ALTER TABLE accounts ADD COLUMN ai_reply_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE accounts ADD COLUMN ai_prompt TEXT;

-- ===== 无订单 issue 去重(research D8) =====
-- 既有 idx_issues_open_dedup 要求 order_id IS NOT NULL;
-- notify_unknown 等无订单事项按 (account_id, kind, reason_code) 去重。

CREATE UNIQUE INDEX idx_issues_open_dedup_account
    ON issues(account_id, kind, reason_code)
    WHERE state='open' AND order_id IS NULL AND account_id IS NOT NULL;
