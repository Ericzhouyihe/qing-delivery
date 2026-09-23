# Tasks: 轻交付首版——闲鱼虚拟商品自动发货

**Input**: 设计文档 `/specs/001-xianyu-auto-delivery/`（spec.md、plan.md、research.md、data-model.md、quickstart.md、contracts/）

**Prerequisites**: plan.md（技术栈与结构）、spec.md（用户故事 US1—US5 与 FR-001—031）、data-model.md（实体与状态机）、contracts/（HTTP/平台/CLI 契约）、宪章 2.0.0

**Tests**: 本规格明确要求行为验证（宪章 IV、quickstart V01—V12、SC-001—009），测试不作为独立前置阶段，而是并入各故事任务并在描述中标注对应 V 编号与验收场景；每个故事阶段的 Checkpoint 必须以可运行测试收尾。

**Organization**: 任务按用户故事分组。US1—US4 为 P1、US5 为 P2；实现顺序 US1 → US2 → US3 → US4 → US5（对应 plan.md 顺序 A—F：US1 以假平台先落交付内核，US2 再接真实协议）。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行（不同文件、无未完成依赖）
- **[Story]**: 所属用户故事（US1—US5）；Setup/Foundational/Polish 阶段无故事标签
- 所有任务必须给出文件路径

## Path Conventions

单仓库单 Rust crate + 嵌入式前端（plan.md Project Structure）：
`src/`（domain/application/runtime/adapters/transport）、`frontend/src/`、`migrations/`、`tests/`（fixtures/contract/integration/e2e）、`scripts/`。
`Ydisks-Xianyu-Helper/` 仅为本地参考，禁止进入 Git、构建、测试与发行包。

---

## Phase 1: Setup（工具链与实验，对应 plan 阶段 A）

**Purpose**: 锁定工具链、建立可验证的最小构建与假平台测试环境（research R8 门禁）

- [x] T001 创建 `rust-toolchain.toml`（锁定实测通过的 stable 完整版本，最低 1.85，以依赖 MSRV 为准）与根 `Cargo.toml`（Rust 2024 edition，单 crate；Tokio 1.53.1、Axum 0.8.9、rusqlite 0.40.2 bundled+backup、reqwest、tokio-tungstenite、serde、rmpv、tracing、Argon2id、aes-gcm、windows；dev 依赖 chromiumoxide 0.9.1），在 Windows MSVC 完成 `cargo build` 后提交 `Cargo.lock`
- [x] T002 [P] 搭建前端脚手架：`frontend/package.json`（typecheck/test/build scripts）、`frontend/tsconfig.json`、`frontend/vite.config.ts`（回环代理同源后端，不设通配 CORS）、`frontend/src/app|shared|features` 目录与空模块
- [x] T003 [P] 执行 research R8 依赖最小实验：Windows MSVC 下分别验证 TLS（reqwest）、SQLite（rusqlite bundled）、DPAPI（windows）、静态资源嵌入、tokio-tungstenite 对本地假 WS 服务的连通，结果与版本调整记录到 `docs/deps-validation.md`
- [x] T004 [P] 构建本地假平台测试服务：HTTP/WS 假服务与脱敏合成协议样本（WS sync 帧、mtop 商品/订单/详情响应、扫码重定向夹具），落在 `tests/fixtures/` 与测试支撑模块（仅 dev-fixtures feature 可用）
- [x] T005 [P] 建立质量门禁脚本 `scripts/verify.ps1`：串联 quickstart §2 命令（rustc/cargo/node/npm 版本、`cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`、`npm ci/typecheck/test/build --prefix frontend`），非零退出阻止发布
- [x] T006 [P] 配置排除规则：`.gitignore` 增加 `target/`、`frontend/node_modules/`、`frontend/dist`、`.work/`；核对 `Ydisks-Xianyu-Helper/` 未被 Git 跟踪，并建立发行白名单骨架 `scripts/release-whitelist.txt`

---

## Phase 2: Foundational（阻塞性公共基础设施，对应 plan 阶段 B）

**Purpose**: 全部用户故事依赖的目录锁、密钥、DB 线程、schema、认证与 HTTP 通用语义

**⚠ CRITICAL**: 本阶段未完成前不得开始任何用户故事实现

