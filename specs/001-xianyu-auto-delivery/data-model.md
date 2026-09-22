# Data Model: 轻交付首版

依据：[spec.md](spec.md)、[research.md](research.md)。这是逻辑结构与状态设计，非迁移实现。
数据库采用本地SQLite；所有业务时间为UTC毫秒，HTTP用RFC3339 UTC字符串。
本地ID采用UUID字符串，平台ID按原字符串保存，金额用整数分与币种，数量用正整数。

## 公共身份、版本与完整性

- 订单外部唯一身份为 `(platform, account_id, external_order_id)`，不能只用订单号。
  平台账号唯一为 `(platform, external_user_id)`；一套本地实例不得重复接入同一平台身份。
- 每个可修改聚合有整数 version；UI的 expected_version 乐观并发校验失败返回冲突。
- SKU由完整属性键值对组成，按平台属性ID/值ID排序后生成规范键；缺任一维度标 incomplete，
  不能与单规格空组合混同。无SKU仅在平台明确单规格事实时使用 single 键。
  首版不做子串、标题或部分规格匹配，无法稳定映射属性身份时进入待处理。
- 订单保存 fact_source、observed_at、platform_revision（若有）、completeness 与 conflicts。
  终态不得被旧付款事件倒退；无可靠新旧顺序时复查详情，冲突不执行。
- 本地平台事件ID不是业务交付幂等键；即使同一订单收到不同格式的通知仍只有一条初始交付。
- 敏感列保存 `ciphertext, nonce, key_id, format_version`，AAD绑定用途、实体ID和内容版本。
  连接密钥与解密Cookie不进入普通仓储摘要、HTTP DTO或日志。

## 实体与约束

| 实体 | 主要字段 | 约束与关系 |
| --- | --- | --- |
| installation | id、schema_version、created_at、restore_epoch、quarantine_started_at、restore_manifest_id | 单行；迁移/恢复失败禁止执行，恢复代次参与执行资格 |
| admin | id、username、password_hash、created_at、password_changed_at | 单管理员；初始化原子完成；Argon2id；不保存明文 |
| sessions | token_hash、admin_id、csrf_hash、expires_at、revoked_at | token只在HttpOnly Cookie；退出/改密/恢复撤销；12小时绝对有效期 |
| accounts | id、platform、external_user_id、display_name、runtime_enabled、auto_delivery_enabled、auto_confirm_enabled、monitor_since、status、control_epoch、credential_epoch、version | 新账号3开关均关闭；monitor_since首次启用写入且暂停不重置；不含可序列化秘密 |
| account_credentials | account_id、encrypted_cookie_jar、encrypted_runtime_metadata、key_id、generation、updated_at | 专用访问口；完整域/路径/有效期；迟到低代次更新拒绝 |
| auth_flows | id、account_id可空、generation、status、expires_at、bound_user_id、session_ref | 二维码内容/临时会话受保护且到期清理；取消、完成、过期不可被晚到结果覆盖 |
| items | id、account_id、external_item_id、title、listing_state、sku_definition、sku_completeness、last_seen_sync、version | 唯一账号+商品；缺失商品只在完整同步后标下架；历史引用不删 |
| sync_jobs | id、account_id、kind、status、cursor、coverage_from/to、complete、gap_reason、generation | 商品同步与订单追溯分开；分页上限/取消/异常均不能complete=true |
| rules | id、account_id、item_id、sku_key、enabled、current_content_version、version | 启用记录对账号+商品+sku_key唯一；禁用不删除历史 |
| rule_contents | id、rule_id、content_version、encrypted_text、text_digest、char_count、byte_count、created_at | 每规则版本唯一且不可变；产品1000标量/4000字节限制，保留换行 |
| orders | id、platform、account_id、external_order_id、buyer_id、item_id、sku_pairs、sku_complete、amount_minor、currency、quantity、paid_at、trade_type、platform_status、observed_at、fact_version、role_verified、history_class | 外部三元唯一；fact字段允许未知但不允许当已核验值使用；无可信paid_at为unknown历史归类 |
| order_facts | id、order_id、source、source_event_id、platform_revision、observed_at、normalized_evidence、evidence_digest | 只保存必要规范字段与脱敏证据；未知身份不能把不相干订单并入 |
| inbound_events | id、account_id、source_event_id、payload_digest、received_at、processed_at、result | 有稳定事件ID时唯一；否则摘要仅作辅助去重；不保存完整敏感原始帧 |
| deliveries | id、order_id、kind、parent_delivery_id、rule_id、content_snapshot_id、content_state、review_state、confirmation_state、evidence_origin、retry_count、next_retry_at、version、control_epoch、restore_epoch | initial每订单唯一；resend关联原交付；已成功内容状态不得为重试改回待发送 |
| content_snapshots | id、order_id、source_content_id、encrypted_text、digest、created_at | 首次任务建立时冻结；补发引用同一快照；不从当前规则再生成 |
| attempts | id、delivery_id、action_kind、sequence、request_id、state、prepared_at、handoff_at、finished_at、result_code、proof_ref、credential_epoch、control_epoch | 每动作/序号唯一；请求关联mid保存于提交前；结果终态不覆盖，仅追加纠正证据 |
| delivery_proofs | id、attempt_id、origin、platform_message_id、request_id、buyer_id、chat_id、content_digest、accepted_at、manual_action_id | origin=platform/manual；人工不能伪造平台标识；完整内容另存快照 |
| order_execution_guards | order_id、delivery_id、attempt_id、execution_generation、state | order_id唯一；自动/人工共用；不以租约过期授权未知尝试重发 |
| issues | id、order_id可空、account_id、delivery_id可空、kind、reason_code、allowed_actions、state、created_at、resolved_at、version | 同一未解决原因不反复创建；摘要无正文；允许操作由当前事实计算 |
| manual_actions | id、admin_id、order_id、delivery_id、action、reason、risk_confirmed、request_key、created_at、result_ref | 追加式审计；补发/接管/确认/终止均记录；原因1—500字符 |
| command_receipts | idempotency_key、actor_id、operation、request_hash、resource_id、status、created_at | 同键同请求返回原结果，同键异请求冲突；有外部动作的记录随交付历史保留 |
| operation_jobs | id、kind、target_id、state、stage、version、cancel_requested、result_ref、safe_error、created_at、updated_at | HTTP的202任务句柄；queued/running/cancel_requested/succeeded/failed/cancelled/needs_review；关联业务任务但不是第二个发送状态机 |
| restore_reviews | restore_epoch、order_id、status、evidence_origin、reason、reviewer、reviewed_at | 恢复隔离逐单解除，不能通过总开关解除；涵盖旧未完任务和恢复区间新发现订单 |
| backup_manifests | id、database_id、schema_version、app_version、snapshot_started_at、snapshot_finished_at、key_id、checksum、restore_policy | 归档外部manifest含最少非敏感字段；身份/业务元数据仍在加密负载 |

