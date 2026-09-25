---
description: "Task list for 007-ydisks-feature-parity"
---

# Tasks: Ydisks 功能缺口复刻·多域增量(007)

**Input**: Design documents from `/specs/007-ydisks-feature-parity/`(plan.md、spec.md、research.md、data-model.md、contracts/http-api.md、quickstart.md)

**Prerequisites**: plan.md(required)、spec.md(required)、research.md(D1~D15 决策)、data-model.md(表结构)、contracts/http-api.md(端点契约)、quickstart.md(走查步骤)

**Tests**: 包含测试任务(项目惯例:cargo test + vitest 全绿是每个规格的验收底线;领域单测先行,实现后转绿)。

**Organization**: 按七个用户故事分组,每个故事是可独立实现/测试/交付的增量。迁移文件 0004(卡密+模板+规则,US1 创建,含 US2/US3 所需表)、0005(聊天,US4)、0006(通知+系统AI,US5)。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行(不同文件,无未完成依赖)
- **[Story]**: 所属用户故事(US1~US7,对应 spec.md P1~P7)
- 描述含精确文件路径与关键约束(引号内容为 data-model/contracts 的硬约束,不得实现时更改)

## Path Conventions

- 后端:`src/domain/`、`src/application/`、`src/adapters/`、`src/transport/`、`src/runtime/`、`migrations/`
- 前端:`frontend/src/`(features/ shared/ ui/ styles/ app/)
- 规格:`specs/007-ydisks-feature-parity/`

---

## Phase 1: Setup(共享基础设施)

- [X] T001 按 research.md D11 完成依赖过审并写入 Cargo.toml:新增 `lettre 0.11`(仅 `smtp-transport`+`builder` feature,native-tls)、`calamine`(xlsx 批量导入)、`csv 1.x`;为 axum 启用 `multipart` feature;在 Cargo.toml 注释记录"zip 双版本并存"的体积权衡(D11)

---

## Phase 2: Foundational(阻塞所有用户故事)

**⚠️ CRITICAL**: 以下完成前不得开始任何用户故事

- [X] T002 重构迁移表数量断言:src/adapters/sqlite/migrations.rs 测试中硬编码的"表数量=25"改为模块级常量 `EXPECTED_TABLE_COUNT`(附注释:每新增迁移随 T005/T042/T052 更新,最终 44),并补一条 schema_migrations 版本连续性断言
- [X] T003 [P] 在 src/transport/error.rs 新增 5 个错误码(research D14):`stock_insufficient`(422)、`referenced_resource`(409)、`payload_too_large`(413)、`reply_limit_reached`(422)、`credential_change_failed`(422)——snake 名/状态映射/retryable 三处 match 同步,补对应单测
- [X] T004 [P] 在 frontend/src/shared/contracts.ts 收录上述新错误码的类型与面向用户的提示文案映射(供各 feature 复用),保持"未知码安全降级"模式

**Checkpoint**: 基础就绪——US1 可开始;US2/US3 的表结构由 T005 一并建立

---

## Phase 3: User Story 1 - 卡密库存域(P1)🎯 MVP

**Goal**: 四类卡密组 CRUD、追加/批量导入、原子预留绑定订单交付、概览库存卡——交付内容链的根基。

**Independent Test**: 建批量组导入 3 张卡 → 配规则 → 触发一笔已付款订单 → 预留→发送→扣减全链路,余量 3→2;删除被引用组被拒。

### Tests for User Story 1

- [X] T005 [P] [US1] 先写领域单测(预期失败):src/domain/cards.rs 的条目状态机(available→reserved→used;释放仅 reserved→available)与校验边界(`delay_seconds` 0–3600、kind 四枚举、api_config 必填字段)
- [X] T006 [P] [US1] 先写集成测试(预期失败):tests/ 内新增卡密集成测试——原子预留不双配(并发两订单)、余量不足转 issue `stock_insufficient` 且不部分交付、`Accepted`→used / `NotSubmitted`→释放 / `Unknown`→保持 reserved、被引用删除 409 referenced_resource

### Implementation for User Story 1

