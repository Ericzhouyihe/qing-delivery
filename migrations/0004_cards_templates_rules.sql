-- 007 增量 1(US1 卡密库存 + US2 发货模板 + US3 规则扩展,data-model.md 增量 1)。
-- 约定沿 007 data-model:时间戳 TEXT(RFC3339 UTC,字典序可排序);
-- 加密列四件套 *_{ciphertext,nonce,key_id,format_version}(AES-GCM 信封,AAD 用途区分);
-- 卡密明文永不落明文列。

-- ===== US1 卡密库存 =====

CREATE TABLE card_pools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('data','text','image','api')),
    enabled INTEGER NOT NULL DEFAULT 1,
    delay_seconds INTEGER NOT NULL DEFAULT 0 CHECK (delay_seconds >= 0 AND delay_seconds <= 3600),
    description TEXT NOT NULL DEFAULT '',
    -- kind=text/image 时的固定内容信封(AAD purpose card_pool_content)
    content_ciphertext BLOB,
    content_nonce BLOB,
    key_id TEXT,
    format_version INTEGER,
    -- kind=api 时整份配置 JSON 信封(AAD purpose card_api_config)
    api_config_ciphertext BLOB,
    api_config_nonce BLOB,
    api_key_id TEXT,
    api_format_version INTEGER,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_card_pools_kind_enabled ON card_pools(kind, enabled);

CREATE TABLE card_entries (
    id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL REFERENCES card_pools(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'available'
        CHECK (state IN ('available','reserved','used','disabled','pending_api')),
    origin TEXT NOT NULL DEFAULT 'stock' CHECK (origin IN ('stock','api')),
    -- 单卡内容信封(AAD purpose card_entry)
    content_ciphertext BLOB NOT NULL,
    content_nonce BLOB NOT NULL,
    key_id TEXT NOT NULL,
    format_version INTEGER NOT NULL,
    content_digest TEXT NOT NULL,
    reserved_order_id TEXT,
    reserved_delivery_id TEXT,
    request_key TEXT,
    reserved_at TEXT,
    used_at TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_card_entries_pool_state ON card_entries(pool_id, state);
CREATE INDEX idx_card_entries_order ON card_entries(reserved_order_id);

-- ===== US2 发货模板 =====

CREATE TABLE delivery_templates (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE delivery_template_messages (
    id INTEGER PRIMARY KEY,
    template_id TEXT NOT NULL REFERENCES delivery_templates(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (template_id, position)
);

-- ===== US3 规则扩展列与变体 =====

ALTER TABLE rules ADD COLUMN trigger_type TEXT NOT NULL DEFAULT 'order_paid'
    CHECK (trigger_type IN ('order_paid','review_missing_timeout','buyer_reviewed'));
ALTER TABLE rules ADD COLUMN priority INTEGER NOT NULL DEFAULT 100;
ALTER TABLE rules ADD COLUMN all_items_confirmed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rules ADD COLUMN needs_reconfiguration INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rules ADD COLUMN card_pool_id TEXT;
ALTER TABLE rules ADD COLUMN template_id TEXT;
ALTER TABLE rules ADD COLUMN template_bindings TEXT;
ALTER TABLE rules ADD COLUMN review_config TEXT;

-- 索引重建(001 FR-008 修订,research D1):同范围同触发同优先级唯一,
-- 存量行 trigger_type='order_paid'、priority=100 天然满足,零迁移风险
DROP INDEX idx_rules_enabled_scope;
CREATE UNIQUE INDEX idx_rules_enabled_scope2
  ON rules(account_id, item_id, sku_key, trigger_type, priority) WHERE enabled = 1;

CREATE TABLE rule_variants (
    id TEXT PRIMARY KEY,
    rule_id TEXT NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    spec_name TEXT NOT NULL DEFAULT '',
    spec_value TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL CHECK (source IN ('card_pool','template')),
    card_pool_id TEXT,
    template_id TEXT,
    template_bindings TEXT,
    units_per_item INTEGER NOT NULL DEFAULT 1 CHECK (units_per_item >= 1 AND units_per_item <= 100),
    delay_override_seconds INTEGER,
    position INTEGER
);
CREATE INDEX idx_rule_variants_rule ON rule_variants(rule_id, position);

-- ===== US3 关键词回复 / 默认回复 / 求评 =====

CREATE TABLE reply_rules (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    keyword TEXT NOT NULL,
    reply_kind TEXT NOT NULL CHECK (reply_kind IN ('text','image')),
    reply_text TEXT,
    reply_image_url TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_reply_rules_account ON reply_rules(account_id, enabled);

CREATE TABLE reply_rule_items (
    reply_rule_id TEXT NOT NULL REFERENCES reply_rules(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL,
    PRIMARY KEY (reply_rule_id, item_id)
);

CREATE TABLE default_replies (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id),
    enabled INTEGER NOT NULL DEFAULT 0,
    reply_text TEXT,
    reply_image_url TEXT,
    reply_once INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

CREATE TABLE default_reply_log (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    buyer_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('accepted','failed','unknown')),
    sent_at TEXT NOT NULL
);
CREATE INDEX idx_default_reply_log_buyer ON default_reply_log(account_id, buyer_id, state);

CREATE TABLE review_reminder_state (
    order_db_id TEXT NOT NULL REFERENCES orders(id),
    rule_id TEXT NOT NULL REFERENCES rules(id),
    reminded_count INTEGER NOT NULL DEFAULT 0,
    last_reminded_at TEXT,
    next_due_at TEXT,
    PRIMARY KEY (order_db_id, rule_id)
);
