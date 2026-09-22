# HTTP Contract: 轻交付首版

日期：2026-09-22。设计契约，尚未实现。依据 [spec](../spec.md)、[research](../research.md)。
管理界面为原生 TypeScript；Rust 服务嵌入静态资源。接口默认仅服务
`http://127.0.0.1:59189`，所有业务路由使用 `/api/v1`。

## 1. 传输、认证和通用约定

- JSON 字段使用 snake_case；标识符均为不透明字符串，不能从 ID 猜测账号归属。
  时间为 UTC RFC 3339，版本为非负整数，金额为 `{minor_units: integer, currency: string}`，禁止浮点金额。
- `GET /health` 仅返回 `HealthResponse {status: "ok"|"degraded", version: string}`，不暴露账号、凭证和数据库路径。
  成功为 200，不可正常提供核心服务为 503。所有业务接口默认要求管理员服务端会话。
- 会话 Cookie 名 `qing_session`，随机不可预测、HttpOnly、SameSite=Strict、Path=/。
  首版本机 HTTP 不错误依赖浏览器不会发送的 Secure Cookie；不采用要求 HTTPS 的 `__Host-` 名称。
  不把 Cookie、CSRF、密码或正文写入 localStorage、日志或诊断导出。
- 校验 Host 的允许回环地址及配置端口；浏览器变更请求必须来自相同 Origin，无通配 CORS。
  开发 Vite 通过代理维持同源语义，额外允许来源仅限显式开发配置中的固定回环Origin；
  开发live与mock均须鉴权和CSRF，发行版不启用这些额外来源。
- `GET /auth/session` 建立短寿命匿名预会话并返回 CSRF token；初始化、登录也必须发送
  `X-CSRF-Token`。授权后轮换会话和 CSRF，退出立即废弃服务端会话。响应 `Cache-Control: no-store`。
  管理会话绝对有效期 12 小时；CLI 的离线维护要求目录独占和同用户 DPAPI，不开 HTTP 认证后门。
- 初始化只能在不存在管理员时成功，数据库唯一约束抵御两个页面同时初始化。
  密码 12 字符起，使用 Argon2id；认证失败统一错误，登录限流且不回显密码。
- `Idempotency-Key` 是产生任务/外部动作的 POST 请求必需头，由调用方为一次用户意图生成。
  服务端按管理员、操作和目标绑定请求摘要及结果；相同键相同参数返回原结果，参数不同返回 409。
  网络超时后保留原键重试，不生成新键；新的一次明确补发才使用新键。
- 修改既有资源携带 `expected_version`，原子比较更新。幂等重放先返回原操作结果，
  不因原请求已改变版本把成功重放误判为冲突。版本冲突为 409，调用方刷新后让用户确认。
- 列表使用 `limit`（默认 50，上限 100）和不透明 `cursor`；稳定按 `(created_at,id)` 排序。
  `Page<T> {items: T[], next_cursor: string|null}`。游标绑定筛选条件，非法游标返回 400。
- 纯读取超时/页面切换可以取消 fetch；它不等于取消服务端任务。任务接受后返回
  `202 Accepted`、`Location: /api/v1/jobs/{id}` 和 `AcceptedOperation`，不假报动作已完成。

### 通用 DTO 与错误

```text
ErrorResponse {error: ApiError}
ApiError {code: ErrorCode, message: string, request_id: string,
          retryable: boolean, field_errors?: FieldError[], current_version?: integer,
          job_id?: string}
FieldError {field: string, code: string, message: string}
AcceptedOperation {operation_id: string, job: JobSummary}
JobSummary {id: string, kind: JobKind, state: JobState, version: integer,
            created_at: timestamp, updated_at: timestamp, target_id?: string}
JobDetail {summary: JobSummary, stage: string, progress?: JobProgress,
           result?: JobResult, error?: ApiError, cancel_allowed: boolean}
JobProgress {completed: integer, total?: integer}
JobResult {resource_kind: "account"|"item_sync"|"order"|"issue"|"delivery"|"control"|"reconciliation",
           resource_id: string, outcome: string}
JobState = queued | running | cancel_requested | succeeded | failed | cancelled | needs_review
JobKind = account_control | account_verification | item_sync | reconciliation |
          delivery | manual_resend | shipment_confirmation | history_takeover
```

