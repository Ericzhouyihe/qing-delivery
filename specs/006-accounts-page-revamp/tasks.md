# Tasks: 账号管理页面复刻与视图切换(006-accounts-page-revamp)

**Input**: Design documents from `/specs/006-accounts-page-revamp/`

**Prerequisites**: plan.md ✅, spec.md ✅, research.md ✅, data-model.md ✅, contracts/ui-contract.md ✅, quickstart.md ✅

**Tests**: 包含 vitest 行为测试任务(plan.md R9/宪章 IV 门禁要求:typecheck + 行为测试 + 构建)。测试先写、先失败(TDD 顺序)。

**Organization**: 按用户故事分组;本特性为纯前端改版,Phase 1/2 极薄,主体在 US1/US2/US3 三个阶段。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

本特性只涉及前端(仓库已有 `frontend/` 单前端布局):代码在 `frontend/src/`,无后端改动。

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: 确认基线,不新建项目(基础设施全部已存在)

- [x] T001 运行基线门禁确认全绿后再动代码:`cd frontend && npm run typecheck && npm run test && npm run build`;任一失败先记录并修复或向用户报告,不得带着红基线开始

**Checkpoint**: 基线绿,可以开始 Phase 2。

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: US1/US2/US3 都要用的共享组件;不完成不得开始用户故事

- [x] T002 [P] 创建内联 SVG 图标表 `frontend/src/ui/icons.ts`:键名联合类型 `search|qr|refresh|shield|edit|power|trash|list|grid`,`icon(name, size=18)` 返回 `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true" focusable="false">`;全部路径自绘,不复制第三方资产(契约 §2.2、宪章 V)
- [x] T003 [P] 创建图标表测试 `frontend/src/ui/icons.test.ts`:每个键名返回 `svg` 元素、含 `aria-hidden`、`currentColor` 继承、`size` 参数生效;先写先失败

**Checkpoint**: 共享图标就绪——US1(主按钮二维码图标)、US2(操作组)、US3(分段控件)都依赖它。

---

## Phase 3: User Story 1 - 页面框架与搜索过滤 (Priority: P1) 🎯 MVP

**Goal**: 账号页出现参考图的页头(标题+副标题)、搜索框、"扫码添加新账号"主按钮、计数横幅;搜索按昵称/备注/ID 即时本地过滤

**Independent Test**: 打开账号页对照参考截图核对页头四要素;输入已知备注关键词验证过滤与计数(quickstart 场景 1–7)

### Tests for User Story 1 (先写,先失败)

- [x] T004 [P] [US1] 创建 `frontend/src/features/accounts/model.test.ts`,覆盖 `filterAccounts`:①昵称/备注/账号ID 三域子串命中;②不区分大小写;③query 首尾空格被 trim;④空 query 返回全部且不改顺序;⑤无命中返回空数组;再覆盖 `viewCounts(filtered, total)` 返回 `{shown, total}`(数据模型 §2)

### Implementation for User Story 1

- [x] T005 [US1] 创建 `frontend/src/features/accounts/model.ts`(依赖 T004 的断言作为行为规格):导出 `filterAccounts(items: AccountCard[], query: string): AccountCard[]` 与 `viewCounts(filtered: number, total: number): {shown: number; total: number}`;匹配域 `displayName`/`remark`/`platformUserId`,比较前 `trim().toLowerCase()`;`AccountCard` 类型从 `./page` 复用(必要时将类型移到 model.ts 并由 page.ts 反向导入,避免循环依赖)
- [x] T006 [US1] 改版 `frontend/src/features/accounts/page.ts` 装配(依赖 T002、T005):①内容区顶部 `.page-head`:h1"账号管理"+ 副标题"管理您的闲鱼授权账号及设置。建议给账号填写备注，便于多账号区分。";②`.account-toolbar`:左侧搜索框 `placeholder="搜索昵称 / 备注 / 账号ID"`,`input` 事件驱动过滤(不回车/按钮),右侧保留既有文字视图切换(US3 再替换)并将主按钮改为"扫码添加新账号"(qr 图标 + 文案,点击仍走既有 `openQrModal`);③列表区上方常驻 `.count-banner`:"当前显示 X / Y 个账号"(X=过滤后,Y=总数,`viewCounts`)+ 右侧固定提示"如果某个账号只显示 ID，点该账号右侧"刷新资料"；若刷新失败，先点二维码重新授权。";④搜索无匹配时列表区显示"无匹配账号"轻空态 + "清空搜索"按钮(与总数为 0 的既有引导空态互斥);⑤`asyncBlock` 加载/错误占位与既有逻辑不动,页头与工具栏在加载期间保持稳定(契约 §1 P1–P6)
- [x] T007 [US1] 在 `frontend/src/styles/components.css` 增加 `.page-head`、`.account-toolbar`、搜索框、`.count-banner`(浅色信息底,左右两端对齐)、无匹配轻空态样式;布局对齐参考截图(标题区与工具栏同属页头块,窄屏可折行)