不建立卡密库存表、计费表或通用插件表；这些在独立需求中定义，当前只保留内容来源的明确类型。
只读详情与列表用独立查询模型，不暴露 account_credentials 或存储密文结构。

## 账号与授权状态

`disabled → connecting → online`；连接故障进入 `offline` 并按退避重连；
验证要求进入 `needs_verification`，会话失效进入 `auth_expired`；完成重新授权且身份一致才重连。
`pausing → paused` 必须等控制屏障完成。`runtime_enabled` 是期望状态，status是观测状态，二者不混同。
所有新账号默认关闭运行/交付/平台确认；登录完成后管理员显式启动运行并选择自动化开关。
重启恢复用户此前允许的运行状态，但restore_quarantine覆盖这些设置。

授权流程状态：`pending → scanned → confirmed → completed`，或 `canceled/expired/failed/needs_verification`。
二维码confirmed并非在线，只有身份核验、必要凭证和连接注册完成才可运行。
连接故障不删除授权；重新登录成功使 credential_epoch递增，旧异步任务不能回写新凭证。

## 订单与资格

平台状态：`unknown/unpaid/pending_ship/shipped/completed/canceled/refunding/refunded`。
交易类型：`ordinary/bargain/unknown/unsupported_bundle`；本版只接受ordinary且字段完整。

自动初次交付必须：账号在线、运行与交付开启、订单pending_ship、完整卖家及买家身份、
完整金额数量SKU、规则启用、商品有效、可信paid_at>=monitor_since、未恢复隔离、无初次完成或未知动作。
金额不用于猜测SKU，0金额也必须由平台事实证明，不作为默认值。
历史接管显式允许paid_at早于monitor_since，但不放宽付款、身份、状态与恢复核对条件。
人工补发允许shipped但拒绝completed/refunding/refunded/canceled，且仍需全部其他资格。

## 交付状态机

content_state：`pending_verification/queued/dispatching/not_sent/accepted/unknown/terminated`。
review_state：`none/required/resolved`；UI将required显示“等待人工”，同时保留底层未知或未发送原因。
confirmation_state：`disabled/pending/dispatching/accepted/rejected/unknown/terminated`。
两个结果轴独立，不能用一个“发货失败”覆盖已经交付的内容。