错误必须匹配 HTTP 状态，不返回 `200 + success:false`：

| HTTP | ErrorCode 示例 | 含义 |
| --- | --- | --- |
| 400 | invalid_request, invalid_cursor | JSON、类型或查询格式错误 |
| 401 | authentication_required, invalid_credentials | 未登录或认证失败 |
| 403 | csrf_rejected, origin_rejected | 请求来源或 CSRF 校验失败 |
| 404 | resource_not_found | 资源不存在；不暴露其他账号身份 |
| 409 | version_conflict, action_in_progress, idempotency_conflict, already_initialized, restore_quarantined | 并发/阶段冲突 |
| 422 | unsupported_capability, rule_conflict, incomplete_order, content_too_long, ineligible_order, unsafe_retry | 业务前提不满足 |
| 429 | rate_limited | 限流，携带 Retry-After |
| 503 | persistence_unavailable, account_unavailable, service_stopping | 无法安全受理；无持久记录不得发送 |

`message` 只含脱敏可操作说明；原始平台响应、Cookie、Token 和正文不得进入错误信封。

## 2. 身份、能力与账号

```text
SessionResponse {initialized: boolean, authenticated: boolean, csrf_token: string,
                 administrator?: AdministratorSummary}
AdministratorSummary {id: string, display_name: string}
InitializeRequest {password: string, password_confirmation: string}
LoginRequest {password: string}
CapabilitiesResponse {platforms: PlatformCapability[], content_sources: ContentCapability[],
                      content_limits: ContentLimits, execution_profile: "live"|"mock"}
PlatformCapability {platform: string, supported: boolean, reason?: string}
ContentCapability {kind: string, supported: boolean, reason?: string}
ContentLimits {unicode_scalars: integer, utf8_bytes: integer, effective_source: "product"|"platform"}
AccountSummary {id: string, platform: "xianyu", platform_user_id: string, display_name: string,
                connection_state: ConnectionState, control: AccountControl,
                control_version: integer, monitoring_since: timestamp|null,
                restore_quarantined: boolean, last_error?: string}
AccountControl {run_enabled: boolean, auto_delivery_enabled: boolean,
                auto_confirm_enabled: boolean, transition: "stable"|"pausing"|"resuming"}
ConnectionState = connecting | online | paused | offline | authorization_expired | verification_required
AccountControlRequest {expected_version: integer, run_enabled?: boolean,
                       auto_delivery_enabled?: boolean, auto_confirm_enabled?: boolean,
                       acknowledge_monitoring_scope?: boolean}
QrCreateRequest {platform: "xianyu", replace_account_id?: string, expected_version?: integer}
QrSession {id: string, state: QrState, expires_at: timestamp, generation: integer,
           image_path: string|null, account_id: string|null, verification?: VerificationSummary}
QrState = awaiting_scan | awaiting_authorization | verification_required | authorized | expired | cancelled | failed
VerificationSummary {id: string, kind: "official_browser", state: "required"|"opening"|"pending"|"completed"|"failed",
                     instruction: string, browser_available: boolean}
StartVerificationRequest {expected_version: integer}
```

