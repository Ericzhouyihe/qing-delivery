---
description: "Task list for 003-verify-automation implementation"
---

# Tasks: 安全验证自动化(003-verify-automation)

**Input**: Design documents from `/specs/003-verify-automation/`

**Prerequisites**: plan.md(required), spec.md(required), research.md, data-model.md, contracts/verification-api.md, quickstart.md

**Tests**: Included——宪章 IV 关键测试要求;分层:确定性(fake driver,无浏览器)/浏览器集成(`QING_BROWSER_TESTS=1` 门控)/实账号(R11,授权项)。

**Organization**: 按用户故事分阶段(US1 自动处置为 MVP);研究决策 D1-D8 已内化到任务描述。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行(不同文件、无未完成依赖)
- **[Story]**: 所属用户故事(US1-US5)
- 所有任务含确切文件路径

## Path Conventions

后端 `src/…`(application/adapters/runtime/transport);迁移 `migrations/`;测试 `tests/`;前端 `frontend/src/…`

---

## Phase 1: Setup (Shared Infrastructure)

- [x] T001 新建 migrations/0002_verification.sql:`verification_attempts` 审计表(id TEXT PK、account_id FK、trigger_source TEXT NOT NULL、trigger_reason TEXT NOT NULL、verification_url TEXT、started_at INTEGER NOT NULL、finished_at INTEGER、outcome TEXT NOT NULL('in_progress')、duration_ms INTEGER、credential_updated INTEGER NOT NULL DEFAULT 0、failure_reason TEXT)——**只插入与回填终态,不删改历史**;`issues` 增列 `metadata TEXT`(可空 json,旧行 NULL);在 src/adapters/sqlite/repos/verification.rs 实现 insert/finish/list_recent 并附单测(只增不变量、metadata 回读) per data-model §1
- [x] T002 [P] 新建 src/application/verification/mod.rs:定义 `VerificationSignal{account_id, source: mtop|ws|qr|manual, url: Option<String>, raw: String}`、`SolveOutcome{Solved{cookie_jar}|Failed{reason, stage}}`、对象安全 trait `VerificationDriver`(fn solve_boxed(url, cookie_seed) → Pin<Box<dyn Future<Output=SolveOutcome>+Send>>;沿 QrDriver/ItemSyncDriver 模式) per 研究 D3、contracts §5

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠ CRITICAL**: 服务核心/闸门/信号接线完成前不开始浏览器侧故事

- [x] T003 新建 src/application/verification/service.rs:验证会话状态机 `detected→browser_open→solving→updating_credential→succeeded|failed_manual`(data-model §2 图);约束内建——**重试上限 2 次、总超时 120s(单调时钟)、降级后冷却 ≥5 分钟、单账号同时至多 1 个活跃会话(重复信号去重合并)**;driver 以 trait 注入,测试用 fake driver(src/application/verification/service.rs + service_tests);附状态机全迁移/超时/冷却/去重单测(确定性,无浏览器)
- [x] T004 service 集成持久层:每次尝试写 verification_attempts 一行(开始 insert、终态 finish 回填 duration/outcome/failure_reason);failed_manual 时生成 issues(category='security_verification',reason=失败原因,allowed_actions='["open_verification","resolve"]',metadata={verification_url,attempt_ids});failed_manual 存在时每 **60s** 轻量探测凭证恢复,恢复→自动关闭事项+审计补记(FR-005/FR-010);单测覆盖:审计行完整性、事项生成与自动关闭、探测不产生交付副作用
- [x] T005 新建 src/application/verification/gate.rs:per-account 内存交付闸门(会话活跃=闸门关闭);在 src/runtime/supervisor.rs 的 handoff/交付触发前检查,命中则延迟入队等待,会话终态即释放;**不触碰 account_control 持久化开关**(研究 D5);单测:验证期间付款事件延迟、验证后按原状态机继续、其他账号不受影响(FR-003/FR-013)
- [x] T006 信号源统一:① src/adapters/xianyu/mtop/client.rs `MtopError::Verification` 载荷结构化为 `{url: Option<String>, ret: String}`——classify_ret 抛错前从响应 JSON 提取验证 URL(gotoUrl/url 类字段;字段名按实账号响应校准,提取失败 url=None 仍触发),同步更新 adapter 内全部调用方(内部破坏性,HTTP 契约不变);② src/adapters/xianyu/events.rs 增 `MessageKind::VerificationRequired{url: Option<String>}`(入站文本含 punish/滑块 关键词,集合收窄防误报 SC-304);③ supervisor/ws 泵与 mtop 错误路径把信号送入 VerificationService;单测:三源→统一事件、普通消息不触发(误报为零)

