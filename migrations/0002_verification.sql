-- 003 安全验证自动化:审计表(只增不改)+ issues.metadata(转人工携带 verification_url)
CREATE TABLE verification_attempts (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    trigger_source TEXT NOT NULL,          -- mtop | ws | qr | manual
    trigger_reason TEXT NOT NULL,
    verification_url TEXT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome TEXT NOT NULL DEFAULT 'in_progress', -- in_progress | succeeded | failed_manual
    duration_ms INTEGER,
    credential_updated INTEGER NOT NULL DEFAULT 0,
    failure_reason TEXT
);
CREATE INDEX idx_verification_attempts_account ON verification_attempts(account_id, started_at);

ALTER TABLE issues ADD COLUMN metadata TEXT;