**Checkpoint**: US1 独立可验——quickstart 场景 1–7 全过;US2/US3 未动,条目仍是旧样式但功能完好。

---

## Phase 4: User Story 2 - 账号条目复刻 (Priority: P1)

**Goal**: 每个账号条目按参考图呈现:头像+状态圆点、昵称+能力徽标行、备注/ID 行、需验证警示条+行内重新授权、右侧纯图标操作组;既有行为零回退

**Independent Test**: 造一个在线账号 + 一个需验证/授权失效账号,逐项核对条目各段并逐个点击操作(quickstart 场景 8–13)

### Tests for User Story 2 (先写,先失败)

- [x] T008 [P] [US2] 扩展 `frontend/src/features/accounts/model.test.ts`(依赖 T005):①`sortAccounts`:verification_required/authorization_expired(0) > connecting(1) > online(2) > 其余(3),同级按 `displayName.localeCompare` 稳定排序;②`deriveCapabilityTags` 映射表断言:`verification_required|authorization_expired → {needs-verify,需要验证,warning}`、`autoDelivery===true → {auto-delivery,自动发货,normal}`、`autoConfirm===true → {auto-confirm,自动平台确认,normal}`;③**不虚标不变量**:遍历各类输入(含 AI 时代无关键),输出 id ⊆ {needs-verify, auto-delivery, auto-confirm},绝不出现 AI/自动评价/每日擦亮(数据模型 §2、FR-008)

### Implementation for User Story 2

- [x] T009 [US2] 在 `frontend/src/features/accounts/model.ts` 实现 `sortAccounts(items)`(迁移 page.ts 既有 `priority()` 语义)与 `deriveCapabilityTags(a: AccountCard): CapabilityTag[]`,导出 `CapabilityTag {id,label,tone}` 类型(依赖 T008)
- [x] T010 [US2] 重构 `frontend/src/features/accounts/page.ts` 条目装配(依赖 T002、T009):抽取两形态(row/card)共享片段——①头像+右下角状态圆点(online 绿/异常红/其余灰,破图回退首字);②徽标行 = 既有 `accountBadge` 连接徽章 + `deriveCapabilityTags` 能力徽标 + 既有验证会话轮询徽章("验证中/待人工验证",轮询与 MutationObserver 清理逻辑原样保留);③次行备注(空→"暂无备注")+ 完整 ID(卡片形态保留 ID 复制);④`verification_required|authorization_expired` 时显示警示条"闲鱼要求安全验证，请重新扫码并完成验证"+ 浅红底行内"重新授权"按钮(打开既有扫码弹窗,授权成功重载);⑤操作组统一改用 `icons.ts` 图标 + title:refresh 刷新资料(进行中禁用+旋转)、qr 重新授权(仅异常态,强调色底)、shield 发起验证(409 视为进行中)、edit 编辑、power 启停(danger 确认流 `applyControl` 原样)、trash 删除(危险红,既有确认弹窗);⑥**行为零回退清单**(FR-012)逐项自检:扫码接入/刷新资料/编辑备注与开关/启停确认/删除确认/验证入口全部保留(契约 §2.3)
- [x] T011 [US2] 在 `frontend/src/styles/tokens.css` 补徽标软色底等语义变量(如既有变量可覆盖则不改),在 `frontend/src/styles/components.css` 调整 `.account-row`/`.account-grid`/`.account-row-actions`/`.account-row-alert`/头像与圆点/徽标行样式,视觉对齐参考截图;悬停反馈:title 提示由浏览器渲染,悬停变色经 `icon-btn` 既有变体扩展

**Checkpoint**: US1+US2 均独立可用;quickstart 场景 8–13 全过,行为零回退清单逐项确认。

---

## Phase 5: User Story 3 - 图标式视图切换 (Priority: P2)

**Goal**: 主按钮左侧出现仅图标的列表/卡片分段切换控件:当前项高亮、悬停有提示、点击切换并记忆偏好

**Independent Test**: 点击分段控件切换视图并 F5 验证记忆;过滤状态下切换验证两视图结果一致(quickstart 场景 14–17)

### Tests for User Story 3 (先写,先失败)

- [x] T012 [P] [US3] 创建 `frontend/src/ui/segmented.test.ts`(依赖 T003 的测试组织方式):①容器 `role="group"`、`aria-label="视图切换"`;②每个选项是 `type="button"` 且内容仅 svg 无文字节点、`title`=label;③激活项 `aria-pressed="true"`、非激活 `"false"`;④点击未激活项触发 `onSelect(value)` 并迁移激活态;⑤点击已激活项不触发(幂等)。同批在 `frontend/src/features/accounts/model.test.ts` 追加偏好读取回退断言:键缺失 → `"row"`、值为 `"card"` → `"card"`、值为任意未知串 → `"row"`(FR-009/FR-010)