- [x] T007 实现数据目录引导：解析规范化 `--data-dir` 为绝对路径（默认 `%LOCALAPPDATA%\QingDelivery\data`）、拒绝 UNC/网络卷、应用当前用户 ACL，落在 `src/adapters/windows/datadir.rs`
- [x] T008 实现进程级独占目录锁：路径归一化后检查（别名路径不能绕过）、持有到退出、第二实例报错并以退出码 3 结束，落在 `src/adapters/windows/lock.rs`（FR-030）
- [x] T009 实现数据密钥与 AEAD 信封：首次创建才生成随机密钥并用当前用户 DPAPI 包装；aes-gcm 加密列格式 `ciphertext, nonce, key_id, format_version`，AAD 绑定用途/实体 ID/内容版本；解密失败停止且不覆盖旧密钥（退出码 4），落在 `src/adapters/windows/dpapi.rs` 与 `src/domain/crypto.rs`
- [x] T010 实现专用 SQLite DB 线程：有界命令队列 + 一次性响应、WAL、synchronous=FULL、foreign_keys=ON、busy_timeout=2000ms、禁止事务内网络等待，落在 `src/adapters/sqlite/db.rs`
- [x] T011 实现版本化迁移器：schema_version 单向递增、事务化、迁移前受保护备份、失败中止启动，落在 `src/adapters/sqlite/migrations.rs` 与 `migrations/`
- [x] T012 创建初始 schema `migrations/0001_init.sql`：按 data-model.md 建 24 个实体（installation、admin、sessions、accounts、account_credentials、auth_flows、items、sync_jobs、rules、rule_contents、orders、order_facts、inbound_events、deliveries、content_snapshots、attempts、delivery_proofs、order_execution_guards、issues、manual_actions、command_receipts、operation_jobs、restore_reviews、backup_manifests），关键约束原样落地：orders 唯一 `(platform, account_id, external_order_id)`、deliveries 每订单唯一 initial、rules 启用范围唯一 `(account_id, item_id, sku_key)`、attempts 唯一 `(delivery_id, action_kind, sequence)`、order_execution_guards 的 order_id 唯一、command_receipts 幂等键唯一；按 data-model「查询索引」节建索引
- [x] T013 [P] 实现 domain 基元：不透明字符串 ID、UTC 毫秒与 RFC3339 映射、整数分金额 Money、聚合 version、SKU 规范键（按属性 ID/值 ID 排序；incomplete 与 single 明确区分，不混同），落在 `src/domain/`
- [x] T014 实现管理员认证：Argon2id、单管理员原子初始化（密码 ≥12 字符且两次一致，唯一约束抵御并发初始化）、登录/退出、会话表（token_hash、csrf_hash、12 小时绝对有效期、退出/改密/恢复撤销）、`qing_session` HttpOnly SameSite=Strict Cookie、登录限流与统一错误，落在 `src/application/auth/` 与 `src/transport/auth.rs`（FR-001）
- [x] T015 实现 HTTP 中间件栈：`GET /health`（仅 status+version，降级 503）、Host/Origin 回环+端口校验（无通配 CORS）、变更请求 CSRF 强制、`Cache-Control: no-store`、按 contracts/http-api.md §1 错误表（400/401/403/404/409/422/429/503 + ErrorCode）构造错误信封，落在 `src/transport/middleware.rs`（FR-029）
- [x] T016 实现通用请求语义：`Idempotency-Key`（同键同参返回原结果、同键异参 409 idempotency_conflict、重放不误判版本冲突）、`expected_version` 乐观并发（409 version_conflict 附 current_version）、分页 limit 默认 50/上限 100、不透明游标绑定筛选条件、稳定 `(created_at, id)` 排序，落在 `src/transport/` 与 `src/application/idempotency.rs`
- [x] T017 实现 operation_jobs 框架：JobKind/JobState 全状态（queued/running/cancel_requested/succeeded/failed/cancelled/needs_review）、`202 + Location + AcceptedOperation`、取消仅停止未提交的未来步骤（否则 409 action_in_progress 附原 job_id），落在 `src/application/jobs/` 与 `src/transport/jobs.rs`
- [x] T018 实现 `GET /capabilities`：仅闲鱼+固定文字 supported，淘宝/卡密等明确 unsupported 带原因，ContentLimits（1000 Unicode 标量/4000 UTF-8 字节，effective_source），execution_profile live|mock，落在 `src/transport/capabilities.rs`（FR-031）
- [x] T019 实现 serve 生命周期与 CLI 解析：`serve --data-dir/--bind/--profile live|mock`、`--version/--help`；启动顺序按 contracts/cli.md（目录检查→独占锁→密钥→迁移→DB 线程→未决尝试恢复→supervisor→绑 HTTP）；拒绝非回环 bind；Ctrl+C 停止接新任务、持久化停止意图、≤15 秒收尾、未知结果保留并以退出码 7 区分，落在 `src/main.rs` 与 `src/transport/cli.rs`
- [x] T020 [P] 实现前端应用壳：路由与会话引导（`GET /auth/session` 取 CSRF）、初始化/登录/退出页面（12 字符客户端提示）、shared HTTP 适配器（AbortController 取消、错误信封映射、3 秒摘要轮询且页面隐藏停止）、契约类型 + 运行时校验，落在 `frontend/src/app/` 与 `frontend/src/shared/`
- [x] T021 实现 tracing 与脱敏：request_id 关联、Cookie/Token/密码/交付正文禁止入日志、安全错误码，落在 `src/transport/middleware.rs` 与 `src/main.rs`
- [x] T022 建立 HTTP 契约测试基座：临时数据目录 + 启动真实 handler，覆盖匿名拒绝、CSRF、错误信封、分页、幂等重放、版本冲突，落在 `tests/contract/`
- [x] T023 实现 dev-fixtures/mock profile 基础：Cargo feature `dev-fixtures` 独占 `--profile mock`、独立数据目录且写入 mock 标记（live 拒绝该目录）、进程内假适配器、`/api/v1/dev/scenarios` 固定 allowlist（鉴权+CSRF+幂等键）、拒绝非回环平台目标，落在 `src/adapters/mock/` 与 `src/transport/dev.rs`

**Checkpoint**: 基础就绪——`cargo test`、`npm run test --prefix frontend`、`cargo run -- serve` 健康检查全部可运行；用户故事可开始

---

## Phase 3: User Story 1 - 付款后收到正确的商品链接（Priority: P1）✅ MVP

**Goal**: 可信付款事件 → 核验 → 唯一交付任务 → 冻结快照 → 一次发送 → 严格证明 → 独立平台确认（以假平台确定性验收）