- [X] T007 [US1] 创建 migrations/0004_cards_templates_rules.sql(data-model.md 增量 1 全量):card_pools、card_entries、delivery_templates、delivery_template_messages、rule_variants、reply_rules、reply_rule_items、default_replies、default_reply_log、review_reminder_state 十表 + rules 扩展列(`trigger_type` DEFAULT 'order_paid'/`priority` DEFAULT 100/`all_items_confirmed`/`needs_reconfiguration`/`card_pool_id`/`template_id`/`template_bindings`/`review_config`)+ 索引重建(`DROP INDEX idx_rules_enabled_scope; CREATE UNIQUE INDEX idx_rules_enabled_scope2 ON rules(account_id,item_id,sku_key,trigger_type,priority) WHERE enabled=1`);在 migrations.rs `MIGRATIONS` 常量追加 `(4, include_str!(...))`,更新 `EXPECTED_TABLE_COUNT`
- [X] T008 [US1] 实现 src/domain/cards.rs(T005 转绿)
- [X] T009 [US1] 实现 src/adapters/sqlite/repos/cards.rs:CRUD、append(空行忽略)、分状态计数、条目分页、原子预留(`UPDATE ... WHERE id=(SELECT ... WHERE pool_id=? AND state='available' ORDER BY created_at,id LIMIT 1) RETURNING`,状态谓词防双配)、释放/扣减(同事务)
- [X] T010 [US1] 实现 src/application/cards/mod.rs:组用例(加密信封 AAD purpose `card_pool_content`/`card_entry`,列表/详情不回显明文)、批量导入(calamine xlsx + csv/tsv,模板校验"名称,类型,内容,描述,启用,延迟秒",逐行成功/失败报告,仅成功行入库)、test-api(不落库)
- [X] T011 [US1] 实现 CardSupplier 端口(src/application/ports/)与适配器:API 取卡(reqwest、超时/重试按配置、`response_path` 点号取值子集),每次取卡 INSERT card_entries(origin='api',request_key + 响应信封留痕,data-model 审计列)
- [X] T012 [US1] 扩展 src/application/delivery/service.rs `freeze_snapshot`(research D2/D3):ContentPlan 解析(fixed_text 透传;card_pool 预留 N=份数×件数、拼接、快照 source_content_id=`card:{pool_id}:{entry_ids}`);classify 挂钩:Accepted→扣减、NotSubmitted→释放、Unknown→保持+既有 issue、订单终态→释放
- [X] T013 [US1] 实现 src/transport/cards_api.rs(contracts §1 全部 9 端点,multipart 接收 ≤ 限值,413 payload_too_large)+ routes.rs 注册
- [X] T014 [P] [US1] 概览第 5 卡后端:src/adapters/sqlite/repos/stats.rs 新只读查询(启用批量组可用余量之和)+ src/application/stats.rs `StatsOverview.stock: Option<StockSummary>` + transport/stats_api.rs 输出可选字段(缺失降级,不闪 0)
- [X] T015 [P] [US1] frontend/src/shared/contracts.ts:CardPoolDto/StockSummary interface + parse 守卫(只校验消费字段)
- [X] T016 [US1] frontend/src/features/cards/(page.ts/model.ts/编辑抽屉/批量导入弹窗/API 构建器含测试按钮)+ app/router.ts 注册 `/cards` + styles/components.css 尾部 `/* ===== 卡密库存(007)===== */` 分节(优先复用既有类,零新增 CSS 为目标)
- [X] T017 [P] [US1] frontend/src/features/cards/model.test.ts(解析/过滤/余量派生,含字段缺失降级分支)
- [X] T018 [US1] frontend/src/features/overview/ 第 5 张统计卡:statArea 追加"库存卡密余量"、原地更新 span、deriveCards 补派生、下钻 href 指向 /cards
- [X] T019 [US1] 按 quickstart.md"US1 卡密库存"1–8 步逐项走查并记录证据(mock 档;UI 实证:导航/空态/建组 3 条无明文回显/编辑抽屉字段/概览卡 0→3 联动;步骤 3 由契约测试、4–7 由集成测试覆盖 reserve/consume/release/不双配/409 引用保护,规则编辑器绑卡组 UI 待 US3 T033 后复验)

**Checkpoint**: US1 独立可验收(MVP)——建组→导入→规则绑定→预留→发送→扣减全链路 + 概览卡;`cargo test`/`npm run test` 全绿

---

## Phase 4: User Story 2 - 发货模板域(P2)