**Checkpoint**: 确定性服务核心就绪(quickstart §构建与确定性测试全绿)

---

## Phase 3: User Story 1 - 风控自动处置 (Priority: P1) ⭐ MVP

**Goal**: 信号→浏览器滑块→凭证回写→账号 5 分钟内自动恢复;处置期间该账号暂停交付

**Independent Test**: quickstart §手动走查 1(手动触发走同一管道)+ 确定性状态机测试;实账号项归 R11

- [x] T007 [P] [US1] 新建 src/adapters/browser/discover.rs:按序探测本机 Chrome/Edge(常见安装路径+注册表),返回可执行路径或带指引的不可用原因(文案:"未检测到 Chrome/Edge,请安装后重试");单测(注入路径表)
- [x] T008 [US1] 新建 src/adapters/browser/manager.rs(基础):按需创建/同账号复用 chromiumoxide 实例;user-data-dir=`<data_dir>/browser-profile`,启动前目录锁检测——被占/损坏→如实报错转 failed_manual,**不删目录**(边界条款);默认有头(headless 为 BrowserInstancePolicy 配置项)
- [x] T009 [US1] 新建 src/adapters/browser/cdp.rs:实现 VerificationDriver——注入 cookie 种子→打开 verification_url→调 slider→成功读取完整 Cookie jar 返回 Solved{cookie_jar};会话结束关闭页面(实例留 manager 复用)
- [x] T010 [P] [US1] 新建 src/adapters/browser/slider.rs:Runtime.evaluate 定位滑块(初始选择器覆盖 baxia/nc_ 系节点,标注"实账号校准项");Input.dispatchMouseEvent 人类轨迹拖动(贝塞尔+速度曲线+微抖动);成功判定=punish 退出/回调;支持 `QING_FORCE_SLIDER_FAIL=1` 故障注入(走查降级用);定位/轨迹生成纯函数单测
- [x] T011 [US1] src/main.rs build_runtime 增挂:live 构造 BrowserManager+cdp Driver+VerificationService 注入 AppState/runtime(live=Some,mock=None 端点如实报不支持);服务收到 Solved → credentials::save(现加密路径)→ accounts.credential_epoch+1 → supervisor 该账号会话热更新(参照启动恢复路径);**保存失败保留旧凭证不 bump epoch**,审计记"凭证更新失败"并转人工(FR-014/研究 D6);单测:回写成功路径与失败保留旧值(fake credentials)
- [x] T012 [US1] src/transport/routes.rs capabilities 增 `browser: {available, engine: "system-chromium", reason}`(读 discover 结果缓存);前端 shared/contracts.ts 增类型;界面按此如实启停(FR-012)
- [x] T013 [US1] US1 确定性测试收口:fake driver 覆盖 detected→…→succeeded 全链(含 2 次重试内成功、超时、driver Failed 各 stage)、闸门联动、凭证回写;`cargo test` 全绿(基线 ≥211 不回退)
- [ ] T014 [US1] 走查 quickstart §手动走查 1:手动触发→分步进度→审计(方式=手动);结果记入 docs/evidence/verification-acceptance.md(新建)

**Checkpoint**: MVP——自动处置链在确定性层与手动走查可验证

---

## Phase 4: User Story 2 - 失败降级转人工 (Priority: P1)

**Goal**: 所有失败路径 100% 生成含一键验证链接的待处理事项;人工完成后 60 秒自动恢复

**Independent Test**: quickstart §手动走查 2/4(故障注入、无浏览器)

