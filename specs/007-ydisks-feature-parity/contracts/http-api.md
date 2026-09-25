# HTTP API Contracts: 007-ydisks-feature-parity

全部端点挂 `/api/v1` 下,沿用既有约定:管理员会话鉴权(`require_admin_public`)、变更请求需 `X-CSRF-Token`、非 GET 自动 `X-Idempotency-Key`(前端 http.ts 已内置)、错误信封 `{"error":{code,message,request_id,retryable}}`、写接口乐观锁 `expected_version`。错误码集见 [../research.md D14](../research.md)。

## 1. 卡密库存(card-pools)

| Method | Path | 用途 | 成功 | 主要错误 |
|---|---|---|---|---|
| GET | `/card-pools?kind=&search=` | 组列表(data 组带库存计数) | 200 | — |
| POST | `/card-pools` | 创建组(按 kind 带内容/条目/api_config) | 201 | 422 invalid_request(校验) |
| GET | `/card-pools/{id}` | 详情(含分状态计数) | 200 | 404 resource_not_found |
| PUT | `/card-pools/{id}` | 更新(text/image 换内容=整体替换) | 200 | 409 version_conflict |
| DELETE | `/card-pools/{id}` | 删除(被引用拒绝) | 204 | 409 referenced_resource |
| POST | `/card-pools/{id}/append-data` | 批量组追加 `{lines: []}` | 200 `{appended, skipped_empty}` | 422 invalid_request |
| POST | `/card-pools/batch-import` | multipart 文件导入(xlsx/csv/tsv) | 200 `{total, succeeded, failed:[{row,error}]}` | 413 payload_too_large |
| POST | `/card-pools/test-api` | API 配置一次性测试(不落库) | 200 `{ok, sample?, latency_ms?, error?}` | 422 invalid_request |
| GET | `/card-pools/{id}/entries?state=&cursor=` | 条目审计列表(**永不返回明文**) | 200 Page | 404 |

**CardPoolDto**: `{id, name, kind, enabled, delay_seconds, description, version, stock?: {available, reserved, used}, content_set?: bool, api_config?: object}`。

## 2. 发货模板(delivery-templates)

| Method | Path | 用途 | 错误 |
|---|---|---|---|
| GET | `/delivery-templates` | 列表含 `used_by_rules` 计数 | — |
| POST | `/delivery-templates` | 创建 `{name, enabled, messages[]}` | 422(占位符拼写/命名/条数/长度);409 无 |
| PUT | `/delivery-templates/{id}` | 更新(整体替换消息列表) | 409 version_conflict |
| DELETE | `/delivery-templates/{id}` | 删除 | **409 referenced_resource**(被规则/变体引用) |

**TemplateDto**: `{id, name, enabled, messages: string[], keys: {cards: string[], custom: string[]}, used_by_rules: number, version}`。

## 3. 自动化规则(扩展现有 `/accounts/{account_id}/rules`)

| Method | Path | 变化 |
|---|---|---|
| GET | `/accounts/{id}/rules?trigger_type=&enabled=&search=` | RuleDto 扩展 + `trigger_counts` 汇总 |
| POST / PUT | 创建/更新 | body 扩展(下);保存冲突语义 = 同范围同触发同优先级(422 rule_conflict) |
| POST | `.../rules/preview` | 渲染预览支持模板/卡密来源;卡密内容以 `[卡密内容 ×N]` 掩码,模板逐条返回 `rendered_messages[]` |
| POST | `.../rules/match-preview` | 不变 |
| GET/POST | `/accounts/{id}/reply-rules` | 关键词回复 CRUD;PUT/DELETE `.../reply-rules/{rid}` |
| GET/PUT | `/accounts/{id}/default-reply` | 默认回复;POST `.../default-reply/clear-records` 清空 reply 记录 |
| PUT | `/accounts/{id}/ai-settings` | `{ai_reply_enabled, ai_prompt}`(账号 AI 开关) |

**RuleDto 扩展**: `{..., trigger_type, priority, all_items_confirmed, needs_reconfiguration, content_source: "fixed_text"|"card_pool"|"template", card_pool_id?, template_id?, template_bindings?, variants?: [{spec_name, spec_value, source, card_pool_id?, template_id?, template_bindings?, units_per_item, delay_override_seconds}], review_config?: {wait_hours, interval_hours, max_count, text}, version}`。
保存约束:账号级(item_id 空)必须 `all_items_confirmed=true` 才落库生效,否则落库为 `需确认·暂不发货`(needs_reconfiguration 语义复用);`buyer_reviewed` 触发仅当 capabilities 声明后接受创建(否则 422 unsupported_capability)。

## 4. 在线聊天(chat)

