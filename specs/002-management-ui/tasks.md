---
description: "Task list for 002-management-ui implementation"
---

# Tasks: 管理界面重构(002-management-ui)

**Input**: Design documents from `/specs/002-management-ui/`

**Prerequisites**: plan.md(required), spec.md(required), research.md, data-model.md, contracts/(dashboard-api.md, match-preview-api.md, ui-consumption-map.md), quickstart.md

**Tests**: Included——宪章 IV 要求前端改动至少通过类型检查、行为测试与构建;SC-206 要求 cargo 基线 97 项不得回退。

**Organization**: 按用户故事分阶段(US1 布局框架为 MVP),每阶段独立可验收。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行(不同文件、无未完成依赖)
- **[Story]**: 所属用户故事(US1-US8,映射 spec.md)
- 所有任务含确切文件路径

## Path Conventions

前端:`frontend/src/…`(app/ui/features/shared/styles);后端:`src/…`(adapters/transport);证据:`docs/evidence/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: 目录结构与共享类型就位,构建链保持绿色

- [x] T001 按 plan.md 结构创建 frontend/src/ui/、frontend/src/styles/、frontend/src/features/{overview,accounts,catalog,orders,issues,settings}/ 目录;将 shared/styles.css 迁移为 styles/ 入口并在 frontend/src/main.ts 更新导入;`npm run typecheck && npm run test && npm run build` 全绿
- [x] T002 [P] 在 frontend/src/shared/contracts.ts 增补 dashboard 扩展字段类型:`orders_today?: number; delivered_today?: number`(可选,对应契约"旧版本服务缺失时降级"要求)
- [x] T003 [P] 创建 frontend/src/ui/dom.ts 组件工厂:基于 shared/dom.ts 的 el() 扩展 card/button/badge/table 行等基础构件(纯函数组件,无状态)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: 全部故事依赖的无视觉基础组件

**⚠ CRITICAL**: 本阶段完成前不开始任何用户故事

- [x] T004 [P] 创建 frontend/src/styles/tokens.css:双主题设计令牌(light 默认 + `[data-theme="dark"]`),五类语义色(normal/warning/danger/neutral/info)、间距、字号层级、圆角、阴影;暗色下语义色仍可辨(FR-004)
- [x] T005 [P] 创建 frontend/src/ui/poll.ts:可见性门控轮询器——`document.visibilityState === "visible"` 时按 30s 间隔回调,隐藏即暂停,返回显式 stop()(FR-006/研究 D5)
- [x] T006 [P] 创建 frontend/src/ui/time.ts:基于 Intl.RelativeTimeFormat 的相对时间格式化 + 1 秒 tick 更新等待时长;页面隐藏暂停 tick(边界条款"等待时长自然增长")
- [x] T007 [P] 创建 frontend/src/ui/states.ts:asyncBlock(loader) 三态组件——loading 骨架屏(占位尺寸与内容一致,无布局跳变)、empty(必含引导动作)、error(错误摘要+重试按钮)(FR-003)
- [x] T008 [P] frontend/src/ui/poll.test.ts 与 frontend/src/ui/time.test.ts:轮询暂停/恢复、tick 隐藏暂停、相对时间格式化(Vitest)
- [x] T009 [P] frontend/src/ui/states.test.ts:三态切换、重试触发、空态引导动作存在、骨架屏占位稳定(Vitest)

**Checkpoint**: 基础组件就绪,用户故事可开始

---

## Phase 3: User Story 1 - 统一布局与视觉框架 (Priority: P1) ⭐ MVP

**Goal**: 侧边栏+顶栏+卡片化内容区、双主题、组件词汇(按钮层级/弹窗/抽屉)、键盘可用性——成体系观感的地基

**Independent Test**: 打开任意页面即见统一框架;主题切换且记忆;停服看错误块+重试;窄窗侧栏折叠(quickstart §1)

### Implementation for User Story 1

- [x] T010 创建 frontend/src/styles/base.css:reset、排版基线、CSS 变量消费(tokens.css 令牌)、暗色适配
- [x] T011 创建 frontend/src/styles/components.css:按钮层级(主/次/危险;禁用态带原因提示)、卡片、表格、徽章、表单控件、骨架屏、空态、错误块、时间线、标签页样式
- [x] T012 创建 frontend/src/ui/theme.ts:主题存储(localStorage,键 `qing-theme`;默认 light;非法值回退 light)并在 frontend/src/main.ts 渲染前应用 `<html data-theme>`(避免闪烁)(FR-002)
- [x] T013 创建 frontend/src/app/shell.ts 并改造 frontend/src/app/router.ts:登录后挂载一次外壳(侧边栏 6 导航项+当前项高亮、顶栏页面标题/管理员标识/主题切换/退出、内容区),路由切换仅重渲染内容区;窗口收窄侧栏折叠为图标条、内容不横向溢出(FR-001)
- [x] T014 [P] 创建 frontend/src/ui/modal.ts:居中弹窗——叠层至多一层可交互、Esc 关闭、关闭焦点回归触发元素、可挂脏检查确认回调(键盘可用性边界)
- [x] T015 [P] 创建 frontend/src/ui/drawer.ts:侧滑抽屉——同 modal 叠层规则、Esc/遮罩关闭走脏确认、焦点管理(键盘可用性边界)
- [x] T016 frontend/src/ui/theme.test.ts、ui/modal.test.ts、ui/drawer.test.ts:主题记忆与非法回退、Esc/脏确认/焦点回归(Vitest)
- [x] T017 [US1] 手动走查 quickstart §1 六项场景(US1 验收 1-6),结果记录到 docs/evidence/ui-acceptance.md

**Checkpoint**: US1 独立可验收——裸表格已升级为成体系框架(MVP)

---

## Phase 4: User Story 2 - 概览工作台 (Priority: P1)

**Goal**: 统计卡+恢复横幅+账号速览+最近订单/待处理+快捷动作,30s 轮询

**Independent Test**: quickstart §2;统计卡数字与列表页实际条目一致(SC-204);30 秒无操作自动刷新且无布局跳变(US2-6)

### Implementation for User Story 2

- [x] T018 [P] [US2] 创建 frontend/src/ui/badge.ts:StatusTone 全局唯一映射——001 全部账号状态(在线/离线/令牌过期/已暂停/暂停中/封禁)与订单内容/审核/确认状态 → 五类语义色;未收录值 → neutral"未知状态"+ title 保留原始值(FR-004/FR-021)
- [x] T019 [P] [US2] frontend/src/ui/badge.test.ts:遍历全部已收录枚举断言 tone、注入未收录值断言兜底、两主题可辨性(静态类名断言)(SC-205 前置)
- [x] T020 [US2] src/adapters/sqlite/repos/orders.rs 新增 `count_today` 只读聚合:`orders_today = COUNT(paid_at >= 本地午夜)`、`delivered_today = 同窗口 content_state='delivered'`;内联单测覆盖跨午夜边界、零订单、`delivered_today ≤ orders_today` 不变量(contracts/dashboard-api.md)
- [x] T021 [US2] src/transport/dashboard_api.rs 序列化新增 `orders_today/delivered_today` 两字段;集成测试断言字段存在、类型与不变量;既有字段与错误码不变(additive)
- [x] T022 [US2] frontend/src/features/overview/page.ts:四张统计卡(在线/总账号、今日订单、今日已自动交付、待人工处理),每卡可下钻对应页面,待处理非零警告色强调(FR-006)
- [x] T023 [US2] overview 恢复隔离横幅(未完成核对时置顶+直达入口)与账号状态速览(异常优先、最近心跳相对时间、监控范围)(FR-007/FR-008)
- [x] T024 [US2] overview 最近订单(10 条,双轴状态标签,badge.ts)与最近待处理摘要(5 条,等待时长 time.ts)与快捷动作区(扫码接入/同步商品/订单中心)
- [x] T025 [US2] overview 接入 ui/poll.ts 30 秒可见性轮询+手动刷新按钮,刷新无布局跳步;`orders_today/delivered_today` 缺失时显示"统计不可用"占位而非 0(契约降级要求);子块错误独立重试(US2-5)
- [x] T026 [US2] frontend/src/features/overview/page.test.ts:统计卡派生(异常排序/计数)、字段缺失降级、恢复横幅出现条件(Vitest)
- [x] T027 [US2] 手动走查 quickstart §2 并核对 SC-204(概览数字 vs 订单页实际条目),记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 概览页独立可验收,后端唯一改动落地

---

## Phase 5: User Story 3 - 账号管理页 (Priority: P1)

**Goal**: 卡片网格+扫码弹窗全流程+启停确认流

**Independent Test**: quickstart §3——空态引导→扫码(倒计时/轮询/过期重取)→启停确认与"暂停中"过渡态

- [x] T028 [US3] frontend/src/features/accounts/page.ts:卡片网格(昵称、平台号脱敏、状态徽章、启停开关、最近心跳相对时间、监控范围标签;异常状态排前);无账号空态+扫码接入 CTA(FR-009)
- [x] T029 [US3] frontend/src/features/accounts/qr-modal.ts:扫码接入弹窗全流程——POST /accounts/qr-sessions → 轮询 GET …/qr-sessions/{id} → 二维码图片+剩余有效期倒计时 → 成功/失败/过期;过期后弹窗内"重新获取"(不关闭重开);长时间无响应显示"仍在等待"+取消按钮(边界条款)(FR-010)
- [x] T030 [US3] accounts 启停切换:确认弹窗(停用影响说明)→ 确认后 POST /accounts/{id}/control → "暂停中"过渡态轮询至终态 → 徽章即时更新;取消无变更;执行中按钮锁定(FR-005)
- [x] T031 [US3] accounts 验证入口按 GET /capabilities 门控:不可用时明确展示不可用状态与原因,不伪称可用(FR-011)
- [x] T032 [US3] frontend/src/features/accounts/qr-modal.test.ts:状态机(等待→二维码→轮询→过期→重取)、取消清理轮询、控制确认取消不变更(Vitest)
- [x] T033 [US3] 手动走查 quickstart §3,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 账号页独立可验收

---

## Phase 6: User Story 4 - 商品与发货规则页 (Priority: P1)

**Goal**: 商品筛选+同步反馈+规则编辑抽屉(行内校验)+引擎匹配预览

**Independent Test**: quickstart §4——筛选/同步计数反馈/校验拒绝保存/预览三种结果(matched/none/ambiguous)

- [x] T034 [US4] frontend/src/features/catalog/products.ts:商品表格(缩略占位、超长标题截断+悬浮全文、价格、在售/下架徽章、已配置规则徽章、所属账号)+筛选(账号/仅已配置/关键字,客户端过滤)+同步按钮(进行态锁定;完成反馈新增/更新/失败计数,失败明细可展开)(FR-012)
- [x] T035 [US4] frontend/src/features/catalog/rule-editor.ts:规则行(商品、内容来源、延迟、模板摘要、启停)点击打开 drawer;表单(内容来源选择、模板多行输入带字数统计与占位符插入按钮、发货延迟、启用开关);行内实时校验——必填、字数≤shared/contracts.ts 既有 001 上限、占位符属 001 合法集合(引用既有常量,不自定口径);脏表单关闭确认(US4-5)(FR-013)
- [x] T036 [US4] frontend/src/features/catalog/match-preview.ts:预览面板——商品选择(本地过滤已同步目录,仅定位 item_id)+规格选择 → POST /accounts/{id}/rules/match-preview → matched(绿徽章+规则信息→再调 POST …/rules/preview 展示最终发送内容)/none(明确"无规则命中,将不自动发货")/ambiguous(冲突警告,引导去重);前端零匹配算法(FR-014/contracts/match-preview-api.md)
- [x] T037 [US4] frontend/src/features/catalog/rule-editor.test.ts 与 features/catalog/match-preview.test.ts:校验规则(空内容/超字数/非法占位符拒绝)、三结果呈现、断言仅经端点判定(Vitest)
- [x] T038 [US4] 手动走查 quickstart §4,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 规则页独立可验收,防错交互(校验+引擎预览)落地

---

## Phase 7: User Story 5 - 订单中心与详情 (Priority: P1)

**Goal**: 组合筛选分页+双轴标签+详情抽屉(三轴时间线/尝试记录/内容揭示/guard 人工动作)

**Independent Test**: quickstart §5;概览→订单详情 ≤3 次交互(SC-203)

- [x] T039 [US5] frontend/src/features/orders/page.ts:筛选栏(订单号搜索、账号下拉、内容状态——001 全部枚举、平台状态、时间范围)组合筛选+清空;分页(50/页,翻页保留筛选,总页数 1 时禁用)(FR-015)
- [x] T040 [US5] frontend/src/features/orders/table.ts:表格列(时间、订单号截断、账号、商品标题截断、金额、双轴状态标签=内容主+审核/确认副、自动/人工接管标识;长文本统一截断+悬浮)(FR-016)
- [x] T041 [P] [US5] frontend/src/ui/timeline.ts:三轴状态时间线组件(节点=时间+轴+状态+语义色;未知枚举走 badge 兜底)
- [x] T042 [US5] frontend/src/features/orders/detail.ts:详情抽屉——基本信息、timeline 三轴全貌、交付尝试记录(时间/结果/重试倒计时)、交付内容默认折叠点击揭示(经授权端点 /content,可重新折叠)、人工动作区按 guard 允许集启停:不可用操作禁用+原因,绝不点击后才报错(FR-017)
- [x] T043 [US5] frontend/src/features/orders/detail.test.ts:筛选跨页保留、guard 启停逻辑(以详情载荷为准)、揭示/折叠切换(Vitest)
- [x] T044 [US5] 手动走查 quickstart §5+SC-203 点击深度,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 订单中心独立可验收——交付审计的用户侧呈现完整

---

## Phase 8: User Story 6 - 待处理工作台 (Priority: P2)

**Goal**: 分类分组+统一确认流(原因必填/风险勾选)+完成即时移出

**Independent Test**: quickstart §6——空原因无法提交;成功后条目移出、计数联动

- [x] T045 [US6] 创建 frontend/src/ui/confirm-flow.ts:统一确认流组件——影响说明、改变交付结果的操作原因必填(空原因禁提交)、高危操作风险勾选、提交中锁定防重复、成功反馈(FR-005/FR-018)
- [x] T046 [US6] frontend/src/features/issues/page.ts:按类别分组(恢复核对/unknown 核对/接管/令牌过期等)——组计数、条目(类别、关联订单/账号、等待时长 time.ts、风险文案、允许操作);页头总数+最久等待;全部处理完毕正向空态(FR-018)
- [x] T047 [US6] issues 接线:恢复核对走 GET /restore/reviews + POST …/decisions(原因必填);unknown 核对走订单人工动作端点;成功后条目动画移出+组计数/总数即时更新(contracts/ui-consumption-map.md)
- [x] T048 [US6] frontend/src/features/issues/page.test.ts 与 ui/confirm-flow.test.ts:空原因阻断、风险勾选强制、提交锁定、移出后计数更新(Vitest)
- [x] T049 [US6] 手动走查 quickstart §6,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 待处理工作台独立可验收——安全网的用户侧出口成型

---

## Phase 9: User Story 7 - 设置与安全页 (Priority: P2)

**Goal**: 运行信息/备份恢复指引/会话安全/本地化说明

**Independent Test**: quickstart §7——运行信息与实际一致;恢复入口展示完整后果

- [x] T050 [US7] frontend/src/features/settings/page.ts:运行信息卡(版本、数据目录、监听地址、执行模式、运行时长——经 GET /capabilities 与 /auth/session)与安全卡(会话剩余有效期、CLI 重置指引)与关于卡(本地运行、数据不出本机)(FR-019)
- [x] T051 [US7] settings 备份/恢复卡:最近备份时间或引导文案;恢复入口确认流——明确后果(进入恢复隔离、全部会话吊销、需逐单核对),确认后执行并引导跳转待处理页(US7-2/3)
- [x] T052 [US7] 手动走查 quickstart §7,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 设置页独立可验收

---

## Phase 10: User Story 8 - 登录与初始化体验 (Priority: P2)

**Goal**: 品牌化初始化/登录卡片+会话过期回落并恢复原页面

**Independent Test**: quickstart §8——密码错误行内提示;会话过期后操作回落登录,重登回订单页

- [x] T053 [US8] 重写 frontend/src/app/auth.ts 初始化卡片:产品标识、管理员密码表单(强度提示、≥12 字符校验、二次输入一致性、行内错误)、数据目录展示与安全说明(US8-1)
- [x] T054 [US8] 登录卡片(密码输入、错误行内提示、登录中按钮态)+ 会话过期处理:操作遇 401/403 → 保留目标路由回落登录 → 重登成功返回原页面(而非固定概览)(app/auth.ts/app/router.ts)(FR-020)
- [x] T055 [US8] frontend/src/app/auth.test.ts:密码长度/一致性校验、错误行内呈现、过期回落与返回路由(Vitest)
- [x] T056 [US8] 手动走查 quickstart §8,记录 docs/evidence/ui-acceptance.md

**Checkpoint**: 八个用户故事全部独立可验收

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: 跨故事收尾与全量验证

- [x] T057 [P] SC-205/SC-201 收口:夹具注入未知枚举全链走查(无崩溃/无空白/原始值悬浮)+ 暗色主题语义色对比走查,结果记入 docs/evidence/ui-acceptance.md
- [x] T058 清理:删除旧版 frontend/src/features/*/index.ts 与 shared/styles.css 残留;`npm run typecheck && npm run test && npm run build` 全绿;`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`(基线 ≥97,SC-206)
- [x] T059 完整运行 quickstart.md 全部 9 组场景(端到端走查矩阵),汇总通过/失败清单到 docs/evidence/ui-acceptance.md
- [x] T060 [P] SC-207 度量:前端构建产物压缩后 ≤1.5MB(vite 报告)、概览首屏可交互 ≤2s(本地)、嵌入后资源哈希一致性(磁盘 vs 服务,webui.rs 重建跟踪回归),记入 docs/evidence/ui-acceptance.md
- [x] T061 发行验证:scripts/release.ps1 白名单打包,产物脱离 Node/参考目录独立运行(宪章 V)
- [x] T062 文档收口:README 运行章节补界面说明;docs/evidence/ui-acceptance.md 汇总验收矩阵(SC-201~207 对照)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 无依赖,立即开始
- **Foundational (Phase 2)**: 依赖 Phase 1;**阻塞全部用户故事**(tokens/poll/time/states 为各页必需)
- **User Stories (Phase 3-10)**: 全部依赖 Phase 2;按优先级顺序 P1(US1→US5)→P2(US6→US8)交付;US2 的 T020/T021(后端)与前端任务无文件冲突
- **Polish (Phase 11)**: 依赖全部所需故事完成

### User Story Dependencies

- **US1 (P1)**: Phase 2 后即可开始,无跨故事依赖 —— **MVP**
- **US2 (P1)**: 依赖 Phase 2(badge 在本阶段内交付);后端 T020→T021 顺序执行
- **US3/US4/US5 (P1)**: 各自依赖 Phase 2(复用 US1 的 modal/drawer/badge 组件,故建议在 US1 后);彼此独立
- **US6 (P2)**: 复用 confirm-flow(阶段内交付)与订单人工动作端点;建议在 US5 后(详情抽屉的动作语义已定型)
- **US7/US8 (P2)**: 仅依赖 Phase 2 与 US1 外壳;彼此独立

### Within Each User Story

- 组件/视图模型先于页面接线;后端聚合先于 dashboard 序列化(T020→T021)
- 每故事:实现 → Vitest → 手动走查(quickstart 对应节)→ 才算完成

### Parallel Opportunities

- Phase 1:T001 完成后 T002/T003 并行
- Phase 2:T004-T009 六项全部可并行(不同文件)
- US1:T014/T015(modal/drawer)并行;T010/T011 与 T012/T013 可并行
- US2:T018/T019(badge)与 T020/T021(后端)跨语言并行;T022-T024 页面分块在骨架定型后可并行
- US4:T034(products)与 T036(match-preview)可并行;US5:T041(timeline)独立
- Polish:T057/T060 并行;不同故事可由不同人并行(Phase 2 后)

---

## Parallel Example: User Story 2

```text
# 后端线(Rust)与前端线(TS)并行:
Task T020: "count_today 聚合 + 单测(src/adapters/sqlite/repos/orders.rs)"
Task T018: "badge.ts 语义色映射(frontend/src/ui/badge.ts)"

