# Data Model: 007-ydisks-feature-parity

三个只追加迁移(0004/0005/0006)。列类型遵循既有惯例:`TEXT` 主键(UUIDv4)、时间戳 `TEXT`(RFC3339 UTC)、金额 `INTEGER`(minor units)、加密列四件套 `*_ciphertext BLOB + *_nonce BLOB + key_id TEXT + format_version INTEGER`(AES-GCM 信封,`domain/crypto.rs`)。

## 增量 1:`migrations/0004_cards_templates_rules.sql`

### card_pools(卡密组)

| 列 | 类型 | 约束/说明 |
|---|---|---|
| id | TEXT PK | |
| name | TEXT NOT NULL | 组名,唯一性按应用层校验 |
| kind | TEXT NOT NULL | CHECK `data`/`text`/`image`/`api` |
| enabled | INTEGER NOT NULL DEFAULT 1 | |
| delay_seconds | INTEGER NOT NULL DEFAULT 0 | CHECK 0–3600 |
| description | TEXT NOT NULL DEFAULT '' | |
| content_ciphertext/content_nonce/key_id/format_version | BLOB/BLOB/TEXT/INTEGER NULL | kind=text/image 时的固定内容信封(AAD purpose `card_pool_content`) |
| api_config_ciphertext/api_config_nonce/api_key_id/api_format_version | 同上 NULL | kind=api 时整份配置 JSON 信封(url/method/timeout_ms/headers/params/body/content_type/response_path/retry_enabled) |
| version | INTEGER NOT NULL DEFAULT 1 | 乐观锁 |
| created_at / updated_at | TEXT NOT NULL | |

### card_entries(卡密条目)

| 列 | 类型 | 说明 |
|---|---|---|
| id | TEXT PK | |
| pool_id | TEXT NOT NULL REFERENCES card_pools(id) ON DELETE CASCADE | |
| state | TEXT NOT NULL DEFAULT 'available' | CHECK `available`/`reserved`/`used`/`disabled`/`pending_api` |
| origin | TEXT NOT NULL DEFAULT 'stock' | CHECK `stock`/`api`;api 行同时是取卡审计(含 request_key/raw 响应) |
| content_ciphertext/content_nonce/key_id/format_version | NOT NULL | 单卡内容信封(AAD purpose `card_entry`) |
| content_digest | TEXT NOT NULL | 明文 SHA-256,池内去重与重试判重 |
| reserved_order_id / reserved_delivery_id | TEXT NULL | 预留绑定 |
| request_key | TEXT NULL | origin=api 的取卡请求标识 |
| reserved_at / used_at / created_at | TEXT | |

索引:`idx_card_entries_pool_state (pool_id, state)`;`idx_card_entries_order (reserved_order_id)`。
不变量(领域层+SQL 双保险):`reserved/used` 条目的 reserved_order_id 非空;`used` 永不复用;释放仅允许 `reserved→available`。

### delivery_templates / delivery_template_messages

- `delivery_templates`:id PK、name NOT NULL、enabled DEFAULT 1、version、created_at/updated_at。
- `delivery_template_messages`:id INTEGER PK、template_id REFERENCES ON DELETE CASCADE、position INTEGER NOT NULL、body TEXT NOT NULL(**明文**:仅含占位符,不含机密;≤10 条/模板,单条 ≤1000 unicode 标量,与规则内容限制一致)、UNIQUE(template_id, position)。
- 占位符语法:`{{buyer_nickname}} {{order_id}} {{buyer_id}} {{card_name}} {{cards.<key>}} {{custom.<key>}}`,key 限 `[A-Za-z0-9_-]`;`cards.<key>` 需在 `template_bindings` 出现,`custom.<key>` 需在规则绑定赋值——保存时校验(应用层)。

### rules(扩展列)与 rule_variants