**Independent Test**: 预置已授权账号、一条商品规则及可控订单，分别提交可信付款通知、普通聊天和重复通知；检查指定买家收到的内容、次数及两个发货状态（spec US1）

### Implementation for User Story 1

- [x] T024 [P] [US1] 定义平台消费者接口：按 contracts/platform-adapter.md §1 能力表（start/observe/cancel_authorization、start/stop_account、refresh_credentials、list_products、trace_sold_orders、fetch_order_snapshot、resolve_conversation、send_text、confirm_shipment、verify_shipment_state）与 §2 错误分类（cancelled_before_submit/timeout/rate_limited/…/browser_unavailable），落在 `src/application/ports/platform.rs`
- [x] T025 [US1] 实现入站事件管道：七类规范事件信封、inbound_events 持久化（有稳定事件 ID 时唯一，否则摘要仅辅助）、按原始顺序处理同帧全部条目（坏条目记 protocol_issue 后继续，不吞后续付款）、每账号有界队列与背压/同步缺口上报，落在 `src/application/events/`
- [x] T026 [US1] 实现订单事实摄取：orders 按 `(platform, account_id, external_order_id)` upsert、order_facts 追加（source、platform_revision、evidence_digest）、fact_version 单调、终态不被旧付款事件倒退、未知身份不并入不相干订单，落在 `src/application/orders/`
- [x] T027 [P] [US1] 实现 OrderSnapshot 完备性模型：逐字段 verified/missing/conflict/unsupported + complete_for_delivery；数量缺失不是默认 1、金额缺失不当 0、paid_at 只能来自明确付款语义（创建/接收/更新时间不可替代）、会话身份必须已核验买家+账号，落在 `src/domain/orders/snapshot.rs`（FR-010、FR-011）
- [x] T028 [P] [US1] 实现交付状态机（纯 domain）：content_state `pending_verification/queued/dispatching/not_sent/accepted/unknown/terminated`、review_state `none/required/resolved`、confirmation_state `disabled/pending/dispatching/accepted/rejected/unknown/terminated` 三轴独立，accepted 不因重试回退；含 data-model 状态迁移表全部触发器，落在 `src/domain/delivery/`（FR-015、FR-016）
- [x] T029 [US1] 实现 fetch_order_snapshot：协议优先拉单订单详情补齐缺失字段，浏览器兜底以接口边界预留（本阶段接假平台），完整性与来源标注，落在 `src/adapters/xianyu/orders.rs`
- [x] T030 [US1] 实现交付资格核验：账号在线+运行+自动交付开启、订单 pending_ship、完整买卖家身份、完整金额/数量/SKU、规则启用且商品有效、可信 paid_at ≥ monitor_since、未恢复隔离、无已完成或未知 initial 动作；不满足 → pending_verification + issue，不发送、不消耗重试预算，落在 `src/application/delivery/eligibility.rs`（FR-011、FR-012）
- [x] T031 [US1] 实现 T1/T2/T3 短事务流：T1 保存事实+唯一 initial 交付、T2 冻结 content_snapshot+规则版本进入 queued、T3 取 order guard 并在 handoff 前持久化 dispatching attempt（request_id 关联 mid 先落库）；BEGIN IMMEDIATE 表达互斥；T3 结果不明确时按操作/请求 ID 查询已保存结果，不得重放 T3 后直接发送，落在 `src/application/delivery/commit.rs` 与 `src/adapters/sqlite/repos/`
- [x] T032 [US1] 实现账号执行器：独占 Cookie 代次、WS 写通道、控制代次与待提交动作；control 与 transport handoff 串行化；最终 handoff 前复查控制代次/凭证代次/规则版本/开关/恢复隔离；DB 事务外才碰网络，落在 `src/runtime/executor.rs`
- [x] T033 [US1] 实现发送严格关联证明：登记等待器先于写出；按 request_id+mid+平台消息 ID+买家+会话+正文摘要关联；裸 200/socket 写成功/心跳 ACK/历史同文消息不构成 accepted；迟到回显不能确认后一次发送、重复证据只消费一次；无法关联的回显保留待核对，落在 `src/application/delivery/proof.rs` 与 `src/adapters/xianyu/ws/`
- [x] T034 [US1] 实现结果分类与自动重试：not_submitted（仅暂时故障可按预算重试）、rejected（永久拒绝 → required）、accepted、unknown（unknown+required，禁止自动重发与自动平台确认）；预算为初次外最多 3 次、间隔 30/60/120 秒、随任务持久且重启不重置，落在 `src/application/delivery/retry.rs`（FR-019）
- [x] T035 [US1] 实现独立平台确认：内容 accepted 且有平台证明且 auto_confirm 开启才确认；独立 attempt 与 confirmation_state；确认失败/未知仅核验确认步骤（verify_shipment_state），绝不触发 send_text；人工"已收到"不自动确认，落在 `src/application/delivery/confirm.rs`（FR-016）
- [x] T036 [P] [US1] 去重硬化测试：每单 20 次重复、并发、乱序重放及重启后再次出现 → 每单自动发送不超过一次（SC-003），落在 `tests/integration/test_delivery_dedup.rs`（FR-014）
- [x] T037 [P] [US1] 数量与 SKU 行为测试：数量 >1 记录数量但每单一份固定文字；完整规格组合唯一匹配才用对应文字；组合缺失/部分匹配/无匹配/歧义 → 待处理不发货（US1-4、US1-5），落在 `tests/integration/test_delivery_sku_quantity.rs`（FR-008、FR-013）
- [x] T038 [P] [US1] 非法事实拒绝测试：通知文案被复制进聊天、买家自述付款、未付款、当前账号买入订单 → 不发送也不标已付款；同批多订单逐笔处理、不跨账号串单，落在 `tests/integration/test_delivery_invalid_facts.rs`（FR-010、Edge Cases）
- [x] T039 [US1] V04/V05 验收测试：100 笔普通订单 × 3 账号，买家/内容匹配 100%、每单一条、中文换行保留、两个确认开关四种组合下内容与平台确认状态分离，落在 `tests/integration/test_delivery_acceptance.rs`
- [x] T040 [P] [US1] 前端订单视图：订单列表与详情将 content_state 与 confirmation_state 分开呈现（含 unknown/needs_review 保留底层分类）、交付时间线，落在 `frontend/src/features/orders/`

