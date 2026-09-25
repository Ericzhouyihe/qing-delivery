# Research: 007-ydisks-feature-parity

Phase 0 产出。所有决策基于两轮源码研究:Ydisks 参考实现(`Ydisks-Xianyu-Helper/`)与本项目现状(`src/`、`frontend/src/`、`migrations/`)。行号引用为研究时快照。

## D1 规则模型:多条并存 + 优先级选取(001 FR-008 修订)

**Decision**: `rules` 表增列 `trigger_type TEXT NOT NULL DEFAULT 'order_paid'`、`priority INTEGER NOT NULL DEFAULT 100`、`all_items_confirmed INTEGER NOT NULL DEFAULT 0`、`needs_reconfiguration INTEGER NOT NULL DEFAULT 0`、`item_id` 允许哨兵值 `''` 表示账号级。重建唯一索引:`DROP INDEX idx_rules_enabled_scope; CREATE UNIQUE INDEX idx_rules_enabled_scope2 ON rules(account_id, item_id, sku_key, trigger_type, priority) WHERE enabled = 1;`。应用层 `count_enabled_exact`/`find_enabled_exact`(repos/rules.rs L83/L113)改为:同范围同触发按 priority 升序取第一条;`EnabledConflict`(422 rule_conflict)语义收窄为"同范围同触发同优先级"。账号级规则仅在 `all_items_confirmed=1` 时参与执行。

**Rationale**: 复刻 Ydisks 行为(`internal/` 规则卡含优先级,同账号+商品+触发仅执行最高一条);多规格变体天然要求同范围多条。防重复交付由"仅执行最高一条 + 既有 `order_execution_guards` 互斥 + T1 `idx_deliveries_initial` 唯一"三层保障,不弱于原唯一性约束。规格 Clarifications 已记录此决策与用户授权。

**Alternatives considered**: ①保持唯一启用、变体收敛进单规则——拒绝:同商品多套内容受限,偏离复刻目标;②首版唯一、优先级后置——拒绝:后续再迁移索引与数据成本更高。

**宪章对齐**: 覆盖全部商品(账号级)规则显式确认后才执行 = 宪章"必须显式配置"。

## D2 内容来源扩展切入点:`freeze_snapshot` 唯一转换点

**Decision**: 在 `application/delivery/service.rs::freeze_snapshot`(L362-432)前置引入 `ContentPlan` 解析:规则命中后按内容来源解析为待冻结文本——`fixed_text`(现状,解密 rule_content)/ `card_pool`(预留并取卡拼接)/ `template`(渲染模板消息列表)。输出不变:仍写 `content_snapshots` 行(`source_content_id` 扩展为 `card:{pool_id}:{entry_ids}` / `template:{template_id}`),下游 T3 dispatch 无需感知来源。模板多条消息 = 快照存 JSON 消息数组,`ContentForSend.text` 为逐条发送的当前条。

**Rationale**: `freeze_snapshot` 已是"规则内容→订单级冻结快照"唯一转换点且幂等(已有 snapshot 直接返回);卡密在冻结阶段取用保证"发送前内容已持久化"(宪章 I)。改动面最小,T1/T3/classify 全部复用。

**Alternatives considered**: dispatch 阶段现取卡现发——拒绝:违反"发送前持久化内容",崩溃恢复时内容丢失。

**宪章对齐**: 对外发送前内容已 seal 进 content_snapshots;`source_content_id` 保留来源追溯。

## D3 卡密预留协议:事务内原子预留,API 取卡两阶段

**Decision**: 存量型(批量/文本/图片):`freeze_snapshot` 内在 `DbThread` 单事务执行 `UPDATE card_entries SET state='reserved', reserved_order_id=?, reserved_delivery_id=? WHERE id=(SELECT id FROM card_entries WHERE pool_id=? AND state='available' ORDER BY created_at, id LIMIT 1) RETURNING ...`,循环 N=每件份数×购买件数;任一次取不到 → 整体回滚 + issue `stock_insufficient`(不部分交付)。API 型:事务外先调 `CardSupplier` 端口(reqwest,超时/重试按配置),成功后在事务内 INSERT `card_entries(state='reserved', origin='api', request_key=..., raw_response_envelope=...)` 绑定订单再快照;每次取卡调用都落一行(审计)。结果处置:classify 后 `Accepted` → 同事务置 `used`;`NotSubmitted`(确定未发送,含重试预算耗尽后的 terminal not_sent)→ 释放回 `available`;`Unknown` → 保持 reserved + issue `delivery_unknown`(既有);订单终态(canceled/refunded/terminated)→ 释放。已 used 条目永不复用。

**Rationale**: 宪章 I 逐条对应:原子预留绑订单、确定成功才扣减、只有确定未发送可释放、结果未知不回库不换卡、外部取卡保存请求标识与返回内容。`DbThread` 串行化天然避免超卖,RETURNING+状态谓词保证并发下单卡单订单。