**Goal**: 多消息模板 + 变量占位符 + 引用保护;规则可选模板并绑定卡密变量。

**Independent Test**: 建含 `{{buyer_nickname}}` 与卡密变量的两消息模板 → 规则绑定后触发订单 → 发送内容与预览逐字一致;被引用删除被拒。

### Tests for User Story 2

- [ ] T020 [P] [US2] 先写单测(预期失败):src/domain/templates.rs 占位符解析/校验(`{{cards.key}}`/`{{custom.key}}`/系统变量;命名限 `[A-Za-z0-9_-]`;未知变量报错定位)
- [ ] T021 [P] [US2] 先写集成测试(预期失败):模板多消息顺序发送、每条消息独立 proof、中断后续发不重发已确认条、被引用删除 409

### Implementation for User Story 2

- [ ] T022 [US2] 实现 src/domain/templates.rs(T020 转绿)
- [ ] T023 [US2] 实现 src/adapters/sqlite/repos/templates.rs 与 src/application/templates/mod.rs:CRUD(消息 ≤10 条、单条 ≤1000 unicode 标量)、used_by 引用计数(rules+rule_variants+template_bindings 扫描)、渲染函数(订单事实 + 绑定卡密内容 + custom 取值;预览与发送共用)
- [ ] T024 [US2] 扩展 delivery(research D4):freeze_snapshot template 分支(快照存渲染后消息数组,source_content_id=`template:{id}`);dispatch 逐条顺序发送、每条 Accepted 各 insert 一行 delivery_proofs(同 attempt 多行)、按 content_digest 判定续发起点
- [ ] T025 [US2] 实现 src/transport/templates_api.rs(contracts §2)+ 扩展 rules/preview 返回 `rendered_messages[]`(卡密内容以 `[卡密内容 ×N]` 掩码)
- [ ] T026 [P] [US2] frontend/src/features/templates/(page.ts/model.ts/编辑器含变量指南与示例块)+ router `/templates` + components.css 分节
- [ ] T027 [P] [US2] frontend/src/features/templates/model.test.ts(占位符校验/keys 提取)
- [ ] T028 [US2] 按 quickstart.md"US2 发货模板"1–6 步走查

**Checkpoint**: US1+US2 独立可验收;模板渲染预览=实际发送

---

## Phase 5: User Story 3 - 自动化规则扩展(P3)

**Goal**: 优先级/变体/账号级确认/关键词回复/默认回复/求评计划;自动化异常入待处理。

**Independent Test**: 同商品两变体规则各命中正确卡组;同范围两条规则仅优先级最高者执行;关键词回复不触交付。

### Tests for User Story 3

- [ ] T029 [P] [US3] 先写单测(预期失败):src/domain/rules_ext.rs 变体匹配(变体 (spec_name→value) 对 ⊆ 订单 sku_pairs)、优先级选取、同优先级冲突判定
- [ ] T030 [P] [US3] 先写集成测试(预期失败):同范围同触发两条启用规则仅执行 priority 最小一条;账号级未确认(all_items_confirmed=0)不执行且标"需确认·暂不发货";关键词回复发送消息但订单/交付状态零变化

### Implementation for User Story 3