**Checkpoint**: 假平台上 US1 全部验收场景（US1-1—US1-7）通过——MVP 达成，可演示确定性交付管道

---

## Phase 4: User Story 2 - 接入、暂停和恢复自己的闲鱼账号（Priority: P1）

**Goal**: 扫码接入真实账号、可视可控的连接/验证状态、三开关与暂停屏障、失效恢复

**Independent Test**: 新数据环境完成管理员初始化，用可控账号登录与失效响应验证状态变化，预置待处理订单验证暂停后行为（spec US2）

### Implementation for User Story 2

- [x] T041 [US2] 实现闲鱼扫码登录协议：二维码创建/轮询/确认（先对假重定向夹具，参考 R4 行为不做逐行迁移）、授权完成后核对账号身份一致才建账号，落在 `src/adapters/xianyu/auth/qr.rs`（FR-002）
- [x] T042 [US2] 实现 auth_flows 应用与 HTTP：QrSession 状态机 `awaiting_scan → awaiting_authorization → authorized | expired | cancelled | failed | verification_required`、generation 绑定（取消/过期后迟到成功不覆盖新会话）、图片端点鉴权 no-store、每 3 秒轮询、cancel（已授权 409），落在 `src/application/accounts/qr.rs` 与 `src/transport/accounts.rs`
- [x] T043 [US2] 实现凭证存储与代次：加密 Cookie Jar 保留完整 domain/path/有效期属性、credential_epoch 单调、迟到低代次更新拒绝、签名 Token 更新与登录会话续期区分，落在 `src/adapters/xianyu/auth/credentials.rs` 与 `src/adapters/sqlite/`
- [x] T044 [P] [US2] 实现 mtop 签名与续期：签名 data 与实际发送字节一致、签名 Token 过期/登录失效/需验证错误分类分离、Token 续期只受控重签只读请求（业务重试取决于提交证据而非新 Token），落在 `src/adapters/xianyu/mtop/`
- [x] T045 [US2] 实现 WS 连接生命周期：连接/注册/心跳、读泵不被详情或发送等待阻塞、帧 ACK 按原始帧一次且保留完整关联头（ACK ≠ 业务提交）、断线指数退避 2→60 秒带抖动且成功重置、验证/失效等待人工不紧密重连，落在 `src/adapters/xianyu/ws/client.rs`
- [x] T046 [P] [US2] 实现 sync 帧解码：Base64/MessagePack 大小/深度/条目数界限、整数键兼容、按原始顺序处理全部条目，落在 `src/adapters/xianyu/codec/`
- [x] T047 [US2] 实现账号状态机与 supervisor：`connecting/online/offline/auth_expired/needs_verification` + paused（disabled 映射）、runtime_enabled（期望）与 status（观测）分离、每账号独立执行器（单账号故障不阻塞其他）、持久化失败 → storage_error 禁新动作，落在 `src/runtime/supervisor.rs`（FR-004）
- [x] T048 [US2] 实现暂停屏障：先落库 control_epoch 再进执行器屏障；屏障完成前 UI 仅显示 pausing；未交出动作明确取消、已交出只跟踪结果；暂停超时仍为 pausing 不谎报生效，落在 `src/runtime/executor.rs` 与 `src/application/accounts/control.rs`（FR-023）
- [x] T049 [US2] 实现账号控制 API：`POST /accounts/{id}/control`（202 幂等 job）；运行/自动交付/自动平台确认三开关默认关闭；首次启用自动交付必须 `acknowledge_monitoring_scope=true` 并写入 monitor_since（暂停不重置），落在 `src/transport/accounts.rs` 与 `src/application/accounts/control.rs`（FR-003）
- [ ] T050 [US2] 实现官方验证流程：检测到验证要求即暂停该账号新外部动作并给出入口；chromiumoxide 专用受限 profile（禁读日常 profile、禁自动滑块/验证码绕过、仅允许官方域导航）、延续原授权临时会话；完成必须依赖新可用授权+身份一致；缺浏览器明确不可用，落在 `src/adapters/browser/verification.rs`
- [ ] T051 [P] [US2] 实现浏览器管理器：进程/profile/CDP 生命周期所有权、数量有界、账号隔离、退出/取消只清理自建进程、按需启动，落在 `src/adapters/browser/manager.rs`
- [x] T052 [P] [US2] 前端账号页：账号列表（状态、三开关、pausing 指示）、扫码页（图片、3 秒轮询、超时重取、取消）、验证入口、监控范围确认对话框、替换账号流程，落在 `frontend/src/features/accounts/`
- [x] T053 [US2] 实现重启与值守语义：页面关闭不停值守；重启恢复此前允许的运行状态（恢复隔离覆盖为全暂停）；退出停止接新任务且已提交结果/未知保留，落在 `src/runtime/` 与 `tests/integration/test_lifecycle.rs`（FR-005）
- [x] T054 [US2] V02 验收测试：扫码超时/取消/晚到成功/需验证/断线；暂停与恢复屏障；其他账号不受阻塞继续工作，落在 `tests/integration/test_account_lifecycle.rs`