**Alternatives considered**: 预留在传输层做——拒绝:违反分层;API 取卡结果进事务内请求——拒绝:网络不得占用 DB 事务。

## D4 模板多消息发送:一次 attempt 顺序发送,逐条留证

**Decision**: T3 dispatch 对含 M 条渲染消息的快照循环发送:每条独立 `adapter.send_text`,前一条 `Accepted` 才发下一条;每条 Accepted 各 insert 一行 `delivery_proofs`(同 attempt_id 多行,表无唯一约束阻挡);中途出现 `Rejected/NotSubmitted` → finish_attempt 按该分类落库,已发条目保留 proofs,重试从首条未确认消息继续(以 proofs 中 content_digest 判定);`Unknown` → 按既有 unknown_manual 处置。渲染以订单事实为源(买家昵称/订单号/买家ID + 绑定卡密内容 + 自定义变量),预览端点与实际发送共用同一渲染函数。

**Rationale**: 宪章"消息交付与平台确认独立";逐条留证保证崩溃后可判定"哪些条已确认送达",重试不重发已确认条目。

**Alternatives considered**: 每条消息独立 attempt——拒绝:attempt 序列语义与重试预算(3 次)耦合混乱;整单合并一条消息——拒绝:偏离 Ydisks 逐条消息体验。

## D5 聊天事件接入:最小侵入新增 ChatMessage 事件

**Decision**: `adapters/xianyu/events.rs::extract`(L97-105)对买家普通消息不再直接丢弃:新增 `MessageKind::ChatMessage{chat_id, buyer_id, message_id, kind, text, image_url, item_id}`(数据 extract 已解析,L80-95);`PlatformEvent` 新增变体 `ChatMessageReceived{...}`;同步更新穷尽 match:`events.rs::meta_of/fields_of`(L180-190)、`supervisor.rs::dispatch_loop`(L182-241)新增分支 → `ChatService::ingest`。`OutgoingMessageEvidence`(已定义、当前仅 Observed)启用:聊天发送的回执经它回填 `chat_messages.platform_message_id`。

**Rationale**: extract 已有全部字段,管道去重(source_event_id 唯一 + payload digest)免费复用;不新增连接或轮询。

**Alternatives considered**: 聊天单独开 WS/轮询适配器——拒绝:重复连接维护,违背轻量;在前端拉取平台接口——拒绝:协议细节泄漏出适配器。

## D6 聊天发送端口:新增 `send_chat_message`,能力门禁

**Decision**: `ports/platform.rs` 新增(带 default 实现返回 `PlatformError::UnsupportedCapability`,mock 适配器覆盖实现):

```rust
fn send_chat_message(&self, ctx, peer: ChatPeer { buyer_id, chat_id: Option<String> },
    kind: ChatSendKind /* Text | Image{path} */, content: &ContentForSend)
    -> impl Future<Output = Result<SendOutcome, PlatformError>> + Send;
```

复用闲鱼 `sendByReceiverScope` 通道(与 send_text 同一 WS 会话),chat_id 优先、buyer_id 兜底;`SendOutcome` 四分类原样复用 → 聊天页四状态(sending/sent/failed/uncertain)。`CapabilitySet`(既有 capabilities 声明)扩展 `chat_send_image`、`chat_history_backfill` 两个布尔,前端按此门禁图片入口与历史说明(规格 FR-042)。自动回复(关键词/默认/AI)走同一端口,**绝不写订单/交付状态**。

**Rationale**: 交付与聊天共享同一发送可靠性语义;default 实现使新增方法不破坏既有 trait 实现与 mock。

**Alternatives considered**: 复用 `send_text(order_id,...)` 传伪造订单——拒绝:污染语义与 chats 绑定 map。

## D7 自动回复分流:关键词 → AI → 默认回复

**Decision**: `ChatService::ingest` 后对买家文本消息执行 `replies::resolve`:①关键词回复(`reply_rules`,商品级优先于账号级,包含匹配,忽略大小写);②未命中且账号 `ai_reply_enabled` → `AiReplyProvider`(OpenAI 兼容 `/chat/completions`,超时 30s,系统提示词=账号自定义+长度约束,输出截断至 2000 字);③仍未产生回复且 `default_replies.enabled` → 默认文案(reply_once 经 `default_reply_log` 判定)。任一环节发送失败顺延下一环节,全部失败仅留痕。AI/通知凭据存 `system_secrets`(信封加密);AI 不可用静默降级 + tracing 留痕,不阻塞交易事件处理。

**Rationale**: 源码证据(Ydisks `internal/engine/reply.go::resolve()`:API→关键词→AI→默认;其 API 回复环节属外部集成不在本项目范围)。宪章:AI 推断不参与付款/发货判定——分流输出仅是聊天消息。