| 方法与路径（均在 /api/v1 下） | 请求 → 响应 | 行为 |
| --- | --- | --- |
| GET /auth/session | → SessionResponse | 允许匿名，只给初始化/会话状态 |
| POST /auth/initialize | InitializeRequest → 201 SessionResponse | 创建唯一管理员并登录 |
| POST /auth/login | LoginRequest → 200 SessionResponse | 轮换会话 |
| POST /auth/logout | 无正文 → 204 | 清除当前会话 |
| GET /capabilities | → CapabilitiesResponse | 仅闲鱼＋固定文字可用，其他明确 unsupported |
| GET /accounts | 分页 → Page<AccountSummary> | 不包含敏感模型 |
| GET /accounts/{account_id} | → AccountSummary | 账号状态及控制版本 |
| POST /accounts/qr-sessions | QrCreateRequest → 201 QrSession | 接入或替换指定账号授权；需幂等键 |
| GET /accounts/qr-sessions/{qr_id} | → QrSession | 每 3 秒查询；迟到扫码不替换新代次 |
| GET /accounts/qr-sessions/{qr_id}/image | → image/png | 已认证、no-store；不暴露包含凭证的 URL |
| POST /accounts/qr-sessions/{qr_id}/cancel | 无正文 → 200 QrSession | 终止本地授权接纳；已授权则 409 |
| POST /accounts/qr-sessions/{qr_id}/verification | StartVerificationRequest → 202 AcceptedOperation | 新账号扫码期间官方验证；expected_version 对应 generation |
| POST /accounts/{account_id}/control | AccountControlRequest → 202 AcceptedOperation | 幂等；暂停屏障和恢复核验 |
| POST /accounts/{account_id}/verification | StartVerificationRequest → 202 AcceptedOperation | 受控打开官方验证浏览器，不接受任意 URL |

首次启用自动交付必须 `acknowledge_monitoring_scope=true`；服务端写可信当前监控起点。
新账号运行、自动交付、平台确认三个开关均关闭。暂停屏障之前仅显示 pausing；任务成功才是已暂停。
恢复后账号默认暂停；明确恢复账号仅允许合格的新付款订单执行，不解除旧订单的逐单隔离；见第 6 节。
自动交付关闭也阻止人工补发；人工确认已收到是本地记录，可在暂停时执行。

## 3. 商品、同步与规则

```text
SkuPart {property_id: string, value_id: string, property_label: string, value_label: string}
ItemSummary {id: string, account_id: string, platform_item_id: string, title: string,
             status: "on_sale"|"off_sale"|"unknown", sku_combinations: SkuCombination[],
             rule_state: "configured"|"missing"|"disabled"|"conflict", version: integer}
SkuCombination {key: string, parts: SkuPart[]}
ItemSyncRequest {expected_account_version: integer}
ItemSyncResult {id: string, account_id: string, state: "running"|"complete"|"incomplete"|"failed",
                fetched_count: integer, started_at: timestamp, completed_at: timestamp|null,
                failure_reason?: string}
RuleSummary {id: string, account_id: string, item_id: string, sku_key: string,
             enabled: boolean, version: integer, content_length: integer, updated_at: timestamp}
RuleDetail {summary: RuleSummary, content: string, content_kind: "fixed_text"}
RuleWriteRequest {item_id: string, sku_key: string, content_kind: "fixed_text",
                  content: string, enabled: boolean, expected_version?: integer}
RulePreviewRequest {item_id: string, sku_key: string, content: string}
RulePreviewResponse {rendered_text: string, unicode_scalars: integer, utf8_bytes: integer,
                     valid: boolean, violations: FieldError[]}
MatchPreviewRequest {item_id: string, sku_key: string}
MatchPreviewResponse {state: "matched"|"none"|"ambiguous"|"unsupported", rule_id?: string,
                      rule_version?: integer, reason?: string}
```

SKU key 由后端根据完整属性 ID/值 ID 排序规范化；无规格使用明确空组合。
前端不能用显示文本拼接替代完整身份；未知/缺失组合不能降为无规格。