**Checkpoint**: 真实协议路径（夹具验证）+ US2 全部场景（US2-1—US2-7）通过

---

## Phase 5: User Story 3 - 为商品和规格配置发货文字（Priority: P1）

**Goal**: 商品同步、精确规格规则配置、预览与冲突防护、内容版本边界

**Independent Test**: 预置账号和商品列表，完成同步、配置、预览、修改及禁用，用订单匹配预览确认选中内容，不依赖真实消息发送（spec US3）

### Implementation for User Story 3

- [x] T055 [US3] 实现商品同步作业：`POST item-syncs`（202 job）；分页按平台约束；成功但省略 cardList 视为正常空页、明确错误不是空页；不完整/失败/取消绝不下架未返回商品；完整同步才将缺失商品标 off_sale；sync_jobs 记录 coverage/complete/gap_reason，落在 `src/application/items/sync.rs` 与 `src/adapters/xianyu/mtop/items.rs`（FR-006）
- [x] T056 [US3] 实现 items 持久化与 API：按 `(account_id, external_item_id)` upsert、sku_definition+completeness、listing_state、last_seen_sync、历史引用不删；`GET items` 分页带 rule_state（configured/missing/disabled/conflict），落在 `src/adapters/sqlite/repos/items.rs` 与 `src/transport/items.rs`
- [x] T057 [US3] 实现 rules CRUD：创建（不带 expected_version）/编辑/禁用（带 expected_version 走版本更新）；启用范围唯一 `(account_id, item_id, sku_key)` 冲突 422 rule_conflict，保存时阻止且执行时也检测歧义（绝不任选一条）；禁用不删历史；列表不返回正文、详情 no-store，落在 `src/application/rules/service.rs` 与 `src/transport/rules.rs`（FR-007、FR-008）
- [x] T058 [US3] 实现内容版本与限制：rule_contents 每版本不可变（encrypted_text AAD 绑定、text_digest、char_count、byte_count）；上限原样执行——最多 1000 Unicode 标量且 UTF-8 ≤4000 字节，空白或超长明确拒绝并显示原因，不截断、不拆分；换行/链接/中文保留；保存与执行前双重校验（若实证平台上限更低按较低值拒绝），落在 `src/application/rules/content.rs`
- [x] T059 [US3] 实现 preview 与 match-preview：preview 纯校验（rendered_text+计数+violations，不保存、不发平台请求）；match-preview 确定性精确匹配返回 matched/none/ambiguous/unsupported+规则版本，落在 `src/transport/rules.rs`
- [x] T060 [US3] 实现规则失效边界：规则禁用或商品下架 → 未开始的新交付转待处理不套用旧配置；内容更新只影响此后新建任务，既有任务与补发用冻结快照；资格版本变化的排队结果过期不发送，落在 `src/application/delivery/eligibility.rs`（FR-009）
- [x] T061 [P] [US3] 前端商品与规则页：商品列表+同步触发+配置状态徽标；规则编辑器（实时预览换行/链接/中文、冲突与超长错误、启用/禁用）；订单匹配预览面板，落在 `frontend/src/features/catalog/` 与 `frontend/src/features/rules/`
- [x] T062 [US3] V03 验收测试：空列表正常、部分分页失败不下架、完整同步下架、完整 SKU、冲突规则阻止、超长文字拒绝、预览零发送、旧快照不变，落在 `tests/integration/test_catalog_rules.rs`

**Checkpoint**: 卖家可在界面上完成同步→配置→预览→启用闭环（SC-001 五分钟路径）

---

## Phase 6: User Story 4 - 漏通知、掉线和发送异常后可控恢复（Priority: P1）

**Goal**: 漏单补偿、崩溃后 unknown 保持、人工补发/确认/终止、自动与人工互斥、历史接管

**Independent Test**: 预置待发货、已发货及结果未知任务，在发送前、提交后和结果记录前分别模拟中断，恢复后检查内容、次数、状态与人工处理选项（spec US4）

### Implementation for User Story 4