| 触发 | 前置状态 | 结果与副作用 |
| --- | --- | --- |
| 可信付款/合法补偿 | 无initial记录 | 事务创建pending_verification与订单事实；完整匹配后冻结快照进入queued |
| 资格缺失 | pending_verification/queued | required+issue；不发，不消耗发送重试预算 |
| 获得执行权且资格版本一致 | queued/not_sent | attempt持久化dispatching；得到事务明确结果后才handoff |
| 传输证明未提交或平台明确拒绝未接收 | dispatching | not_sent；仅暂时故障按预算重试，永久拒绝required |
| 严格关联平台接收证明 | dispatching | accepted+platform proof；原内容不可改 |
| 超时、断连、启动发现未定尝试 | dispatching | unknown+required；禁止自动重试 |
| 人工确认已收到 | unknown/required | accepted+manual proof；平台确认不自动触发 |
| 显式补发原内容 | accepted、unknown/required或not_sent/required（含自动重试耗尽） | 新resend子交付引用原快照；原记录保留；旧动作被人工接管后不能自行恢复 |
| 取消/退款/身份冲突 | 未提交动作 | 不合格者terminated或required；已提交尝试保留事实，不能抹除发送 |
| 平台确认已开启且内容accepted有平台证明 | confirmation pending | 独立dispatching尝试；人工证明要求显式确认操作 |
| 平台确认超时 | confirmation dispatching | unknown；查询平台；确已shipped则补证，无法确认则required |

正常任务初次发送后最多再重试3次；同任务重启不重置。人工resend新动作有自己的预算，
但未知结果不能进入自动预算。强制绕过未知状态的通用 retry 接口不存在。
查询平台仍pending_ship仅能证明尚未平台确认，不能证明正文从未发送。

## 事务、并发与崩溃

1. T1保存事实与唯一任务；T2完成快照与资格版本；T3取得order guard并写dispatching attempt。
   使用BEGIN IMMEDIATE等短写事务表达互斥；唯一约束是最终防线，内存锁只作优化。
2. T3响应可能超时而实际提交：不得执行新的T3后直接发送；按operation/request_id读取已保存结果。
3. T3与外部handoff之间不可能做到跨系统原子。重启无证据统一unknown，宁可人工核对。
4. 暂停意图先落库control_epoch，再进入账号执行器屏障；最终发送入口检查代次并确认是否handoff。
   所有未交出动作明确取消；已交出只等待结果。暂停超时仍pausing而不是成功。
5. 规则disable与商品下架使相关资格版本变化；排队结果过期不能继续发送。
6. 同一订单的resend/initial/confirm互斥；guard可在确定终态释放，未知时只能显式人工解决。
   不使用“超过N秒就重发”的guard租约。
7. 持久化结果失败使账号进入storage_error禁止新动作；重启复原时按未知处理，
   不因内存里曾看到成功就伪造已保存证据。

## 备份恢复与迁移

业务连接WAL+FULL，一致快照经online backup写入独立目标；首版备份仅在停机独占CLI中执行。
归档绑定数据库ID、schema、密钥ID、版本和时间区间。拷贝运行中的主.db不是受支持的备份流程。
恢复只允许主服务停止且取得独占目录锁；校验、解密、quick_check与必要迁移在暂存目录完成，
写restore_epoch、全部账号暂停、会话撤销后才切换到新数据；任何失败保留原目录可恢复。

恢复隔离包括：备份时所有未完/未知任务，及snapshot_started_at到restore_finished_at可能发生交付的订单。
即使订单创建早于备份，也不能绕过。新发现订单按可信付款时间分类；事实不足保持隔离。
恢复后的明确新付款且晚于restore_finished_at，在账号重新启用并满足规则后可正常执行。

恢复核对在同一事务中写restore_review、人工审计、相关delivery和guard：received追加人工证明并
终结旧自动执行路径，terminated终止旧路径，approved_not_sent保留manual_only执行门槛。
这些路径均不使旧queued/not_sent重新被调度器捡起；明确接管/补发才建立可执行动作。
manual_only作为delivery的execution_policy保存（auto/manual_only），不得只在UI隐藏自动执行按钮。

数据密钥封装限制同机器同用户；跨用户或密钥损坏显示不可恢复，不覆盖密钥。
备份和迁移禁止并行密钥轮换；首版不提供轮换UI。迁移按schema单向版本执行，失败回滚，
不支持直接运行旧程序打开新schema；回退必须按备份恢复流程并保留隔离。
默认保留全部订单、快照、操作幂等与审计，不引入可能破坏去重的自动清理。

## 查询索引与验收映射

索引至少覆盖账号+订单号唯一、账号+状态+更新时间、待处理状态+时间、next_retry_at、
账号+外部商品ID、规则启用范围、事件ID和命令幂等键。分页limit默认50/最大100，按稳定游标。

- FR-001—005 → admin/sessions/accounts/auth_flows与控制状态。
- FR-006—009 → items/sync_jobs/rules/rule_contents。
- FR-010—018 → orders/order_facts/inbound_events/deliveries/content_snapshots。
- FR-019—024 → attempts/proofs/guards/issues/manual_actions/command_receipts。
- FR-025—031 → 账号范围查询、秘密模型、installation/backup_manifests/restore_reviews及目录锁。
- SC-003/004/008 必须用真实临时SQLite与进程崩溃测试验证，不能只在内存模拟事务。