| 方法与路径 | 请求 → 响应 | 行为 |
| --- | --- | --- |
| GET /accounts/{account_id}/items | 分页、status → Page<ItemSummary> | 仅当前账号 |
| POST /accounts/{account_id}/item-syncs | ItemSyncRequest → 202 AcceptedOperation | 幂等；只有完整分页成功才标记缺失商品下架 |
| GET /accounts/{account_id}/item-syncs/{sync_id} | → ItemSyncResult | 区分正常空列表、失败和不完整 |
| GET /accounts/{account_id}/rules | 分页、item_id → Page<RuleSummary> | 默认不返回正文 |
| GET /accounts/{account_id}/rules/{rule_id} | → RuleDetail | 授权编辑详情，no-store |
| POST /accounts/{account_id}/rules | RuleWriteRequest → 201 RuleDetail | 幂等；expected_version 不传 |
| PUT /accounts/{account_id}/rules/{rule_id} | RuleWriteRequest → 200 RuleDetail | expected_version 必填；禁用也走版本更新 |
| POST /accounts/{account_id}/rules/preview | RulePreviewRequest → 200 RulePreviewResponse | 纯校验，不保存，不发平台请求 |
| POST /accounts/{account_id}/rules/match-preview | MatchPreviewRequest → 200 MatchPreviewResponse | 确定性精确匹配，不发送正文 |

正文上限采用 research R7，执行前再次校验有效平台上限；保留换行，不截断、不自动拆分。
启用范围唯一约束为账号＋商品＋完整 SKU，冲突 422；修改正文不覆盖已创建任务快照。

## 4. 订单、交付、任务和异常

```text
Money {minor_units: integer, currency: string}
OrderSummary {id: string, platform: "xianyu", account_id: string, platform_order_id: string,
              item_title: string, paid_at: timestamp|null, quantity: integer|null,
              amount: Money|null, trade_type: "ordinary"|"bargain"|"unknown",
              platform_state: PlatformOrderState, delivery_state: DeliveryState|null,
              confirmation_state: ConfirmationState|null, monitoring_scope: "current"|"historical"|"uncertain",
              version: integer, updated_at: timestamp}
PlatformOrderState = unpaid | pending_ship | shipped | completed | cancelled | refunding | refunded | unknown
DeliveryState = awaiting_verification | ready | sending | delivered | definitely_not_sent | unknown | needs_review | terminated
ConfirmationState = disabled | pending | dispatching | succeeded | failed | unknown | terminated
OrderDetail {summary: OrderSummary, buyer: BuyerIdentity, item_id: string|null,
             sku: SkuCombination|null, facts_complete: boolean, fact_source: string,
             fact_observed_at: timestamp|null, delivery: DeliveryDetail|null,
             allowed_actions: AllowedAction[], timeline: TimelineEntry[]}
BuyerIdentity {platform_user_id: string|null, display_name: string|null}
DeliveryDetail {id: string, version: integer, state: DeliveryState, content: string|null,
                rule_id: string|null, rule_version: integer|null, content_hash: string|null,
                accepted_evidence: "none"|"platform"|"manual",
                automatic_retries_used: integer, confirmation_state: ConfirmationState,
                attempts: AttemptSummary[]}
AttemptSummary {id: string, kind: "initial"|"retry"|"manual_resend"|"confirmation",
                state: "prepared"|"dispatching"|"accepted"|"not_sent"|"unknown"|"cancelled",
                started_at: timestamp, finished_at: timestamp|null, reason?: string,
                evidence_source?: "platform"|"manual", platform_message_id?: string}
TimelineEntry {id: string, at: timestamp, action: string, actor: "system"|"administrator",
               reason?: string, attempt_id?: string}
AllowedAction {action: "resend"|"mark_received"|"terminate"|"confirm_shipment"|"takeover"|"restore_review",
               enabled: boolean, disabled_reason?: string}
IssueSummary {id: string, account_id: string, order_id: string|null, category: string,
              reason: string, state: "open"|"resolved"|"terminated", last_attempt_at: timestamp|null,
              allowed_actions: AllowedAction[], version: integer, created_at: timestamp}
DashboardResponse {accounts: AccountSummary[], open_issue_count: integer, active_job_count: integer,
                   persistence: "healthy"|"blocked", stopping: boolean, restore: RestoreSummary|null}
```