- [ ] T015 [US2] 降级端到端:重试耗尽/超时→issues(security_verification, metadata.verification_url)完整生成;**降级后 5 分钟内不再次自动拉起**(服务冷却时间戳已有,补集成断言);每次尝试独立审计行(US2-3 无信息丢失);`QING_FORCE_SLIDER_FAIL=1` 与 `QING_NO_BROWSER=1` 两条注入路径接入测试(tests/ 内确定性用例)
- [x] T016 [US2] 前端 frontend/src/features/issues/page.ts:新增"安全验证"分组(KIND_LABELS/操作映射);`open_verification`=window.open(metadata.verification_url);`resolve`=触发 POST /accounts/{id}/verification 轻量会话做凭证探测(探测通过→事项由服务自动关闭,契约 §4);账号卡"待人工验证"徽章(frontend/src/features/accounts/page.ts);Vitest:分组渲染、链接打开、resolve 调用
- [x] T017 [US2] 走查 quickstart §手动走查 2/3/4:注入失败→转人工→人工完成→60s 自动关闭;无浏览器→明确指引不伪称;记入 docs/evidence/verification-acceptance.md

**Checkpoint**: 最坏情况=点一下链接

---

## Phase 5: User Story 3 - 浏览器生命周期管理 (Priority: P2)

**Goal**: 队列/回收/退出清理/崩溃重建;残留进程为 0

**Independent Test**: quickstart §夹具滑块页集成 browser_lifecycle + 进程卫生走查

- [x] T018 [US3] src/adapters/browser/manager.rs 增:请求队列 mpsc(**并发上限 1**,超出排队且队列超时 60s→直接 failed_manual,边界:风控风暴);空闲 **5 分钟**自动关闭实例;BrowserInstancePolicy 集中参数
- [x] T019 [US3] 退出清理挂 runtime_stop 收尾链(服务停止等待在途处置有上限后全量关闭);实例崩溃→按会话重试策略重建或降级;单测:队列超时、空闲回收(虚拟时钟)、清理注册
- [x] T020 [US3] 新建 tests/fixtures/slider-page.html(自建滑块页:滑块元素+完成回调)+ tests/browser_lifecycle.rs(`QING_BROWSER_TESTS=1` 门控,无浏览器明确 SKIP):空闲回收、退出零残留、并发排队;走查进程卫生(Get-Process 无残留)记入证据文档(SC-303)
- [x] T021 [US3] 前端 frontend/src/features/settings/page.ts 增浏览器策略卡(available/engine/并发与回收参数,只读呈现);Vitest

**Checkpoint**: 浏览器资源受治理

---

## Phase 6: User Story 4 - 手动触发验证 (Priority: P2)

**Goal**: 账号页主动发起,同一管道,后台完成不依赖页面

**Independent Test**: quickstart §手动走查 1 扩展(关闭页面后台完成)

- [x] T022 [US4] src/transport/accounts_api.rs:POST /accounts/{id}/verification 升级为真实触发(202+session;活跃会话冲突→409 返回既有会话;mock/无驱动→unsupported_capability 如实文案)——替换现有占位实现;GET /accounts/{id}/verification 新增(active 会话+recent_attempts,契约 §2);routes 挂载
- [x] T023 [US4] 新建 tests/contract/verification_http.rs:新端点鉴权(匿名 401/CSRF)、状态枚举、conflict 语义、capabilities browser 块、issues metadata 形状(契约 §7)
- [x] T024 [US4] 前端 frontend/src/features/accounts/page.ts:"查看验证"入口升级为"发起验证"(capabilities 门控;点击→POST→轮询 GET 5s 呈现分步进度 detected/browser_open/solving/updating_credential/succeeded);Vitest:入口门控、进度轮询停止条件
- [x] T025 [US4] 走查:手动触发→关闭管理页→后台完成→重开页面见结果(US4-2);记入证据文档

**Checkpoint**: 主动控制权就绪

---

## Phase 7: User Story 5 - 验证审计与状态呈现 (Priority: P2)

**Goal**: 每次尝试完整留痕;徽章/分组/角标联动

**Independent Test**: quickstart 走查审计字段完整性

- [x] T026 [US5] 前端账号页验证状态徽章(正常/验证中/待人工)与验证历史列表(GET verification recent_attempts 呈现:原因/方式/结果/耗时/凭证更新);frontend/src/features/accounts/page.ts + 徽章映射(frontend/src/ui/badge.ts 增验证态)
- [x] T027 [US5] 待处理"安全验证"分组计数与侧边栏角标联动(002 store 复用);Vitest:计数/角标
- [x] T028 [US5] 走查:一次自动成功+一次失败转人工的审计字段 100% 可查(SC-305);凭证更新失败场景旧凭证完好断言已有单测,走查复确认;记入证据文档