| Method | Path | 用途 | 错误 |
|---|---|---|---|
| GET | `/chat/unread-summary` | `{total, by_account: {aid: n}}`(侧边栏/Tab 徽标) | — |
| GET | `/accounts/{aid}/chat/sessions?search=&unread_only=&cursor=` | 会话分页 | — |
| GET | `/chat/conversations/{cid}/messages?before_id=&limit=` | 消息游标(新→旧) | 404 |
| POST | `/chat/conversations/{cid}/messages` | `{text}` ≤2000 字;同步等待发送结果(≤15s) | 422 content_too_long;403 account_unavailable(离线) |
| POST | `/chat/conversations/{cid}/images` | multipart ≤10MB(能力门禁) | 413 payload_too_large;403 unsupported_capability |
| POST | `/chat/messages/{mid}/retry` | failed 重发(幂等键承载) | 422 unsafe_retry(uncertain 不可重试) |
| POST | `/chat/conversations/{cid}/read` | 未读清零 | — |
| DELETE | `/chat/conversations/{cid}` | 本机隐藏+清空展示消息 | — |
| GET/POST | `/chat/quick-replies` | 列表/新增(≤50 条) | 422 reply_limit_reached |
| DELETE | `/chat/quick-replies/{id}` | 删除 | — |
| GET/PUT | `/chat/accounts/{aid}/buyer-notes/{buyer_id}` | 买家备注(≤2000) | — |

**MessageDto**: `{id, direction, msg_kind, body_text?, image_path?, status?(out), error_hint?, item_snapshot?, created_at, read_at?}`。能力声明扩展(GET /capabilities 增):`chat_send_image: bool`、`chat_history_backfill: bool`(false 时前端隐藏图片入口并显示"仅展示接入后收到的会话")。

## 5. 通知(notification-channels)

| Method | Path | 用途 | 错误 |
|---|---|---|---|
| GET | `/notification-channels` | 列表(`secrets_configured: [key]` 脱敏) | — |
| POST | `/notification-channels` | 创建 `{kind, name, config, secrets?, event_types}` | 422(kind/config 校验) |
| GET/PUT/DELETE | `/notification-channels/{id}` | 详情/更新(secrets 留空=不修改)/删除 | 409 version_conflict |
| POST | `/notification-channels/{id}/test` | 测试投递 | 200 `{state: accepted\|not_sent\|unknown, error_hint?}` |
| GET/PUT | `/accounts/{aid}/notification-bindings` | `{channel_ids[]}` 覆盖式绑定 | — |
| GET/PUT | `/settings/system-smtp` | 系统 SMTP(secrets 脱敏) | — |

event_types 合法值:`account_offline, account_recovered, security_verification, manual_intervention_required, delivery_result, system_error`(空=全部)。

## 6. 订单同步与人工交付动作

| Method | Path | 用途 | 错误 |
|---|---|---|---|
| POST | `/orders/syncs` | `{account_ids?: [], window_days?: 7}` → 202 job(kind=order_sync),result_ref 含逐账号 `{orders_seen, created, restored, reassigned, ineligible, failed[], coverage, offline?}` | 422 |
| POST | `/accounts/{aid}/orders/{oid}/syncs` | 单笔同步,同步返回 HandleOutcome | 403 account_unavailable |
| POST | `/accounts/{aid}/orders/{oid}/deliveries` | **人工触发交付** `{reason}`(核验缺失 → 422 附缺失要素) | 422 ineligible_order / incomplete_order;409 idempotency_conflict |
| POST | `/accounts/{aid}/orders/{oid}/confirm-shipments` | **人工确认平台已发货** `{reason}` | 422(交付未 accepted 时) |

既有 `GET /orders`、详情、resends/mark-received/terminate/takeovers 不变;issues 的 `allowed_actions` 增 `"trigger_delivery"`。

## 7. 系统与AI(settings)

| Method | Path | 用途 | 错误 |
|---|---|---|---|
| GET | `/settings/system` | `{ai_api_url, ai_model, ai_key_configured, smtp: {..., password_configured}}` | — |
| PUT | `/settings/system` | 更新(ai_api_key 留空=不修改) | 422 |
| POST | `/settings/ai/models` | `{base_url?}` 拉取模型列表 | 422/504(结构化原因) |
| POST | `/settings/ai/test` | 测试连接 → `{model, latency_ms, reply}` | 422/504 |
| PUT | `/auth/credentials` | `{current_password, new_username?, new_password?}`;成功后其他会话全部失效 | 422 credential_change_failed |
| GET | `/stats/overview` | 增可选字段 `stock?: {available_total}`(缺失降级,不闪 0) | — |

## 8. 前端契约层落点

`shared/contracts.ts` 新增对应 interface + parse 守卫(只校验消费字段、缺失降级);分页复用 `parsePage`;`ApiErrorBody.code` 收录 D14 新码。各 feature 私有 parse 放 `features/<f>/model.ts`。