订单列表/日志/待处理摘要不得含完整正文；只有授权订单详情返回快照。管理员人工确认的
`accepted_evidence=manual` 不得映射成平台消息确认。原成功记录不因补发中断变成未交付。
尚未匹配规则或建立快照时相应字段为null，此时不得开放原文补发。
持久模型与DTO命名允许在transport做显式映射：pending_verification→awaiting_verification、
queued→ready、dispatching→sending、accepted→delivered、not_sent→definitely_not_sent；
review_state=required时展示needs_review，但详情/时间线须保留底层结果分类。
账号disabled映射paused，auth_expired映射authorization_expired，needs_verification映射verification_required；
pausing/resuming通过control.transition展示。禁止直接序列化数据库模型来代替这些映射。

| 方法与路径 | 请求 → 响应 | 行为 |
| --- | --- | --- |
| GET /dashboard | → DashboardResponse | 可见页面每 3 秒刷新，隐藏后停止 |
| GET /orders | account_id、platform_order_id、delivery_state、platform_state、from、to、分页 → Page<OrderSummary> | from/to 为 UTC 的付款时间范围；缺付款时间可独立筛选 |
| GET /accounts/{account_id}/orders/{order_id} | → OrderDetail | 路径账号必须匹配订单 |
| GET /issues | account_id、state、category、分页 → Page<IssueSummary> | 持久异常，重开页面不消失 |
| GET /jobs/{job_id} | → JobDetail | 查询接受后的工作，不依赖浏览器连接 |
| POST /jobs/{job_id}/cancel | CancelJobRequest → 202 AcceptedOperation | 幂等，按下述取消语义处理 |
| POST /accounts/{account_id}/reconciliations | ReconcileRequest → 202 AcceptedOperation | 补偿核验；不绕过去重、监控起点和恢复隔离 |

`CancelJobRequest {expected_version: integer, reason: string}`。
`ReconcileRequest {expected_account_version: integer}`。
取消仅停止尚未提交的未来步骤；已提交动作继续记录结果。不可取消阶段返回
409 `action_in_progress` 并附原 job_id；取消本地查询、扫码、同步不撤销平台已经完成的操作。
取消不把未知结果变成未发送，取消同步不得应用半套下架结果。暂停账号是控制屏障，不是取消单个任务。

## 5. 人工动作

```text
OrderActionRequest {expected_order_version: integer, expected_delivery_version?: integer,
                    reason: string}
ResendRequest {expected_order_version: integer, expected_delivery_version: integer,
               reason: string, acknowledge_duplicate_risk: boolean}
MarkReceivedRequest {expected_order_version: integer, expected_delivery_version: integer,
                     reason: string, evidence_note: string}
TakeoverRequest {expected_order_version: integer, reason: string,
                 acknowledge_historical_scope: boolean}
```

以下 POST 均要求幂等键、CSRF 和当前版本；所有提交仍由服务端再次核验身份、资格和互斥状态。

| 路径（前缀 /api/v1/accounts/{account_id}/orders/{order_id}） | 请求 → 响应 | 语义 |
| --- | --- | --- |
| /resends | ResendRequest → 202 AcceptedOperation | 发送原快照；未知结果须明确风险确认；已发货普通订单按 FR-022 例外处理 |
| /mark-received | MarkReceivedRequest → 200 OrderDetail | 只追加人工收到证据，不发消息、不自动确认平台发货 |
| /terminate | OrderActionRequest → 200 OrderDetail | 尚未提交才能立即终止；已提交返回冲突并保留追踪 |
| /shipment-confirmations | OrderActionRequest → 202 AcceptedOperation | 有内容接收证据才可执行；失败/未知只核验或重试确认步骤 |
| /takeovers | TakeoverRequest → 202 AcceptedOperation | 逐单接管历史普通订单，重新核验后才能创建交付 |