ALTER:`trigger_type TEXT NOT NULL DEFAULT 'order_paid'`(CHECK 集合 `order_paid`/`review_missing_timeout`/`buyer_reviewed`[能力门禁])、`priority INTEGER NOT NULL DEFAULT 100`、`all_items_confirmed INTEGER NOT NULL DEFAULT 0`、`needs_reconfiguration INTEGER NOT NULL DEFAULT 0`、`card_pool_id TEXT NULL`、`template_id TEXT NULL`、`template_bindings TEXT NULL`(JSON:`{cards:[{key,pool_id,units}], custom:{k:v}}`)。

**索引重建(001 FR-008 修订,见 research D1)**:
```sql
DROP INDEX idx_rules_enabled_scope;
CREATE UNIQUE INDEX idx_rules_enabled_scope2
  ON rules(account_id, item_id, sku_key, trigger_type, priority) WHERE enabled = 1;
```
存量行(defaults)自动满足;`sku_key=''`/`item_id=''` 哨兵语义不变(账号级范围,item_id='' + all_items_confirmed)。

`rule_variants`:id PK、rule_id REFERENCES ON DELETE CASCADE、spec_name/spec_value TEXT NOT NULL DEFAULT ''(分号分隔多规格)、source CHECK `card_pool`/`template`、card_pool_id/template_id NULL、template_bindings JSON 同上、units_per_item INTEGER DEFAULT 1 CHECK 1–100、delay_override_seconds INTEGER NULL、position INTEGER。
匹配语义:规则无变体 → 规则级来源适用全部规格;有变体 → 变体的 (spec_name→spec_value) 对全部出现在订单 sku_pairs 中才命中。

### reply_rules(关键词回复)与关联

- `reply_rules`:id PK、account_id NOT NULL、keyword TEXT NOT NULL、reply_kind CHECK `text`/`image`、reply_text NULL、reply_image_url NULL、enabled DEFAULT 1、created_at/updated_at。
- `reply_rule_items`:reply_rule_id REFERENCES CASCADE、item_id NOT NULL、PK(reply_rule_id, item_id)。(无关联行 = 账号级)

### default_replies 与发送记录

- `default_replies`:account_id PK、enabled DEFAULT 0、reply_text/reply_image_url、reply_once INTEGER DEFAULT 1、updated_at。
- `default_reply_log`:id PK、account_id、buyer_id、state CHECK `accepted`/`failed`/`unknown`、sent_at。reply_once 判定 = 同 (account,buyer) 存在 accepted 行。

### review_reminder_state(求评计划)

(order_db_id, rule_id) PK、reminded_count INTEGER DEFAULT 0、last_reminded_at NULL、next_due_at NULL。求评文案与节奏参数存规则变体外的规则级 JSON 列 `review_config TEXT NULL`(wait_hours/interval_hours/max_count/text)。

## 增量 2:`migrations/0005_chat.sql`

### conversations

id PK、account_id NOT NULL、peer_buyer_id NOT NULL、peer_nickname NULL、chat_id NULL、last_item_id NULL、last_message_at TEXT、last_message_preview TEXT NULL、unread_count INTEGER DEFAULT 0、hidden_at NULL;UNIQUE(account_id, peer_buyer_id);索引 (account_id, hidden_at, last_message_at)。

### chat_messages

id TEXT PK(时间有序 UUID)、conversation_id REFERENCES ON DELETE CASCADE、direction CHECK `in`/`out`、msg_kind CHECK `text`/`image`/`system`/`item_card`/`unknown`、body_text TEXT NULL(**明文**,research D15 权衡)、image_path TEXT NULL(数据目录 uploads/ 相对路径)、status TEXT NULL(out 专用:CHECK `sending`/`sent`/`failed`/`uncertain`/`cancelled`)、platform_message_id NULL、error_hint NULL、item_snapshot JSON NULL(item_card 展示数据)、created_at、read_at NULL。
索引:(conversation_id, created_at);(status) WHERE status IN ('sending','uncertain')。