- [ ] T031 [US3] 实现 src/domain/rules_ext.rs(T029 转绿)
- [ ] T032 [US3] 实现 src/adapters/sqlite/repos/rules_ext.rs:变体/关键词(reply_rules+reply_rule_items)/默认回复(default_replies+default_reply_log)/求评状态(review_reminder_state)仓储;rules 查询扩展(trigger_type/enabled/search 过滤、priority 升序)
- [ ] T033 [US3] 改造 src/application/catalog/rules.rs:RuleDraft 扩展(trigger_type/variants/来源绑定/review_config/priority)、同范围同触发同优先级冲突→422 rule_conflict、账号级必须 all_items_confirmed、引用缺失→needs_reconfiguration 置位并旁路执行、修复后清除
- [ ] T034 [US3] 扩展 src/application/delivery/eligibility.rs + service.rs 规则命中:同范围同触发 priority 升序取第一条(仅最高执行)、变体命中、item 规则优先于账号级(item_id='')回退
- [ ] T035 [US3] 端口扩展(research D6 文本部分):src/application/ports/platform.rs 新增 `send_chat_message(ctx, ChatPeer{buyer_id, chat_id: Option}, kind: ChatSendKind, content)`(default 实现=UnsupportedCapability);src/adapters/xianyu/adapter.rs 复用 sendByReceiverScope 实现(chat_id 优先/buyer_id 兜底);mock 适配器实现
- [ ] T036 [US3] 实现 src/application/replies/mod.rs 分流编排(research D7):买家文本消息 → 关键词(商品级优先→账号级,包含匹配忽略大小写)→ AI 占位 hook(US7 实装)→ 默认回复(reply_once 查 default_reply_log);任一环节失败顺延;全部经 send_chat_message,**不写订单/交付状态**;发送结果留痕
- [ ] T037 [US3] 求评计划:src/runtime/supervisor.rs 每小时定时器 + src/application/replies/ 求评扫描(review_config:wait_hours/interval_hours/max_count/text;账号停用/离线跳过留痕;达上限停止)
- [ ] T038 [US3] issues 扩展:自动化失败新 kind 与 allowed_actions 语义映射(continue→确认继续/retry→安全重试/cancel→终止,复用既有 manual 动作);src/application/delivery/service.rs 失败路径携带
- [ ] T039 [US3] transport:rules API 扩展(contracts §3:RuleDto 新字段/trigger_counts/保存约束)+ reply-rules 与 default-reply 端点
- [ ] T040 [P] [US3] frontend:features/catalog/rule-editor.ts 表单主体提取为 `renderRuleForm`(match-preview 零改动)+ 新建 features/rules/(三页签:交易自动化/关键词回复/默认回复)+ 新建 features/items/(自 catalog 拆出商品列表,保留"关联发货规则"跨页跳转 `?account=&item=`)+ app/router.ts:`/catalog` 移除,`/items` `/rules` 注册
- [ ] T041 [P] [US3] frontend/src/features/rules/model.test.ts + match-preview 回归测试
- [ ] T042 [US3] 按 quickstart.md"US3 自动化规则扩展"1–8 步走查

**Checkpoint**: US1~US3 独立可验收;内容链(卡密/模板/规则)完整

---

## Phase 6: User Story 4 - 在线聊天域(P4)

**Goal**: 会话/消息/发送/快捷回复/买家备注/未读;事件接入最小侵入。

**Independent Test**: 在线账号发文本状态流转"发送中→已发送";未知结果标黄不自动重发;删除会话本机隐藏。

### Tests for User Story 4

- [ ] T043 [P] [US4] 先写单测(预期失败):src/domain/chat.rs 出站状态机(sending→sent|failed|uncertain;uncertain 不可自动重试)与未读聚合
- [ ] T044 [P] [US4] 先写集成测试(预期失败):同一条 WS 消息经事件管道与聊天摄取不重复落库;未知结果不开重试;快捷回复第 51 条 422 reply_limit_reached

### Implementation for User Story 4

- [ ] T045 [US4] 适配器事件(research D5):src/adapters/xianyu/events.rs 新增 `MessageKind::ChatMessage{chat_id,buyer_id,message_id,kind,text,image_url,item_id}`(普通买家消息不再丢弃)+ `PlatformEvent::ChatMessageReceived`;同步 events.rs `meta_of/fields_of` 与 supervisor.rs `dispatch_loop` 穷尽 match → ChatService::ingest
- [ ] T046 [P] [US4] migrations/0005_chat.sql(data-model 增量 2:conversations/chat_messages/quick_replies/buyer_notes,含唯一与部分索引)+ migrations.rs 追加 + `EXPECTED_TABLE_COUNT` 更新
- [ ] T047 [P] [US4] capabilities 扩展:src/application/ports/platform.rs CapabilitySet 增 `chat_send_image`/`chat_history_backfill`(live 适配器如实声明,mock 实现)+ transport capabilities DTO + frontend parse
- [ ] T048 [US4] 实现 src/domain/chat.rs(T043 转绿)
- [ ] T049 [US4] 实现 src/adapters/sqlite/repos/chat.rs(会话 upsert/消息分页/未读/隐藏清理/快捷回复/买家备注)
- [ ] T050 [US4] 实现 src/application/chat/mod.rs:ingest(去重、未读++、触发 replies 分流)、发送(同步等待 ≤15s,SendOutcome→四状态)、retry(仅 failed)、read 清零、删除会话(本机隐藏+清展示消息,确认流)、`OutgoingMessageEvidence` 回填 platform_message_id
- [ ] T051 [US4] 图片发送(能力门禁):ChatSendKind::Image 适配器实现、上传文件存数据目录 uploads/(≤10MB,413 payload_too_large)、`chat_send_image=false` 时端点 403 unsupported_capability
- [ ] T052 [US4] 实现 src/transport/chat_api.rs(contracts §4 全部端点)
- [ ] T053 [US4] frontend/src/features/chat/(三栏布局/账号 Tab+在线点/会话搜索+只看未读/消息面板+加载更早/输入区回车发送/粘贴图片预览/快捷回复抽屉/买家备注弹窗/四状态渲染)+ router `/chat` + shell 徽标(app/store.ts 增 chatUnread 通道,ShellNav.badge 接线)
- [ ] T054 [P] [US4] frontend/src/features/chat/model.test.ts
- [ ] T055 [US4] 按 quickstart.md"US4 在线聊天"1–7 步走查,**并记录 3s 轮询空闲开销测量**(宪章 III:可见时 ≤3s、页面不可见暂停)