**Alternatives considered**: AI 优先于关键词——拒绝:与参考实现相反,关键词是卖家明确意图,应最高优先。

## D8 通知事件源与投递语义

**Decision**: `application/notify` 定义内部事件枚举 `NotifyEvent { account_offline, account_recovered, security_verification, manual_intervention_required, delivery_result{order, outcome}, system_error }`。挂钩点:`supervisor::dispatch_loop` 的 `AccountRuntimeChanged`/`AuthorizationChanged`/`ProtocolIssue` 分支;`issues::open` 的应用层包装(按 kind 映射 manual_intervention_required);`classify_and_persist` 终态 not_sent/unknown 映射 delivery_result。派发:`tokio::spawn` 异步扇出到启用渠道(事件类型订阅过滤 + 账号绑定覆盖,未绑定账号走全部启用渠道),单渠道 10s 超时,结果 `accepted/not_sent/unknown` 落 `notification_deliveries`;`unknown` 开 issue(kind=`notify_unknown`)。**无订单 issue 去重**:0006 迁移补 `CREATE UNIQUE INDEX idx_issues_open_dedup_account ON issues(account_id, kind, reason_code) WHERE state='open' AND order_id IS NULL AND account_id IS NOT NULL;`(既有去重索引要求 order_id NOT NULL)。

**Rationale**: 复用既有事件事实链路,不重复判定;secret 脱敏回显对齐 Ydisks `*_configured` 模式(编辑留空=不修改)。

**Alternatives considered**: 独立事件总线/重试队列——拒绝:宪章 III 轻量,unknown 转人工已覆盖可靠性。

## D9 订单一键同步:包装 TraceScanService 为任务

**Decision**: `POST /api/v1/orders/syncs {account_ids?: []}` → `JobService::create("order_sync")` + spawn:逐账号调用既有 `TraceScanService::run_once` 模式(`adapter.trace_sold_orders` 列表拉取,`mtop.taobao.idle.trade.merchant.sold.get`,30 行/页)→ 候选逐单 `handle_payment`(新增/恢复/修正全部经既有 find_or_create + 事实摄取);窗口参数化(默认 7 天,替代 trace.rs 硬编码 24h,向后兼容默认值不变);循环内每账号检查 `jobs.compare_and_set` 的 `cancel_requested` 兑现取消。结果 DTO 按账号 `{orders_seen, created, restored, reassigned, ineligible, failed:[{order_id, reason}], coverage}`;离线账号跳过并注明 `account_offline`。单笔同步 = `POST /accounts/{aid}/orders/{oid}/syncs` 直接调 handle_payment(带 30s 超时,同步返回 HandleOutcome)。

**Rationale**: research 证实列表能力与摄取管线完整存在,任务机制(job/create/cancel/轮询端点)现成;语义与规格澄清"按账号全量对账"一致。

**Alternatives considered**: 新写批量订单详情接口——拒绝:适配器仅暴露列表 ID+逐单详情,现管线已按此设计;前端循环单笔同步——拒绝:无统一进度/取消。

## D10 人工动作扩展:trigger_delivery / confirm_shipment

**Decision**: 复制 `resend` 骨架(reason 1-500 字校验 → `IdempotencyService::begin("trigger_delivery"/"confirm_shipment")` → `manual_actions::insert` + guard 同事务 → 执行 → complete):
- `trigger_delivery`:前置资格核验(付款事实齐备:paid_at、金额、买家定位;缺失 → 422 `ineligible_order` 附缺失字段列表);通过后调 `delivery.handle_payment`(T1 唯一索引天然幂等,重复触发走 `IdempotentReplay` 或 AlreadyHandled)。manual_actions.action=`'trigger_delivery'`。
- `confirm_shipment`:前提=交付 `content_state='accepted'`(消息已确认送达);调 `adapter.confirm_shipment`(既有 ConfirmOutcome 端口)→ 成功置 confirmation 轴 confirmed(origin 平台)+ proof;平台不支持/失败时仅记录人工断言(origin='manual',reason 留痕),两者都在 attempts 记 `action_kind='manual_confirm'`。
- issues 的 `allowed_actions` 增加 `"trigger_delivery"`(delivery_ineligible 的可重试子集);transport `manual_err` 映射表加分支;前端 `ACTION_META`/`manualActionBody`/`actionAvailability` 同步。

**Rationale**: 与既有四动作同构,幂等/审计/互斥免费获得;核验缺失即拒绝 = 宪章"人工发起不跳过核验"。

## D11 新增依赖评审(宪章 III)

**Decision**: 仅新增三个库 + 一个 feature 开关,全部记录如下:

