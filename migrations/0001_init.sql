-- 初始 schema v1:按 specs/001-xianyu-auto-delivery/data-model.md 实体表。
-- 约定:本地 ID 为 UUID 字符串;业务时间为 UTC 毫秒(INTEGER);
-- 加密列保存 ciphertext/nonce/key_id/format_version(AAD 由应用层构造);
-- 不建立卡密库存、计费或通用插件表(范围外)。

CREATE TABLE installation (
    id TEXT PRIMARY KEY CHECK (id = 'singleton'),
    schema_version INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    restore_epoch INTEGER NOT NULL DEFAULT 0,
    quarantine_started_at INTEGER,
    restore_manifest_id TEXT
);

CREATE TABLE admin (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    password_changed_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    admin_id TEXT NOT NULL REFERENCES admin(id),
    csrf_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX idx_sessions_expiry ON sessions(expires_at);

CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    external_user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    runtime_enabled INTEGER NOT NULL DEFAULT 0,
    auto_delivery_enabled INTEGER NOT NULL DEFAULT 0,
    auto_confirm_enabled INTEGER NOT NULL DEFAULT 0,
    monitor_since INTEGER,
    status TEXT NOT NULL DEFAULT 'disabled',
    control_epoch INTEGER NOT NULL DEFAULT 0,
    credential_epoch INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (platform, external_user_id)
);