状态机(仅 out):`sending → sent(Accepted) | failed(Rejected/NotSubmitted 不可重试) | uncertain(Unknown)`;`failed` 允许人工重试(新 attempt 语义由幂等键承载);`uncertain` 仅人工核对,永不自动重发。

### quick_replies / buyer_notes

- `quick_replies`:id PK、body TEXT NOT NULL(≤2000 字,应用层上限 50 条)、created_at。
- `buyer_notes`:PK(account_id, buyer_id)、note TEXT DEFAULT ''(≤2000)、updated_at。

## 增量 3:`migrations/0006_notify_ai.sql`

### notification_channels / bindings / deliveries

- `notification_channels`:id PK、kind CHECK `webhook`/`email`/`dingtalk`/`feishu`/`wecom`/`bark`/`telegram`、name NOT NULL、enabled DEFAULT 1、config TEXT NOT NULL(**非机密字段明文 JSON**:URL、chat_id、from_name、端口、加密方式等)、secrets_ciphertext/secrets_nonce/secrets_key_id/secrets_format_version NULL(**机密字典整包信封**:token/secret/password/smtp_user 等)、event_types TEXT DEFAULT '[]'(JSON 数组,空=全部)、version、created_at/updated_at。读取端点返回 `secrets_configured: [key...]` 脱敏。
- `notification_bindings`:PK(account_id, channel_id),channel_id REFERENCES CASCADE。
- `notification_deliveries`:id PK、channel_id NULL(渠道删除后保留审计)、event_kind TEXT、subject TEXT、state CHECK `accepted`/`not_sent`/`unknown`、error_hint NULL、created_at;索引 (created_at DESC)。

### system_settings / system_secrets

- `system_settings`:key PK(value 明文:ai_api_url、ai_model、smtp_host/port/encryption/from_address/from_name)。
- `system_secrets`:key PK(ai_api_key、smtp_password)、信封四件套 NOT NULL。

### accounts 扩展与 issues 索引

- `ALTER TABLE accounts ADD COLUMN ai_reply_enabled INTEGER NOT NULL DEFAULT 0; ADD COLUMN ai_prompt TEXT NULL;`
- 无订单 issue 去重:`CREATE UNIQUE INDEX idx_issues_open_dedup_account ON issues(account_id, kind, reason_code) WHERE state='open' AND order_id IS NULL AND account_id IS NOT NULL;`(既有 `idx_issues_open_dedup` 要求 order_id NOT NULL,见 research D8。)

### 不新增 schema 的能力

- 订单同步:`operation_jobs(kind='order_sync')` 复用。
- 人工动作:`manual_actions.action` 新字符串值(`trigger_delivery`/`confirm_shipment`);issues 新 kind(`stock_insufficient`/`notify_unknown`/`review_reminder_failed`)走自由 TEXT。
- 概览第 5 卡:`repos/stats.rs` 新只读查询 `SUM(可用余量)`,无新表。

## 领域状态机汇总

```text
card_entries:  available → reserved → used
                    ↑          ├→ used(确定成功,批量扣减)
                    └──────────┘(确定未发送:释放)
                               └→ reserved 持有(unknown/终态前)→ issue 人工裁决
chat_messages(out): sending → sent | failed(→人工重试) | uncertain(仅人工)
notify_deliveries: accepted | not_sent | unknown(→ issue notify_unknown)
rules:  enabled ⇄ disabled;needs_reconfiguration=1 ⇒ 执行旁路 + 列表警示(保存修复后清除)
```

## 迁移机制落点

`src/adapters/sqlite/migrations.rs::MIGRATIONS` 常量表追加 `(4, include_str!("../../../migrations/0004_cards_templates_rules.sql"))` 等三行;**迁移测试硬编码的"表数量=25"断言同步更新(0004:+10 表,0005:+4 表,0006:+5 表 → 44;以 tasks 实现时实际清点为准)**。
