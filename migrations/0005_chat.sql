-- 007 增量 2(US4 在线聊天,data-model.md 增量 2)。
-- 约定沿 007 data-model:时间戳 TEXT(RFC3339 UTC,字典序可排序);
-- chat body 明文存储是显式接受的权衡(research D15:内容搜索需要 LIKE 可行,
-- 访问边界=本机 OS 用户;交付正文仍走加密 content_snapshots,此处是展示副本)。

-- ===== 会话 =====

CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    peer_buyer_id TEXT NOT NULL,
    peer_nickname TEXT,
    chat_id TEXT,
    last_item_id TEXT,
    last_message_at TEXT,
    last_message_preview TEXT,
    unread_count INTEGER NOT NULL DEFAULT 0,
    hidden_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (account_id, peer_buyer_id)
);
CREATE INDEX idx_conversations_account_visible
    ON conversations(account_id, hidden_at, last_message_at);

-- ===== 消息 =====

CREATE TABLE chat_messages (
    -- 时间有序 UUID(字典序≈时间序,游标分页 before_id 依赖)
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('in','out')),
    msg_kind TEXT NOT NULL CHECK (msg_kind IN ('text','image','system','item_card','unknown')),
    -- 明文(research D15)
    body_text TEXT,
    -- 数据目录 uploads/chat/ 相对路径(出站图片)
    image_path TEXT,
    -- 仅 out 行:发送状态机 sending → sent|failed|uncertain(人工重试=新行)
    status TEXT CHECK (status IN ('sending','sent','failed','uncertain','cancelled')),
    -- 平台消息 ID(入站去重键;出站为接纳回执里的平台消息 ID)
    platform_message_id TEXT,
    -- T050 OutgoingMessageEvidence 回填:出站行存发送幂等键/适配器请求 ID
    request_key TEXT,
    error_hint TEXT,
    item_snapshot TEXT,
    created_at TEXT NOT NULL,
    read_at TEXT
);
CREATE INDEX idx_chat_messages_conversation ON chat_messages(conversation_id, created_at);
CREATE INDEX idx_chat_messages_open_status ON chat_messages(status)
    WHERE status IN ('sending','uncertain');

-- ===== 快捷回复(全局 ≤50 条,应用层上限)与买家备注 =====

CREATE TABLE quick_replies (
    id TEXT PRIMARY KEY,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE buyer_notes (
    account_id TEXT NOT NULL,
    buyer_id TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (account_id, buyer_id)
);