### Implementation for User Story 3

- [x] T013 [P] [US3] 创建 `frontend/src/ui/segmented.ts`:`createSegmented<V extends string>(opts: {options: Array<{value;icon;label}>, value, onSelect}) => HTMLElement`,契约 §2.1 全条款(aria-pressed/title/仅图标/幂等);控件不读写 localStorage、不发请求
- [x] T014 [US3] 在 `frontend/src/features/accounts/model.ts` 增加 `readViewPreference(): "row"|"card"`(读 localStorage 键 `qing-account-view`,缺失/未知回退 `"row"`)与 `writeViewPreference(v)`(依赖 T012)
- [x] T015 [US3] 集成 `frontend/src/features/accounts/page.ts` 与 `frontend/src/styles/components.css`(依赖 T013、T014):删除既有文字切换按钮,在主按钮左侧挂 `createSegmented`(list/grid 图标),`onSelect` → 更新 `viewMode`、`writeViewPreference`、重载列表;`.segmented` 样式:分段容器、激活项高亮(主色底)、悬停反馈

**Checkpoint**: 三个用户故事全部独立可用;quickstart 场景 14–17 全过。

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: 门禁收敛与视觉验收

- [x] T016 全量门禁:`cd frontend && npm run typecheck && npm run test && npm run build` 全绿;确认无新增运行时依赖(`frontend/package.json` 的 dependencies 未变化)且 `npm run build` 产物落到 `src/webui`
- [x] T017 [P] 按 `specs/006-accounts-page-revamp/quickstart.md` 场景 1–21 逐项人工验证(服务经 `cargo run` 或 `npm run dev` + 后端),完成 SC-001"对照参考截图逐元素齐全率 100%"核对表;如实记录已知限制(AI/自动评价/每日擦亮徽标按 FR-008 有意不复刻、顶栏"账号"文案不变)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 无依赖,立即开始;基线红则先处理
- **Foundational (Phase 2)**: 依赖 T001;T002/T003 可并行
- **US1 (Phase 3)**: 依赖 Phase 2(T002 图标、T005 模型);T004 → T005 → T006 → T007 严格顺序(T004 测试先行)
- **US2 (Phase 4)**: 依赖 US1 完成(model.ts 已存在、页面已装配);T008 → T009 → T010 → T011
- **US3 (Phase 5)**: 依赖 US2 完成(操作组已用 icons.ts,避免同文件冲突);T012 → (T013 ∥ T014) → T015
- **Polish (Phase 6)**: 依赖全部故事完成

### User Story Dependencies

- US1 与 US2/US3:US2、US3 均改造 `page.ts`/`components.css`,与 US1 同文件——**串行执行**(单人单分支下按 P1→P1→P2 顺序);独立"可测试"指每个故事完成后页面都可独立验收,而非文件级并行
- US2 内部:T008(测试)与 T009(实现)不同文件可紧邻;T010/T011 同依赖 T009
- US3 内部:T013(新文件 segmented.ts)与 T014(model.ts 追加)互不依赖,可并行

### Parallel Opportunities

- T002 ∥ T003(不同新文件)
- T004 先行,T008/T012 各自故事内测试先行,可与前置实现的验证并行编写
- T013 ∥ T014(不同文件)
- T016 ∥ T017 不行——T017 需要构建产物,先 T016 后 T017;T017 标 [P] 指与文档整理类工作可并行

---

## Implementation Strategy

### MVP First (US1 Only)

1. T001 → T002/T003 → T004–T007
2. **STOP & VALIDATE**: quickstart 场景 1–7 + 门禁三连
3. 此时页面已有参考图框架与搜索,条目为旧样式但功能完好——可作为中间交付

### Incremental Delivery

1. US1(框架+搜索)→ 验收 → US2(条目复刻)→ 验收 → US3(图标切换)→ 验收 → Polish
2. 每个故事交付后页面都完整可用,无跨故事半成品状态

### Single-Developer Path(本仓库实际情形)

T001 → T002 → T003 → T004 → T005 → T006 → T007 → T008 → T009 → T010 → T011 → T012 → T013 → T014 → T015 → T016 → T017

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- 测试任务(T004/T008/T012)先写先失败,是宪章 IV 门禁的一部分;不得先实现后补测试
- 涉及 `page.ts` 的任务(T006/T010/T015)串行执行,避免同文件冲突;每任务内自检 FR-012 零回退清单
- 提交节奏:每个任务或逻辑组一次提交;在 Checkpoint 处可暂停独立验收
- 注意并行会话风险:本特性期间发现 `.specify/feature.json` 被其他 005 会话改写过;凡 speckit 脚本输出 FEATURE_DIR 指向 005 时,用 `SPECIFY_FEATURE_DIRECTORY=specs/006-accounts-page-revamp` 重跑