- [x] T063 [US4] 实现订单追溯扫描：trace_sold_orders 每 60 秒限量拉取已售列表、分页上限 100 页、每页按平台约束；输出 requested/observed 范围、coverage_complete、gaps、最后游标与终止原因；候选订单必须再拉详情核验；扫描源时钟 UTC、间隔用单调时间，落在 `src/application/recovery/scan.rs` 与 `src/adapters/xianyu/mtop/sold_orders.rs`（FR-018）
- [x] T064 [US4] 实现漏通知补偿：普通待发货首次被可靠观察后至少等 120 秒再补偿缺事件任务（给实时消息先处理机会）；有任务走任务恢复不建第二个 initial；未知或人工隔离禁止自动恢复；迟到付款通知不产生第二次发送；恢复后 5 分钟内交付或形成带原因待处理；不可追溯范围明示缺口，落在 `src/application/recovery/compensate.rs`（SC-005）
- [x] T065 [US4] 实现启动恢复：启动时发现 dispatching 且无可信结果的尝试统一 unknown+required；重启不清除未知状态、不重置已消耗重试预算；停机较久按实际可追溯数据报告，落在 `src/application/recovery/boot.rs`（FR-020）
- [x] T066 [P] [US4] 崩溃矩阵测试（V06）：T3 落库前/后、handoff 前/后、ACK 丢失、ACK 保存失败、确认前退出的进程级故障注入；断言可证未提交才重试、其余 unknown、结果持久化失败停止新增发送，落在 `tests/integration/test_crash_matrix.rs`
- [x] T067 [P] [US4] 控制竞争测试（V07）：暂停/规则禁用/凭证换代分别与资格检查、T3、handoff 竞争；第二实例目录锁；断言生效后无新提交、已提交仅跟踪、无双执行、锁一直持有，落在 `tests/integration/test_control_race.rs`
- [x] T068 [US4] 实现人工动作 API：`/resends`（幂等键+expected 双版本+原因+acknowledge_duplicate_risk，使用同一冻结快照，重新核验资格；FR-022 例外允许 shipped 未完成的普通订单，取消/退款中/退款成功/完成/账号错误拒绝；禁用规则/下架商品/暂停账号/关闭自动交付不可绕过）；`/mark-received`（仅追加人工证据，不发消息不自动平台确认）；`/terminate`（未提交才可立即终止，已提交 409 保留追踪），落在 `src/application/manual/actions.rs` 与 `src/transport/manual.rs`（FR-021、FR-022）
- [x] T069 [US4] 实现自动/人工互斥：order_execution_guards 自动与人工共用、同订单 resend/initial/confirm 互斥、guard 只在确定终态释放（未知仅显式人工解决，不用超时租约）；管理员连点经 command_receipts 同键单动作；竞争方获 409 并看到现有执行状态，落在 `src/application/delivery/guards.rs`（FR-014、FR-022）
- [x] T070 [US4] 实现历史接管：monitor_since 之前的订单仅展示、零自动补发；`/takeovers` 逐单接管（原因+acknowledge_historical_scope）、显式允许 paid_at 早于 monitor_since 但不放宽其他核验；接入前订单不因"现在待发货"自动接管，落在 `src/application/manual/takeover.rs`（FR-017）
- [x] T071 [US4] 实现待处理事项：issues 含账号/订单/原因/最后尝试时间/由当前事实计算的 allowed_actions；同一未解决原因不重复创建；页面刷新或重开仍存在直到处理或核验终止，落在 `src/application/issues/service.rs`（FR-024）
- [x] T072 [P] [US4] 前端人工处理：订单详情动作区（补发带风险确认与原因、人工确认带证据说明、终止）、unknown 状态引导面板、历史订单接管流程、issues 驱动入口，落在 `frontend/src/features/issues/` 与 `frontend/src/features/orders/`
- [x] T073 [US4] V08/V09 验收测试：漏通知 5 分钟内交付或待处理、晚到通知不重复、接入前订单零自动、paid_at 缺失转人工、分页截断缺口可见；人工补发同键同参返回原结果/同键异参 409/版本冲突 409；人工确认后另选独立平台确认；退款/完成拒绝补发，落在 `tests/integration/test_recovery_manual.rs`

**Checkpoint**: US4 全部场景（US4-1—US4-8）+ V06/V07/V08/V09 通过；崩溃与竞争矩阵有进程级证据

---

## Phase 7: User Story 5 - 查询记录并管理异常（Priority: P2）

**Goal**: 账号隔离的订单查询、受保护详情、待处理与仪表盘、持久化故障行为、备份恢复与逐单核对

**Independent Test**: 预置不同账号、时间和状态的记录，验证查询隔离、详情、人工动作与重启保留，不依赖实时平台连接（spec US5）

### Implementation for User Story 5