**Checkpoint**: US1~US4 独立可验收;聊天与交付共用发送可靠性语义

---

## Phase 7: User Story 5 - 通知渠道域(P5)

**Goal**: 七类渠道、事件订阅、账号绑定、测试投递、系统 SMTP;unknown 转待处理。

**Independent Test**: Webhook 渠道测试成功触达;仅订阅"账号掉线"时其他事件不发送;unknown 投递留痕转待处理。

### Tests for User Story 5

- [ ] T056 [P] [US5] 先写单测(预期失败):dingtalk 加签与 feishu 签名算法(HMAC-SHA256,chrono 时间戳)、event_types 订阅过滤(空=全部)
- [ ] T057 [P] [US5] 先写集成测试(预期失败):绑定覆盖(绑定账号仅发绑定渠道)、unknown 投递→issue `notify_unknown`(无订单去重索引生效)、secrets 脱敏回显(编辑留空不覆盖)

### Implementation for User Story 5

- [ ] T058 [US5] migrations/0006_notify_ai.sql(data-model 增量 3:notification_channels/bindings/deliveries、system_settings/system_secrets、accounts 增 `ai_reply_enabled`/`ai_prompt`、`idx_issues_open_dedup_account` 无订单去重索引)+ migrations.rs 追加 + `EXPECTED_TABLE_COUNT` 更新
- [ ] T059 [US5] 实现 src/domain/notify.rs(T056 转绿)
- [ ] T060 [US5] 实现 src/adapters/notify/:webhook/bark/telegram/dingtalk(加签)/feishu(签名)/wecom(reqwest,10s 超时)+ email(lettre,独立 SMTP 或系统 SMTP,STARTTLS/SSL 互斥);secrets 整包信封加密(AAD purpose `notify_secrets`)
- [ ] T061 [US5] 实现 src/adapters/sqlite/repos/notify.rs 与 src/application/notify/mod.rs:NotifyEvent 枚举、订阅过滤、绑定覆盖(未绑定走全部启用渠道)、tokio::spawn 异步扇出(不阻塞事件管道)、notification_deliveries 留痕、unknown→issue
- [ ] T062 [US5] 挂钩(research D8):supervisor.rs dispatch_loop(AccountRuntimeChanged→掉线/恢复、AuthorizationChanged→security_verification、ProtocolIssue→system_error)+ issues::open 应用层包装(manual_intervention_required)+ delivery classify 终态 not_sent/unknown→delivery_result
- [ ] T063 [US5] 实现 src/transport/notify_api.rs(contracts §5 全部端点,secrets_configured 脱敏模式)
- [ ] T064 [P] [US5] frontend/src/features/notifications/(渠道列表/编辑弹窗含各类型"如何获取配置"指引/测试按钮/账号绑定/系统 SMTP 卡)+ router `/notifications`
- [ ] T065 [P] [US5] frontend/src/features/notifications/model.test.ts
- [ ] T066 [US5] 按 quickstart.md"US5 通知渠道"1–6 步走查

**Checkpoint**: US1~US5 独立可验收

---

## Phase 8: User Story 6 - 订单同步与人工交付动作(P6)