CREATE TABLE account_credentials (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id),
    cookie_ciphertext BLOB NOT NULL,
    cookie_nonce BLOB NOT NULL,
    metadata_ciphertext BLOB,
    metadata_nonce BLOB,
    key_id TEXT NOT NULL,
    format_version INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE TABLE auth_flows (
    id TEXT PRIMARY KEY,
    account_id TEXT REFERENCES accounts(id),
    generation INTEGER NOT NULL,
    status TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    bound_user_id TEXT,
    session_ref TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE items (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    external_item_id TEXT NOT NULL,
    title TEXT NOT NULL,
    listing_state TEXT NOT NULL DEFAULT 'unknown',
    sku_definition TEXT NOT NULL DEFAULT '[]',
    sku_completeness TEXT NOT NULL DEFAULT 'incomplete',
    last_seen_sync TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (account_id, external_item_id)
);

CREATE TABLE sync_jobs (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    cursor TEXT,
    coverage_from INTEGER,
    coverage_to INTEGER,
    complete INTEGER NOT NULL DEFAULT 0,
    gap_reason TEXT,
    generation INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_sync_jobs_account ON sync_jobs(account_id, kind, created_at);

CREATE TABLE rules (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    item_id TEXT NOT NULL REFERENCES items(id),
    sku_key TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    current_content_version INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- 同一完整匹配范围最多启用一条规则(FR-008)
CREATE UNIQUE INDEX idx_rules_enabled_scope ON rules(account_id, item_id, sku_key) WHERE enabled = 1;

CREATE TABLE rule_contents (
    id TEXT PRIMARY KEY,
    rule_id TEXT NOT NULL REFERENCES rules(id),
    content_version INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    key_id TEXT NOT NULL,
    format_version INTEGER NOT NULL,
    text_digest TEXT NOT NULL,
    char_count INTEGER NOT NULL,
    byte_count INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (rule_id, content_version)
);

CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    external_order_id TEXT NOT NULL,
    buyer_id TEXT,
    item_id TEXT,
    sku_pairs TEXT,
    sku_complete INTEGER NOT NULL DEFAULT 0,
    amount_minor INTEGER,
    currency TEXT,
    quantity INTEGER,
    paid_at INTEGER,
    trade_type TEXT NOT NULL DEFAULT 'unknown',
    platform_status TEXT NOT NULL DEFAULT 'unknown',
    observed_at INTEGER,
    fact_version INTEGER NOT NULL DEFAULT 0,
    role_verified INTEGER NOT NULL DEFAULT 0,
    history_class TEXT NOT NULL DEFAULT 'unknown',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- 外部唯一身份三元组:不能只用订单号(data-model)
    UNIQUE (platform, account_id, external_order_id)
);
CREATE INDEX idx_orders_account_status_updated ON orders(account_id, platform_status, updated_at);
CREATE INDEX idx_orders_paid_at ON orders(paid_at);

CREATE TABLE order_facts (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(id),
    source TEXT NOT NULL,
    source_event_id TEXT,
    platform_revision TEXT,
    observed_at INTEGER NOT NULL,
    normalized_evidence TEXT NOT NULL,
    evidence_digest TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_order_facts_order ON order_facts(order_id);

CREATE TABLE inbound_events (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    source_event_id TEXT,
    payload_digest TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    processed_at INTEGER,
    result TEXT
);
-- 有稳定事件 ID 时唯一;否则摘要仅辅助去重
CREATE UNIQUE INDEX idx_inbound_events_source ON inbound_events(account_id, source_event_id) WHERE source_event_id IS NOT NULL;

CREATE TABLE deliveries (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(id),
    kind TEXT NOT NULL,
    parent_delivery_id TEXT REFERENCES deliveries(id),
    rule_id TEXT,
    content_snapshot_id TEXT,
    content_state TEXT NOT NULL,
    review_state TEXT NOT NULL DEFAULT 'none',
    confirmation_state TEXT NOT NULL DEFAULT 'disabled',
    evidence_origin TEXT NOT NULL DEFAULT 'none',
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    version INTEGER NOT NULL DEFAULT 1,
    control_epoch INTEGER,
    credential_epoch INTEGER,
    restore_epoch INTEGER NOT NULL DEFAULT 0,
    execution_policy TEXT NOT NULL DEFAULT 'auto',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- initial 每订单唯一(FR-014)
CREATE UNIQUE INDEX idx_deliveries_initial ON deliveries(order_id) WHERE kind = 'initial';
CREATE INDEX idx_deliveries_next_retry ON deliveries(next_retry_at) WHERE next_retry_at IS NOT NULL;
CREATE INDEX idx_deliveries_order ON deliveries(order_id);

CREATE TABLE content_snapshots (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(id),
    source_content_id TEXT NOT NULL,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    key_id TEXT NOT NULL,
    format_version INTEGER NOT NULL,
    digest TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL REFERENCES deliveries(id),
    action_kind TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    request_id TEXT NOT NULL,
    state TEXT NOT NULL,
    prepared_at INTEGER NOT NULL,
    handoff_at INTEGER,
    finished_at INTEGER,
    result_code TEXT,
    proof_ref TEXT,
    credential_epoch INTEGER,
    control_epoch INTEGER,
    UNIQUE (delivery_id, action_kind, sequence)
);
CREATE INDEX idx_attempts_delivery ON attempts(delivery_id);

CREATE TABLE delivery_proofs (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    origin TEXT NOT NULL,
    platform_message_id TEXT,
    request_id TEXT NOT NULL,
    buyer_id TEXT,
    chat_id TEXT,
    content_digest TEXT NOT NULL,
    accepted_at INTEGER NOT NULL,
    manual_action_id TEXT
);

CREATE TABLE order_execution_guards (
    order_id TEXT PRIMARY KEY REFERENCES orders(id),
    delivery_id TEXT,
    attempt_id TEXT,
    execution_generation INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL
);

CREATE TABLE issues (
    id TEXT PRIMARY KEY,
    order_id TEXT,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    delivery_id TEXT,
    kind TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    allowed_actions TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL DEFAULT 'open',
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_issues_state_created ON issues(state, created_at);
-- 同一未解决原因不反复创建(FR-024)
CREATE UNIQUE INDEX idx_issues_open_dedup ON issues(order_id, kind, reason_code) WHERE state = 'open' AND order_id IS NOT NULL;

CREATE TABLE manual_actions (
    id TEXT PRIMARY KEY,
    admin_id TEXT NOT NULL REFERENCES admin(id),
    order_id TEXT NOT NULL REFERENCES orders(id),
    delivery_id TEXT,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    risk_confirmed INTEGER NOT NULL DEFAULT 0,
    request_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    result_ref TEXT
);
CREATE INDEX idx_manual_actions_order ON manual_actions(order_id);

CREATE TABLE command_receipts (
    idempotency_key TEXT PRIMARY KEY,
    actor_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    resource_id TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    result_json TEXT
);

CREATE TABLE operation_jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    target_id TEXT,
    state TEXT NOT NULL DEFAULT 'queued',
    stage TEXT NOT NULL DEFAULT '',
    version INTEGER NOT NULL DEFAULT 1,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    result_ref TEXT,
    safe_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_operation_jobs_state ON operation_jobs(state, updated_at);

CREATE TABLE restore_reviews (
    id TEXT PRIMARY KEY,
    restore_epoch INTEGER NOT NULL,
    order_id TEXT,
    account_id TEXT,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unresolved',
    evidence_origin TEXT,
    reason TEXT,
    reviewer TEXT,
    reviewed_at INTEGER,
    version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_restore_reviews_epoch ON restore_reviews(restore_epoch, status);

CREATE TABLE backup_manifests (
    id TEXT PRIMARY KEY,
    database_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    app_version TEXT NOT NULL,
    snapshot_started_at INTEGER NOT NULL,
    snapshot_finished_at INTEGER NOT NULL,
    key_id TEXT NOT NULL,
    checksum TEXT NOT NULL,
    restore_policy TEXT NOT NULL
);