| 依赖 | 用途 | 裁量 |
|---|---|---|
| `lettre 0.11`(features: `smtp-transport`,`builder`;native-tls) | email 通知渠道 SMTP | 纯 Rust,与既有 native-tls 栈共享;禁用默认全 feature 控制体积。替代:手写 SMTP 客户端——拒绝(STARTTLS/认证边界矩阵风险高)。 |
| `calamine`(最新稳定) | xlsx 批量导入读取 | 纯 Rust;注意与 chromiumoxide 传递的 zip 8.x 并存(第二个 zip 版本,接受二进制增量)。替代:仅支持 CSV/TSV——拒绝:规格 FR-012 明确含电子表格。 |
| `csv 1.x` | CSV/TSV 导入 | 零原生依赖,手写解析的转义边界风险高于库。 |
| axum `multipart` feature | 图片上传、导入文件接收 | 仅开 feature,无新依赖。 |

**Rationale**: 全部为库而非服务,不引入运行进程/端口;与"首版不得默认引入外部设施"不冲突。每项在 tasks 中有对应"依赖过审"检查点。

## D12 迁移划分与机制

**Decision**: 三个只追加迁移,按增量划分(细节见 data-model.md):`0004_cards_templates_rules.sql`、`0005_chat.sql`、`0006_notify_ai.sql`。`src/adapters/sqlite/migrations.rs` 的 `MIGRATIONS` 常量表追加三行 `(4|5|6, include_str!(...))`;**同步更新迁移测试中硬编码的表数量断言(现 25)**。规则索引重建在 0004 内完成(旧数据 trigger_type='order_paid'、priority=100,天然满足新索引,零迁移风险)。

**Rationale**: 每增量独立迁移 = 独立交付单位;迁移前自动备份机制(migrations.rs L58-70)已存在。

## D13 前端:拆分与复用清单

**Decision**: 零新依赖。路由改造:`/catalog` 拆为 `/items`(商品列表,自 catalog/products 迁移)与 `/rules`(自动化规则,rule-editor 表单主体提取为 `renderRuleForm` 供独立页复用,match-preview 零改动);新增 `/cards` `/templates` `/chat` `/notifications`;`/settings` 扩展 AI 配置卡与凭据修改卡(表单三件套 field/hint/field-error,敏感操作 openConfirmFlow)。聊天徽标:复用 `ShellNav.badge` + `app/store.ts` 新增 `chatUnread` 内存通道(聊天页/概览轮询时写入)。订单页新人工动作按 ACTION_META 表扩展;概览第 5 卡按 contracts 可选字段降级模式。样式仅在 components.css 尾部按 `/* ===== 卡密库存(007)===== */` 分节追加;localStorage 沿用 `qing-<feature>-<attr>` 命名。

**Rationale**: 两轮研究证实既有积木(三态/确认流/抽屉/分段/轮询/手写 SVG 图表/自绘图标)覆盖全部新页面需求。

## D14 新增错误码

**Decision**: `transport/error.rs` 三处 match(snake 名/状态/retryable)新增:`stock_insufficient`(422)、`referenced_resource`(409,被引用的卡组/模板删除拒绝)、`payload_too_large`(413,上传超限)、`reply_limit_reached`(422,快捷回复 50 上限)、`credential_change_failed`(422,改密当前密码不符)。其余场景复用既有 22 码(`content_too_long`、`unsupported_capability`、`incomplete_order`、`idempotency_conflict` 等)。

## D15 聊天消息明文存储的权衡(显式接受)

**Decision**: `chat_messages.body_text` 与会话摘要明文存储(不加密),理由:①规格 FR-040 要求按消息内容搜索,加密使 LIKE 检索不可行;②库文件为本机单管理员数据,访问边界=OS 用户;③交付正文的正式持久化仍走加密 content_snapshots,聊天记录是"已发送/已接收的展示副本"。风险已在 data-model 标注;备份归档沿用既有全库加密机制。

**Alternatives considered**: 加密+内存解密检索——拒绝:万级消息全表解密成本与复杂度不成比例;加密+独立检索索引——拒绝:宪章 III 轻量,过度设计。

---

## 覆盖核对(规格 FR → 决策)

| 规格域 | FR | 决策 |
|---|---|---|
| 横切 | FR-001~005 | D12/D13/D14 + 既有中间件 |
| 卡密库存 | FR-010~017 | D2/D3 + data-model 卡密表 + stats 第 5 卡 |
| 发货模板 | FR-020~023 | D4 + data-model 模板表 |
| 自动化规则 | FR-030~039 | D1/D2/D4/D7 + 变体/关键词/默认回复/求评表 |
| 在线聊天 | FR-040~046 | D5/D6/D15 + chat 表 |
| 通知渠道 | FR-050~054 | D8/D11 + notify 表 |
| 订单同步与人工 | FR-060~064 | D9/D10 |
| 系统与AI | FR-070~073 | D7 + system_settings/secrets + 改密用例 |