# 后 T021(依赖 T020)与 T019(依赖 T018)各自跟进,
# 汇合于 T022-T025(overview 页面接线)
```

---

## Implementation Strategy

### MVP First (Phase 1+2+US1)

1. 完成 Setup + Foundational(tokens/三态/轮询/相对时间)
2. 完成 US1(外壳+主题+组件词汇)→ **STOP 验证**:quickstart §1 独立走查
3. 此时的交付已消除"裸表格"核心抱怨,可随时展示

### Incremental Delivery

1. Foundation → US1(MVP)→ US2 概览 → US3 账号 → US4 规则 → US5 订单 → US6 待处理 → US7 设置 → US8 登录
2. 每个故事完成后按其 Independent Test 独立验收(quickstart 对应节),不阻塞下一故事
3. Polish 收口:全量走查+基线回归+SC-207 度量+发行验证

---

## Notes

- [P] = 不同文件且无未完成依赖
- 后端改动仅 T020/T021(dashboard additive),其余任务不触 Rust;任何超出 contracts/ 的端点需求必须先修订契约(consumer map 越界禁令)
- 界面不得出现 SQL/平台协议/匹配判定逻辑(宪章 II);交付内容揭示仅经授权端点,不进前端持久化(宪章 V)
- 测试基线:cargo ≥97 项不回退(SC-206);每任务完成后运行对应检查再勾选
- 走查记录统一沉淀 docs/evidence/ui-acceptance.md(SC-201~207 验收矩阵)

---

## Phase 12: Convergence

**Purpose**: 2026-09-23 收敛核查:代码实现与规格总体吻合(10 点浏览器走查+全部测试门禁通过),但两处人工路径的实现与后端契约不一致导致功能必然失败

- [x] T063 修复订单详情内容揭示的 HTTP 方法:frontend/src/features/orders/detail.ts 揭示按钮当前以 GET 调用 `/accounts/{id}/orders/{id}/content`,而该端点仅接受 POST(routes.rs reveal_content)且 POST 会自动携带 CSRF——改为 httpPost 并补一条揭示成功的走查/测试 per T042/FR-017 (partial)
- [x] T064 修复人工接管确认流的历史范围旗标:detail.ts 与 issues/page.ts 的 takeover 提交体缺少 `acknowledge_historical_scope: true`(服务端 manual_api 强制要求,缺失即被拒)——在确认流增加 takeover 专用风险文案(确认接管 monitor_since 之前订单)并随请求传递该旗标 per T070/FR-022、US4/US6 (partial)

## Phase 13: Convergence

**Purpose**: 2026-09-23 第二轮收敛:并行核查确认 T063/T064 属实待修;本轮独立复核另发现 4 处低危缺口(边界条款/测试覆盖/契约文档与规格口径对齐)

- [x] T065 相对时间悬浮改为本地化绝对时间:概览与订单页的时间单元格 title 当前为原始 RFC3339 UTC(如 `title: o.paid_at`),按边界条款"悬浮显示绝对时间"应显示本地时区绝对时间——用 ui/time.ts 的 formatAbsolute 替换 frontend/src/features/overview/page.ts 与 features/orders/table.ts 的 title 赋值 per 边界条款"时间显示" (partial)
- [x] T066 补订单分页保留筛选的行为测试:frontend/src/features/orders/ 下游标栈(cursorStack)与筛选状态跨页保留逻辑无自动化断言——在 orders.test.ts 增加分页/筛选组合的行为测试(纯状态层抽出或 DOM 级) per US5-1/US5-5 (partial)
- [x] T067 修正消费图谱的验证端点方法:contracts/ui-consumption-map.md「账号页」表将验证入口记为 `GET /accounts/{id}/verification`,实现为 POST(发起验证任务)——按实现修正文档口径,避免后续调用方误读 per FR-022/契约准确性 (contradicts)
- [x] T068 规则抽屉帮助文案与引擎能力对齐:规格 US4 叙述提到"占位符插入按钮",但 001 引擎为纯固定文本(rules/preview 原样渲染、无占位符替换体系)——在抽屉帮助文案如实说明"当前为纯固定文本,无占位符替换",不承诺不存在的占位符能力,并将该口径差异记入规格假设 per 宪章"不虚报能力"/US4 (partial)
