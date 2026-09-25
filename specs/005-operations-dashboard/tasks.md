---
description: "Task list for 005-operations-dashboard implementation"
---

# Tasks: 运营概览统计首页(005-operations-dashboard)

**Input**: Design documents from `/specs/005-operations-dashboard/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/stats-api.md, quickstart.md

**Tests**: Included——契约 §测试要求 + quickstart S1~S7;测试先行(先写先败)。

**Organization**: 按用户故事分阶段;研究决策 R1~R8 已内化;澄清 Q1(退款不回冲)/Q2(自然日对齐)/Q3(第四卡=待人工处理)已内化。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行(不同文件、无未完成依赖)
- **[Story]**: US1(范围筛选+四卡)/US2(趋势图)/US3(自定义范围)

---

## Phase 1: Setup (Shared Infrastructure)

- [x] T001 基线确认:当前分支 `005-operations-dashboard`(或工作树)上 `cargo test`、`cd frontend && npm run typecheck && npm run test` 全绿,记录起点快照;确认无未迁移 schema 依赖(本特性零迁移、零新依赖,per plan Technical Context)

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠️ CRITICAL**: 统计领域层是三个用户故事的共同前置;未完成不得开始任何故事

- [x] T002 新建 src/domain/stats.rs(纯值+纯函数,零 IO):`StatsRange{from_ms,to_ms,granularity}` 与 `Range::new` 校验(**`to_ms > from_ms`、跨度 ≤92 天**,违反→`RangeError`,per data-model §StatsRange);`Granularity::{Hourly,Daily}` 推导(**`to-from ≤ 48h` → Hourly,否则 Daily**,spec FR-007);`bucket_ranges(&StatsRange) -> Vec<(i64,i64)>`(本地自然小时/自然日 00:00 边界,空桶保留,末桶不足整段成桶,桶上限 92/49);营收口径纯函数(仅 `currency=="CNY"` 且金额非空计入合计、订单数只按付款事实不过滤状态——**退款/关闭不回冲,澄清 Q1**、`change_percent` 四舍五入取整且 `prev==0→None`);附单测覆盖 data-model §验证规则 1-5 全部用例
- [x] T003 [P] 扩展 src/domain/time_util.rs:`local_day_start_ms(now_ms, days_back)`(本地自然日 00:00,可回溯 N 天)、`local_hour_start_ms(ms)`(本地自然小时);沿用 `local_midnight_ms` 的 DST 解析失败回退策略(回退入参,宁多算不抛错);单测:跨日界/跨月/跨年边界各一

**Checkpoint**: 统计领域口径确定且可单测——应用层/传输层/前端可并行展开

---

## Phase 3: User Story 1 - 时间范围筛选与四张统计卡 (Priority: P1) ⭐ MVP

**Goal**: 概览首页呈现标题区+范围筛选(五预设,默认 7天内)+四张统计卡(区间营收+对比徽标/活跃账号/订单数/待人工处理),切换原地更新,口径与订单中心逐单一致

**Independent Test**: quickstart S1(空库空态)+ S2(口径核对)+ S3(范围切换/刷新)

### Tests for User Story 1(先写,确认 FAIL)

- [x] T004 [P] [US1] 新建 tests/contract/stats_http.rs(参照 tests/contract/dashboard_http.rs 夹具风格):①未登录 401;②参数三态 400(from/to 非数字、`to<=from`、跨度>92 天);③口径三例(夹具插单:CNY 计入、非 CNY 排除合计但计入订单数、金额缺失排除合计但计入订单数、**退款/关闭状态订单仍计入合计与订单数**);④账号快照 online/total;⑤开放事项计数;⑥**只读不变式**(查询前后 orders/issues/accounts 行集与状态零变化,FR-012);⑦空区间:合计 0、`change_percent=null`、(趋势断言留 T014);契约细节 per contracts/stats-api.md

### Implementation for User Story 1

- [x] T005 [US1] 新建 src/adapters/sqlite/repos/stats.rs(只读,复用 `idx_orders_paid_at`):`raw_paid_orders(conn,&StatsRange) -> Vec<(i64,Option<i64>,String)>`(区间原料 `paid_at∈[from,to) AND paid_at IS NOT NULL`,研究 R3:SQL 只过滤不分桶)、`account_activity(conn) -> (online,total)`(与账号页 `AccountStatus::parse` 同源)、`open_issue_count(conn)`;单测(内存库+迁移):边界含/不含、退款单不过滤
- [x] T006 [US1] 新建 src/application/stats.rs:`StatsService{db:DbThread}` + `overview(&self, range) -> Result<StatsOverview>`——db.call 内组装:revenue(区间+前一等长区间 `[from-span,from)`,对比百分比调 T002 纯函数)、order_count、accounts、pending_issues、trend(T002 `bucket_ranges` 填桶,空桶补零,Σ桶=合计);**FR-012 只读:不触碰任何写路径/交付/值守**;单测:Σ桶一致、对比区间正确、空区间形状(per data-model §TrendPoint 不变式)
- [x] T007 [US1] 新建 src/transport/stats_api.rs + 挂载:`GET /api/v1/stats/overview`(薄 handler,无 SQL——研究 R6:`require_admin_public(&state,&headers,false)`;from/to 解析 epoch ms,非法 400;调用 StatsService;200/400/500 按契约错误信封);src/transport/routes.rs 注册一行、src/transport/mod.rs 导出、src/application/mod.rs 注册 stats;**既有 /api/v1/dashboard 零改动**(研究 R1);T004 契约测试转绿
- [x] T008 [US1] frontend/src/shared/contracts.ts:`StatsOverview` 类型 + `parseStatsOverview(v)` 容错解析(`change_percent:null` 合法、trend 非数组降级空数组不抛错,契约 §前端消费契约);新建 frontend/src/shared/contracts.test.ts:正常/null/缺字段/类型错四态
- [x] T009 [US1] frontend/src/features/overview/model.ts 扩展:`resolvePresetRange(preset,nowMs)`(**自然日对齐,澄清 Q2**:今天=当日 00:00 起;昨天=前一自然日;三天内/7天内/一个月内=含今天共 3/7/30 个自然日)、四卡视图模型派生(徽标 null→隐藏);model.test.ts 先败后绿:五预设边界含跨月/跨年、徽标 null 隐藏、解析降级
- [x] T010 [US1] frontend/src/features/overview/page.ts 改版①:标题区("运营概览"+副文案+系统运行状态指示,恢复隔离/停止横幅沿用既有 deriveStats 置顶);范围 chips 五预设(**默认 7天内**,选中态);四张新统计卡(图标+数值+对比徽标+下钻:账号管理/订单中心/待处理工作台;**第四卡=待人工处理,澄清 Q3**;pending>0 警示样式);**移除旧四卡**(旧"今日订单/已交付"口径与区间卡并存会矛盾,研究 R8);30s 可见轮询+手动刷新按当前范围(FR-013);切范围原地更新不整页刷新(FR-015);frontend/src/styles/components.css 增补 chips/图标卡/徽标样式类
- [x] T011 [US1] 走查 quickstart S1+S2(卡片部分)+S3,结果记 docs/evidence(沿用 004 证据文档惯例)

**Checkpoint**: MVP——四卡+范围切换+口径端到端正确;趋势区暂为占位/空态

---

## Phase 4: User Story 2 - 营收趋势分析图表 (Priority: P2)

**Goal**: 趋势区按所选区间绘制销售额走势(≤48h 小时粒度、其余日粒度),空桶补零,空态明确,Σ桶=卡值

**Independent Test**: quickstart S2 趋势部分 + S3 图表联动

### Tests for User Story 2(先写,确认 FAIL)

- [x] T012 [P] [US2] tests/contract/stats_http.rs 增趋势断言:hourly(今天/昨天)与 daily(7天内)两档分桶边界正确、空桶补零、`Σtrend[].minor_units == revenue.minor_units` 且 `Σtrend[].order_count == revenue.order_count`(契约 §不变式)
- [x] T013 [P] [US2] 新建 frontend/src/ui/chart.ts:`renderTrendChart(points) -> HTMLElement`(手写 SVG 柱状,研究 R5:小时/日轴标签、空桶零高柱、每柱 hover title=本地时间+金额 ¥x.xx+笔数;纯 DOM 无依赖);frontend/src/ui/chart.test.ts:桶数=柱节点数、零桶渲染、title 文案、空数组不抛错

### Implementation for User Story 2

- [x] T014 [US2] frontend/src/features/overview/page.ts 改版②:趋势区块(标题"营收趋势分析"+副文案含粒度说明;接入 T013 图表;区间无订单→既有 `empty-state` 空态"暂无营收数据/所选时间范围内暂无订单记录";加载/错误占位不影响统计卡,spec US2 验收 4);T012 转绿
- [x] T015 [US2] 走查 quickstart S2 趋势部分+S3(图表联动),记证据

**Checkpoint**: 趋势可用且与卡值严格一致(SC-002)

---

## Phase 5: User Story 3 - 自定义时间范围 (Priority: P3)

**Goal**: "自定义"chip 展开起止日期输入;同日允许、倒置/超 92 天拦截不发请求;应用后刷新保持

**Independent Test**: quickstart S4 全部场景

### Tests for User Story 3(先写,确认 FAIL)

- [x] T016 [P] [US3] frontend/src/features/overview/model.test.ts 增 `resolveCustomRange(startDay,endDay,nowMs)` 用例:同一天(该自然日 00:00~次日 00:00)、正常跨两周、倒置 Err、恰 92 天通过、93 天 Err、跨月末/年末正确

### Implementation for User Story 3

- [x] T017 [US3] frontend/src/features/overview/model.ts 增 `resolveCustomRange`(本地时区日界解析,校验复用 T002 的 92 天上限规则);page.ts 增自定义交互:"自定义"chip→展开两个 date input+应用/取消;无效→行内提示**不发请求不改变当前展示**(FR-009);应用后轮询/手动刷新保持自定义区间(FR-013);Vitest:倒置/超限拦截不发请求(spec US3 验收 2-3)
- [x] T018 [US3] 走查 quickstart S4,记证据

**Checkpoint**: 三个故事全部独立可验收

---

## Phase 6: Polish & Cross-Cutting Concerns

- [x] T019 SC-003 性能抽查:临时夹具批量插入 ≥1 万笔订单(91 天内随机分布),"一个月内"查询 <3s;记录测量值(数据量/耗时)入证据文档(宪章 III 以测量决定优化);若超阈值,按研究 R3 预留方案(SQL 内聚合)升级并在 tasks 记录
- [x] T020 [P] 全量回归 + 剩余走查:`cargo test` / `tsc --noEmit` / `vitest` 全绿;quickstart S5(保留区块与下钻)、S6(权限 401/400 三态/只读)、S7(停止/恢复隔离横幅置顶)逐项确认
- [x] T021 收尾:spec FR-001~016 逐条对照勾验;确认 002 既有契约(/api/v1/dashboard)零破坏;quickstart 完成判据全满足

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup(T001)**: 无依赖,立即开始
- **Foundational(T002/T003)**: 依赖 T001;**阻塞全部用户故事**
- **US1(T004~T011)**: 依赖 Foundational;T004(测试)先于 T005~T007(实现);前端 T008~T010 可与后端 T005~T007 并行(契约即边界)
- **US2(T012~T015)/US3(T016~T018)**: 均依赖 US1 完成(页面脚手架与契约测试文件);US2 与 US3 相互独立可并行
- **Polish(T019~T021)**: 依赖全部故事完成

### Within Each Story

- 测试先写先败(T004/T012+T013/T016)→ 实现 → 契约/单测转绿 → 走查
- 后端链条:repos(T005)→ service(T006)→ transport(T007)
- 前端链条:contracts(T008)→ model(T009/T017)→ page(T010/T014/T017)

### Parallel Opportunities

- T002 ∥ T003(不同文件)
- T004 ∥ T005(测试文件 vs 仓储实现,先写后实现仍可并行起草)
- US1 内:后端(T005~T007)∥ 前端(T008~T010)
- US2 ∥ US3(T012~T015 ∥ T016~T018)
- T019 ∥ T020(不同验证面)

---

## Parallel Example: US1 后端与前端并行

```text
# 同一契约(contracts/stats-api.md)为边界,两条线互不阻塞:
后端线: T005(repos) → T006(service) → T007(transport,契约测试转绿)
前端线: T008(contracts.ts) → T009(model) → T010(page 改版①)
```

---

## Implementation Strategy

- **MVP First**: T001~T011(US1)完成即交付核心价值——卖家按范围看到营收/账号/订单/待处理;STOP 验证 S1~S3
- **Incremental**: +US2 趋势图(可视化核心)→ +US3 自定义对账 → Polish 性能与回归
- **口径单一来源**: 全部数值口径在 src/domain/stats.rs 纯函数;前端不做口径计算,只做解析与呈现(违反即 bug)
- Commit 建议:每 Task 或同故事任务组一 commit,信息带 T 编号

## Notes

- 本特性**零迁移、零新增运行时依赖**(plan Technical Context);发现需要迁移/新依赖即偏离计划,先回查 plan/research
- `dashboard_api.rs` 既有内联 SQL 为技术债:本特性**不改动不扩大**(研究 R1/R8);新代码以分层为范式
- 验收口径争议一律回溯 spec FR-010 + data-model §验证规则,不以界面直觉为准

---

## Phase 7: Convergence

> 来源:$speckit-analyze + $speckit-converge(2026-09-25)。代码总体已实现并走查通过;以下为对照 spec/plan/宪法逐项复核后的剩余缺口,按 CRITICAL/HIGH 优先排序。

- [x] T022 统计加载过期响应守卫:frontend/src/features/overview/page.ts `loadStats` 增递增序号守卫(响应返回时序号非最新则丢弃,以最后一次所选范围为准),复用 asyncBlock 同款模式;新增 Vitest 用例:两次范围切换、旧请求后返回不覆盖新范围数据 per spec Edge Cases「自动刷新落在范围切换的瞬间」+ FR-013 + Constitution IV「过期响应」(partial)
- [x] T023 宪法门禁修复与记录:修复 `cargo fmt --check` 不通过项(repos/stats.rs 等格式 diff)与 `cargo clippy` 2 条警告("items after a test module"、"very complex type");plan.md Testing 节补记门禁命令(fmt/clippy/test);结果记入 docs/evidence/005-operations-dashboard.md per plan「Testing」+ Constitution 交付门禁(partial)
- [x] T024 plan Constitution Check 补齐:plan.md 检查表增原则 IV(关键行为可验证/契约运行时校验/过期响应)与 V(回环绑定/无敏感数据出统计响应)逐项结论 + 门禁六问逐项回答(当前实际工作已满足,缺的是计划文档评估) per Constitution「开发流程与交付门禁」(missing)
- [x] T025 未付款订单契约用例:tests/contract/stats_http.rs 增 `paid_at IS NULL`(未付款)订单不计入营收与订单数的显式断言 per Constitution IV 关键测试清单 + FR-010(missing)
- [x] T026 对比徽标样本补足:src/domain/stats.rs change_percent 单测增补取整边界样本(.5 进位、临界 ±1、大数),总量 ≥10 组 per SC-005(partial)
- [x] T027 统计查询失败占位:frontend/src/features/overview/page.ts loadStats 失败时趋势区显示错误占位(重试按钮/提示文案)、统计卡保留上次值不闪 0;新增 Vitest 用例:fetch reject → 占位可见 per US2/AC4 + FR-008(partial)
- [x] T028 SC-001 计时断言:走查脚本(或 Vitest)对"切换范围→四卡/趋势更新完成"计时,断言 <2s;或在 docs/evidence/005-operations-dashboard.md 记录目测口径与豁免理由 per SC-001(partial)
- [x] T029 文档一致性修订:①plan.md 项目树 `shared/contracts/stats.test.ts` → `shared/contracts.test.ts`;②plan 或证据文档注明 T019 以"一个月内"更宽窗口覆盖 SC-003 字面"7天内"场景;③spec FR-015 "无明显布局跳变"补可检验表述(数值节点原地复用,容器高度仅随内容一次变化);④spec 补一句"统计不可用时四卡显示占位" per F4/B1/C3 分析结论(partial)