**Goal**: 一键同步任务、单笔同步、人工触发交付、人工确认平台发货。

**Independent Test**: 一键同步产出逐账号报告可取消;人工触发交付幂等;缺核验要素时拒绝并指明缺什么。

### Tests for User Story 6

- [ ] T067 [P] [US6] 先写集成测试(预期失败):同步任务逐账号报告与取消(cancel_requested 兑现);trigger_delivery 重复触发幂等拒绝;付款事实缺失→422 并列缺失字段;confirm_shipment 在交付未 accepted 时 422

### Implementation for User Story 6

- [ ] T068 [US6] 实现 src/application/orders_sync.rs(research D9):JobService::create("order_sync") + spawn 逐账号复用 TraceScanService 模式、`window_days` 参数化(默认 7,trace.rs 硬编码 24h 改为带默认值参数向后兼容)、逐账号检查 cancel_requested、报告字段 `{orders_seen, created, restored, reassigned, ineligible, failed[], coverage, offline?}`
- [ ] T069 [US6] 扩展 src/application/manual/actions.rs(research D10):`trigger_delivery`(reason 1-500 字→begin_idem("trigger_delivery")→manual_actions+guard 同事务→资格核验(缺→422 附缺失要素)→delivery.handle_payment)+ `confirm_shipment`(前提 content_state='accepted';调 adapter.confirm_shipment,平台失败/不支持时记录人工断言 origin='manual';attempts action_kind='manual_confirm')
- [ ] T070 [US6] transport:POST /orders/syncs、POST /accounts/{aid}/orders/{oid}/syncs、POST .../deliveries、POST .../confirm-shipments + manual_err 映射新错误分支 + issues allowed_actions 增 "trigger_delivery"
- [ ] T071 [P] [US6] frontend/src/features/orders/:工具栏"一键同步"(job 轮询反馈行,复用 wireSyncButton 模式)+ 单笔同步按钮 + ManualAction/ACTION_META/manualActionBody/actionAvailability 增两动作 + features/issues/page.ts ACTION_LABELS 同步
- [ ] T072 [P] [US6] frontend/src/features/orders/model.test.ts 扩展(可用性/请求体分支)
- [ ] T073 [US6] 按 quickstart.md"US6 订单同步与人工交付动作"1–5 步走查

**Checkpoint**: US1~US6 独立可验收

---

## Phase 9: User Story 7 - 系统与 AI 设置(P7)

**Goal**: AI 配置/账号 AI 自动回复实装、管理员凭据修改;设置页可写。

**Independent Test**: 测试连接返回模型/耗时/摘要;改密后其他会话失效;AI 不可用静默降级。

### Tests for User Story 7

- [ ] T074 [P] [US7] 先写集成测试(预期失败):改密成功后其他会话 401、当前密码错误 422 credential_change_failed;AI 不可用时买家消息静默降级且关键词/默认回复不受影响

### Implementation for User Story 7

- [ ] T075 [US7] 实现 src/adapters/aiclient/:OpenAI 兼容 `/chat/completions`(30s 超时)、`/models` 列表、最小测试对话;错误结构化
- [ ] T076 [US7] 实现 src/application/settings_sys.rs(system_settings 明文键/system_secrets 信封键 ai_api_key+smtp_password,读取返回 `*_configured` 布尔)+ src/application/auth.rs 扩展 `change_credentials`(argon2 验证当前密码→更新→**除当前会话外全部 revoke**)
- [ ] T077 [US7] 实装 src/application/replies/ AI 分支(T036 占位):AiReplyProvider 注入、账号 ai_reply_enabled+ai_prompt(系统提示词+输出截断 2000 字)、失败静默降级 tracing 留痕
- [ ] T078 [US7] transport:GET/PUT /settings/system、POST /settings/ai/models、POST /settings/ai/test、PUT /auth/credentials、PUT /accounts/{id}/ai-settings(contracts §7)
- [ ] T079 [P] [US7] frontend/src/features/settings/page.ts 扩展:AI 配置卡(表单三件套/读取模型/测试连接/secrets 占位)+ 账号 AI 开关接入账号编辑弹窗 + 凭据修改卡(openConfirmFlow,提示"其他会话将退出");保留既有只读信息卡不回退
- [ ] T080 [P] [US7] frontend/src/features/settings/model.test.ts
- [ ] T081 [US7] 按 quickstart.md"US7 系统与AI"1–5 步走查