**Checkpoint**: 五个故事全部独立可验收

---

## Phase 8: Polish & Cross-Cutting Concerns

- [ ] T029 新建 tests/browser_integration.rs(`QING_BROWSER_TESTS=1` 门控):夹具滑块页端到端——打开→定位→轨迹拖动→成功判定→Cookie 读取→凭证回写路径(fake credentials);无浏览器环境输出 SKIP 说明不算失败
- [ ] T030 全量门禁:`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`(基线 ≥211 不回退,SC-306);前端 `npm run typecheck && npm run test && npm run build`;交付安全不变量相关测试零修改通过
- [ ] T031 docs/evidence/real-account-matrix.md 增 R11(实账号自动处置:触发信号片段/URL 字段名/滑块选择器/成功判定依据,以实账号观察回填);未授权则如实标"待验证";001 tasks.md 关闭 T050/T051(注明由 003 承接)
- [ ] T032 文档收口:README 安全模型/常见问题补"安全验证自动化"说明(含无浏览器指引);docs/evidence/verification-acceptance.md 汇总 SC-301~307 对照(实账号项如实区分已验/待验证);48h 常驻误报观察(SC-304)列入待办记录

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 无依赖;T001/T002 可并行
- **Foundational (Phase 2)**: 依赖 T001/T002;T003→T004 顺序,T005/T006 可与服务并行(不同文件)
- **US1 (Phase 3)**: 依赖 Phase 2 全部;T007/T010 并行,T008→T009 顺序,T011 收口接线
- **US2 (Phase 4)**: 依赖 US1(降级路径需要 driver 存在);前端 T016 仅依赖 T022 的端点形状(可先按契约 mock)
- **US3 (Phase 5)**: 依赖 US1 的 manager 基础;T018/T019 顺序,T020/T021 并行
- **US4 (Phase 6)**: 依赖 Phase 2(服务)+T011(接线);与 US2/US3 可并行
- **US5 (Phase 7)**: 依赖 T022(GET 端点)与 T016(分组);其余并行
- **Polish (Phase 8)**: 依赖全部故事

### Within Each User Story

- 端口/纯函数→实现→接线→确定性测试→走查记录
- 平台 specifics(滑块选择器/URL 字段)改动必须同步 R11 记录,不静默改

### Parallel Opportunities

- T001/T002;T007/T010;T015(后端)/T016(前端);T020/T021;T026/T027
- 前端任务(T016/T021/T024/T026)与对应后端任务按契约并行

---

## Parallel Example: User Story 1

```text
# 三线并行:
Task T007: "discover.rs 系统浏览器探测 + 单测"
Task T010: "slider.rs 定位/轨迹纯函数 + 单测"
Task T008: "manager.rs 基础(创建/复用/锁检测)"
# 汇合于 T009(cdp.rs Driver)→ T011(main.rs 接线+凭证回写)→ T013 测试收口
```

---

## Implementation Strategy

### MVP First (Phase 1+2+US1)

1. 迁移与端口类型 → 服务核心(状态机/审计/闸门/信号)→ 浏览器三件套+接线
2. **STOP 验证**:确定性测试全绿 + 手动触发走查分步进度可见
3. 此时已具备:真实风控到来时的自动处置能力(滑块 specifics 待实账号校准)

### Incremental Delivery

MVP → US2 降级(安全网)→ US3 生命周期(资源)→ US4 手动(控制权)→ US5 审计(可见性)→ Polish(集成测试/实账号 R11/文档)

---

## Notes

- [P] = 不同文件且无未完成依赖
- 平台 specifics 不臆造:滑块选择器/URL 字段名/成功判定以实账号观察为准回填并记 R11(宪章:不虚报)
- 全程本机;不得引入远程验证/打码网络调用(FR-009);verification_url 不写日志明文(记域名/哈希)
- 基线 ≥211(前端 80+Rust 131)任何时刻不得回退(SC-306);浏览器集成测试无浏览器环境必须 SKIP 而非失败
- 走查证据统一 docs/evidence/verification-acceptance.md;实账号项与确定性项严格区分表述