- [x] T074 [US5] 实现订单查询 API：`GET /orders` 支持 account_id、platform_order_id、delivery_state、platform_state、付款时间 from/to（缺付款时间可独立筛选）；游标分页；索引保证 10,000 记录 P95 ≤2 秒，落在 `src/transport/orders.rs` 与 `src/adapters/sqlite/queries/orders.rs`（FR-025、SC-006）
- [x] T075 [US5] 实现订单详情 API：OrderDetail（买家/商品/SKU/facts_complete/fact_source、DeliveryDetail 含 attempts 与 timeline、allowed_actions 动态计算）；完整正文只在授权详情返回；DTO 状态映射按 http-api.md §4（pending_verification→awaiting_verification 等，禁止直接序列化数据库模型）；路径账号不匹配 404 不泄露他账号存在性，落在 `src/transport/orders.rs`（FR-025、FR-026）
- [x] T076 [P] [US5] 实现 issues 与 dashboard API：`GET /issues`（账号/状态/类别筛选+分页）；`GET /dashboard`（账号摘要、open_issue_count、active_job_count、persistence healthy/blocked、stopping、restore 摘要）；打开中页面异常提示 ≤10 秒出现，落在 `src/transport/issues.rs` 与 `src/transport/dashboard.rs`
- [x] T077 [US5] 实现隐私审计加固：普通列表/日志/诊断导出不含凭证与完整正文；前端不将正文写入 localStorage；以测试断言加固，落在 `src/transport/`、`frontend/src/shared/` 与 `tests/contract/test_privacy.rs`（FR-026）
- [x] T078 [US5] 实现持久化故障行为：DB 不可保存 → 503 persistence_unavailable、账号 storage_error、阻止一切新发送并显示恢复指引；与日志写失败分别提示；绝不以清空数据继续，落在 `src/application/` 与 `src/runtime/`（FR-027、US5-5）
- [x] T079 [US5] 实现 backup CLI：停机独占（服务运行退出码 3）；SQLite online backup 一致快照；加密归档（随机归档密钥 DPAPI 包装、AEAD 绑定 manifest、校验和）；冻结密钥轮换；临时文件+原子发布、目标已存在拒绝；输出路径/sha256/快照时间/同机同用户限制，落在 `src/adapters/sqlite/backup.rs` 与 `src/transport/cli/backup.rs`（FR-028）
- [x] T080 [US5] 实现 restore CLI：仅同机同用户停机恢复；暂存目录解密/校验/quick_check/必要迁移；原目录先受保护保留副本；事务内写 restore_epoch+全部账号暂停+会话撤销+隔离（快照未完任务与快照-恢复区间的可能发送订单）；崩溃安全的切换标记；任何失败保留原数据；拒绝未来 schema 降级，落在 `src/adapters/sqlite/restore.rs` 与 `src/transport/cli/restore.rs`
- [x] T081 [US5] 实现恢复核对 API：`GET /restore` 与 `/restore/reviews`（筛选分页）；`POST decisions`（received/approved_not_sent/terminated+原因+证据+风险确认，幂等）；同一事务原子更新 review+delivery+guard+人工审计；manual_only 作为 execution_policy 持久化（不是 UI 隐藏）；总开关不能批量解除、覆盖缺口保持隔离，落在 `src/application/restore/reviews.rs` 与 `src/transport/restore.rs`
- [x] T082 [P] [US5] 实现 init-admin CLI：停机独占锁；首次无管理员隐藏输入两次 ≥12 字符；已有管理员未带 `--reset` 报前提错误；`--reset` 仅重哈希+废会话+审计+全账号暂停（不解密展示凭证、不改数据密钥、不清订单、不解除隔离）；提交前 Ctrl+C 无修改，落在 `src/transport/cli/init_admin.rs`
- [x] T083 [US5] 前端查询与待处理页：订单页（筛选/分页/详情入口）、订单详情（时间线、受保护正文揭示、可用动作）、issues 页（可用动作）、dashboard 页（账号/待处理/任务/持久化横幅、3 秒轮询），落在 `frontend/src/features/orders/`、`frontend/src/features/issues/` 与 `frontend/src/app/`
- [x] T084 [P] [US5] 前端恢复核对页：隔离横幅、逐单核对卡（决定+证据说明+风险确认）、账号重新启用指引、追溯缺口展示，落在 `frontend/src/features/settings/`
- [x] T085 [US5] V10/V11 验收测试：10k 记录查询 P95 ≤2 秒；多账号同名商品/同形订单号隔离；正文仅授权详情可见；隐藏页面重开仍见待处理；备份→发正文不确认平台→恢复的隔离；损坏归档/错误用户/迁移失败保留原数据，落在 `tests/integration/test_query_privacy_backup.rs` 与 `tests/e2e/`

**Checkpoint**: US5 全部场景（US5-1—US5-6）通过；全部 P1/P2 交付，规格功能完成

---

## Phase 8: Polish & Cross-Cutting Concerns（对应 plan 阶段 F）

**Purpose**: 发行、规模验证、实证矩阵与文档（SC-001—009 门禁）

- [x] T086 [P] 实现发行打包 `scripts/release.ps1`：显式白名单目录/文件（禁止递归打包仓库或上游）、要求本次前端构建产物（拒绝陈旧页面）、嵌入静态资源、无开发工具/无参考目录可运行，产出独立发行目录
- [x] T087 [P] 编写用户文档：README 与发行说明（启动、初始化、备份恢复说明、版本兼容提示、支持/不支持能力清单、回环访问边界），落在 `README.md` 与 `docs/`
- [x] T088 [P] 许可与隔离审查：核对是否复用上游实质代码并保留 LICENSE/NOTICE/修改声明（Apache-2.0 义务）；确认 `Ydisks-Xianyu-Helper/` 排除于 Git/构建/打包；检查仓库制品无敏感数据，落在 `docs/` 与 `scripts/`
- [x] T089 规模与性能验证：SC-001（5 分钟配置计时）、SC-002（100 单/3 账号交付正确率与 P95≤10 秒）、SC-006（10k 查询 P95≤2 秒）；记录样本数/分位数/排除原因，落在 `tests/e2e/` 与 `docs/evidence/`
- [x] T090 执行完整验收矩阵：V01—V12 逐项运行并记录命令、版本、样本与结果；产出 FR-001—031 与 SC 覆盖对照报告，落在 `docs/evidence/acceptance.md`
- [ ] T091 授权实单矩阵（SC-009）：在明确授权的真实账号/交易范围内核验扫码授权、普通付款交付、完整规格、确认开关、人工补发，各至少一例并记录版本与日期；未获授权则版本标记"待验证"，不得以模拟替代，落在 `docs/evidence/`
- [x] T092 资源预算记录：固定环境记录服务与浏览器进程树的空闲/峰值内存、CPU、冷启动与页面加载（分别计量，不只测 Rust 主进程），写入 `docs/evidence/` 并回填预算

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup（Phase 1）**: 无依赖，立即开始；T003 依赖实验是工具链锁定的门禁
- **Foundational（Phase 2）**: 依赖 Phase 1；**阻塞全部用户故事**
- **User Stories（Phase 3—7）**: 均依赖 Phase 2 完成
- **Polish（Phase 8）**: 依赖全部所需故事完成（T091 实单矩阵可与其他任务并行推进授权）