**Checkpoint**: 七个故事全部独立可验收

---

## Phase 10: Polish & Cross-Cutting

- [ ] T082 [P] 回写 specs/001-xianyu-auto-delivery/spec.md:FR-008 处追加"007 修订注记"(唯一启用→优先级模型,指向 007 Clarifications),不改历史原文
- [ ] T083 [P] 更新 README.md/docs 功能清单:新导航 11 项与各页面一句话说明
- [ ] T084 FR-001 导航总检:侧边栏 11 项顺序对齐参考页(概览/账号管理/在线聊天/卡密库存/商品列表/订单管理/发货模板/自动化规则/待处理/通知设置/系统与AI),/chat 徽标、/catalog 入口已移除、全部一步可达
- [ ] T085 全量回归:quickstart.md 全部七节走查 + SC-001~SC-008 逐项核对表 + 迁移断言 44 复核
- [ ] T086 终验门禁:cargo test、cargo build --features dev-fixtures、npm run typecheck、npm run test 全绿;虚标能力扫描(界面上每个入口/徽标对应真实能力)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup(T001)**→ **Foundational(T002~T004)** → 阻塞全部用户故事
- **US1(T005~T019)**:首个故事;T007 创建迁移 0004(含 US2/US3 所需全部表)
- **US2(T020~T028)**:依赖 US1(T007 表、T012 ContentPlan 框架、卡密渲染依赖卡组)
- **US3(T029~T042)**:依赖 US1(表/ContentPlan)+ US2(模板来源);T035 的 send_chat_message 端口被 US4 复用
- **US4(T043~T055)**:依赖 T035(发送端口)与 T036(ingest 触发分流可空跑);其余独立
- **US5(T056~T066)**:独立(仅依赖 Foundational);T058 创建迁移 0006(含 US7 的 settings/ai 列)
- **US6(T067~T073)**:独立;T070 依赖 T003 错误码
- **US7(T074~T081)**:依赖 T058(0006 表)与 T036(AI hook 占位)
- **Polish**:依赖全部故事

### 并行机会

- Foundational 内 T003/T004 并行
- 每故事内测试任务([P])与后端模型任务先行;前端任务与后端任务可双线并行(契约以 contracts/http-api.md 为准)
- 跨故事:US4/US5/US6 在 US1~US3 完成后可三线并行(不同文件域);US5 与 US7 串行(共享 0006)
- 单人顺序执行建议:严格按 Phase 3→9 顺序(MVP 优先)

### User Story 独立验收标准

| 故事 | 独立测试(quickstart 节) |
|---|---|
| US1 | 卡密全链路:3→2 余量、不双配、引用保护、概览卡 |
| US2 | 预览=发送、逐条 proof、引用删除拦截 |
| US3 | 变体命中/优先级唯一执行/关键词不触交付 |
| US4 | 四状态可见、unknown 不自动重发、未读徽标 |
| US5 | 测试触达、订阅过滤、unknown 转待处理 |
| US6 | 同步报告可取消、人工交付幂等、缺核验拒绝 |
| US7 | 测试连接、改密会话失效、AI 降级不阻塞 |

---

## Implementation Strategy

- **MVP First**:T001→T004→US1(T019 走查通过)即为可演示 MVP(卡密库存交付链+概览卡),此时可停下验证再继续。
- **增量交付**:每故事结束跑该故事 quickstart 节 + cargo test/npm test;任何故事单独回滚不影响已完成故事(迁移只追加)。
- **TDD 节奏**:每故事先落测试任务(标注"预期失败"),实现后转绿;禁止先实现后补测。
- **宪章红线**(实现中随时自检):网络不进 DB 事务;unknown 不自动重发/不回库;关键词/默认/AI 回复零订单状态写入;界面上无未验证能力的可用入口。

## Notes

- [P] 任务=不同文件、无未完成依赖;并行前确认契约(contracts/http-api.md)已定
- 每任务或逻辑组完成后 commit;每个 Checkpoint 处停下独立验证
- 迁移文件一经提交不得修改(只追加);`EXPECTED_TABLE_COUNT` 随 T007/T046/T058 递增至 44
- 实现遇到与 data-model.md/contracts 冲突的事实:回到规格澄清,不得静默偏离