补发不允许客户端指定新正文、买家或绕过去重；若任务不存在返回 422，先完成历史接管或正常建任务。
原内容已存在且确定未发送、自动重试已耗尽的任务，也允许显式原内容补发，记录新的人工动作。
未知平台确认先查当前事实：已发货记确认成功，无法确定转人工，明确待发货才允许再次确认。
人工确认与正在 dispatch 的操作竞争时返回 409，不能覆盖正在形成的真实平台结果。
禁用规则、已下架商品、账号暂停或自动交付关闭均不能通过手工补发绕过。

## 6. 恢复核对

```text
RestoreSummary {id: string, state: "quarantined"|"reviewing"|"released",
                snapshot_started_at: timestamp, snapshot_finished_at: timestamp, restored_at: timestamp,
                unresolved_count: integer, coverage_complete: boolean}
RestoreReview {id: string, account_id: string, order_id: string|null,
               kind: "unfinished_snapshot"|"uncertain_interval"|"coverage_gap",
               state: "unresolved"|"received"|"approved_not_sent"|"terminated",
               reason: string, version: integer}
RestoreReviewRequest {expected_version: integer, decision: "received"|"approved_not_sent"|"terminated",
                      reason: string, evidence_note: string, acknowledge_duplicate_risk: boolean}
```

| 方法与路径 | 请求 → 响应 | 行为 |
| --- | --- | --- |
| GET /restore | → RestoreSummary 或 null | 当前恢复隔离 |
| GET /restore/reviews | account_id、state、分页 → Page<RestoreReview> | 逐单事项和追溯缺口 |
| POST /restore/reviews/{review_id}/decisions | RestoreReviewRequest → 200 RestoreReview | 幂等；只记录证据和处理授权，不直接发送 |

恢复后管理员可明确启用账号；可信付款时间晚于恢复完成、非旧任务的订单可以正常处理，
旧订单隔离继续强制阻止消息发送和平台发货确认，总开关不能清除逐单隔离。
`approved_not_sent` 必须有逐单证据；平台仍待发货不足以证明文字未发过。核对不会自动创建补发。
完整可追溯区间外的旧订单缺口保持隔离，显示需要人工处理；不能把“忽略”当成已核对。
逐单解除隔离后仍须明确接管/补发，恢复 API 不允许批量重放未知任务。
核对决定必须原子更新review、delivery、guard及人工审计：received记录人工证明并终结旧自动路径，
terminated终止旧路径，approved_not_sent保留manual_only执行门槛。
只清除review标记却让旧queued/not_sent再次自动执行不符合本契约。
DTO 的状态是面向界面的映射，底层 content_state/review_state/confirmation_state 遵循 data-model，
不能因显示“等待人工”而丢失 unknown/not_sent 分类。
备份及恢复使用停机独占 CLI，首版 HTTP 不提供备份下载或在线替换数据库接口。

## 7. 契约验证与需求覆盖

- FR-001—005、029—031：身份/能力、账号、控制屏障、进程状态；第二实例由 CLI/数据锁实现。
- FR-006—009：完整同步、精确 SKU、规则版本、正文限制与预览。
- FR-010—020：订单核验、持久任务、去重、内容/平台确认分离、补偿及未知结果。
- FR-021—024：具备证据和原因的人工动作、幂等和互斥、持久事项。
- FR-025—028：分页查询、脱敏、持久化阻塞提示、受保护备份与恢复核对。

实现时 Rust 命名 DTO 与 TS 边界运行时校验对齐；用 HTTP 行为测试验证类型、状态码、
匿名拒绝、CSRF、资源归属、幂等重放、版本冲突、202 状态演进和取消。
契约尚非平台支持证据；真实扫码、发送关联证明和完整订单读取仍执行 research R8 实验。