### User Story Dependencies

- **US1（P1）**: 基础就绪即可开始；以假平台/夹具独立验收，不依赖真实协议。交付内核（事件管道、状态机、执行器、证明、T1/T2/T3）是后续故事的共享底座
- **US2（P1）**: 应用层依赖 US1 的执行器与事件管道（T032、T025）；真实协议实现（T041—T046）可与 US3 并行
- **US3（P1）**: 规则 CRUD/内容版本不依赖 US2；资格边界集成（T060）依赖 US1 的 eligibility
- **US4（P1）**: 依赖 US1 状态机与 T1/T2/T3、US2 账号运行状态；人工动作与 issues 是 US5 查询的数据源
- **US5（P2）**: 查询/详情依赖 US1—US4 的数据；备份/恢复只依赖 Phase 2，可提前并行

### Within Each User Story

- 纯 domain 模型先于应用服务，服务先于端点，核心实现先于集成
- 契约测试（tests/contract/）在对应 transport 任务内同步建立
- 每故事以 Checkpoint 验收测试收尾，不实现下个优先级故事前先验证当前故事

### Parallel Opportunities

- Phase 1：T002—T006 全部可并行（不同文件）
- Phase 2：T013、T020 与其他任务并行
- Phase 3：T024/T027/T028（纯接口与 domain）可先行并行；T036/T037/T038 测试文件相互并行
- 故事间：US3 规则后端 ‖ US2 扫码协议；US5 备份/恢复 CLI ‖ US4 恢复逻辑；各故事前端页面（T040/T052/T061/T072/T084）互不冲突
- 不同故事可由不同开发者并行（遵守上述故事依赖）

---

## Parallel Example: User Story 1

```text
# 接口与 domain 模型并行：
Task: "T024 平台消费者接口 src/application/ports/platform.rs"
Task: "T027 OrderSnapshot 完备性 src/domain/orders/snapshot.rs"
Task: "T028 交付状态机 src/domain/delivery/"

# 验收测试并行（实现 T030—T035 完成后）：
Task: "T036 去重硬化 tests/integration/test_delivery_dedup.rs"
Task: "T037 数量与SKU tests/integration/test_delivery_sku_quantity.rs"
Task: "T038 非法事实 tests/integration/test_delivery_invalid_facts.rs"
```

---

## Implementation Strategy

### MVP First（Phase 1 + 2 + US1）

1. 完成 Phase 1 工具链锁定与假平台夹具
2. 完成 Phase 2 公共基础设施（阻塞项）
3. 完成 Phase 3 US1 交付内核
4. **停下验证**：在 mock/dev-fixtures 环境跑 US1 验收场景与 V04/V05
5. 得到可演示的确定性交付管道（真实协议尚未接入，不宣称实账号支持）

### Incremental Delivery

1. Setup + Foundational → 基础就绪
2. +US1 → 假平台 MVP（演示/验证）
3. +US2 → 真实账号接入与控制
4. +US3 → 卖家自助配置（SC-001 五分钟路径）
5. +US4 → 异常恢复与人工处理（可承担真实交易）
6. +US5 → 查询、隐私与备份恢复（日常运维闭环）
7. Polish → 独立发行包 + 实证矩阵（SC-009 前版本只能标"待验证"）

### Parallel Team Strategy

多开发者时：共同完成 Phase 1—2 → 开发者 A：US2 协议与账号；开发者 B：US3 规则与前端；随后 A：US4 恢复、B：US5 查询与备份；共同收尾 Phase 8。

---

## Notes

- [P] 任务 = 不同文件且无未完成依赖；仍须遵守故事内"模型→服务→端点"顺序
- 所有外部动作任务（发送/确认/补发/备份）必须先持久化再执行，未知结果永不自动重放（宪章 I）
- 平台适配器不写业务表、不决定规则（宪章 II）；Rust 改动过 fmt/clippy/test，前端过 typecheck/test/build（宪章门禁）
- 每任务或逻辑组提交一次；Checkpoint 处停下来独立验证故事
- 实账号验证（T091）独立授权与标识，模拟通过不得表述为实单通过
- 避免：模糊任务、同文件冲突、破坏故事独立性的跨故事依赖

---

## Phase 9: Convergence

**Purpose**: 2026-09-23 收敛核查发现：应用层/HTTP/DB/测试真实存在，但 serve 二进制从未构造交付管道服务（T032/T047/T053 勾选失实），真实协议路径（T041/T044/T045，未勾选）与浏览器验证（T050/T051，未勾选）仍由未勾选任务跟踪、不在此重复

- [x] T093 实现 serve 运行时编排并接线交付管道：在 `src/runtime/supervisor.rs` 建立账号运行时（事件通道→交付服务→恢复扫描循环，mock 与 live 共用构造），在 `src/main.rs` serve 启动序列（DB 线程之后、绑 HTTP 之前）构造 DeliveryService/ItemSyncService/CompensationService/TraceScanService/ManualService 并注入 AppState，manual_handle 按 profile 提供真实句柄；管理页面关闭后核心交付持续运行 per T019/T032/T047/T053、宪章 II（missing, CRITICAL）
- [ ] T094 为真实协议路径提供确定性夹具：T041/T044/T045 完成后，以脱敏合成响应（generate.do/query.do/token 端点/reg/sync 帧）扩展 `tests/fixtures/`，使 live 协议代码可在无真实账号下测试 per 宪章 IV、quickstart V01/V02（missing, HIGH）
