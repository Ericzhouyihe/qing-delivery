---
description: "Task list for 004-account-profile implementation"
---

# Tasks: 账号资料同步与管理增强(004-account-profile)

**Input**: Design documents from `/specs/004-account-profile/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/accounts-api.md, quickstart.md

**Tests**: Included——宪章 IV;确定性测试+契约测试+实账号走查(R11 模式)。

**Organization**: 按用户故事分阶段;研究决策 D1-D6 已内化。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行(不同文件、无未完成依赖)
- **[Story]**: US1(资料同步)/US2(删除)/US3(ID+备注)

---

## Phase 1: Setup (Shared Infrastructure)

- [x] T001 新建 migrations/0003_account_profile.sql:accounts 增列 `avatar_url TEXT`(可空)、`remark TEXT`(可空);迁移版本注册+表断言更新;`SUPPORTED_SCHEMA` 常量 2→3(src/adapters/sqlite/migrations.rs、src/application/backup/restore.rs);accounts 仓储增两列读写(src/adapters/sqlite/repos/accounts.rs)并附单测(空值/回读) per data-model §迁移 0003
- [x] T002 [P] src/adapters/xianyu/mtop/client.rs 增 GET 型调用:`call_get(api, query_pairs, referer)`——签名/超时/Cookie 吸收与既有 call 一致(user.page.nav 为 GET query 型,研究 D1);单测:query 组装与签名复用

---

## Phase 2: Foundational (Blocking Prerequisites)

- [x] T003 新建 src/adapters/xianyu/profile.rs:`fetch_profile(session) -> ProfileResult{nickname, avatar_url}`——GET `mtop.idle.web.user.page.nav` v1.0;解析 `module.base.{displayName→displayNick 兜底→空}`(引用项目 user_profile.go:175-190 同链);风控错误沿既有 `MtopError::Verification` 分类(转 003 由服务层接);纯解析函数单测(三字段/兜底链/空对象)
- [x] T004 新建 src/application/accounts/profile.rs:ProfileService——`refresh(account_id)`:载凭证→fetch_profile→UPDATE accounts SET display_name/avatar_url(空昵称不覆盖旧值)→落审计日志;`MtopError::Verification` → 构造 VerificationSignal 送 003 服务(FR-005);Cookie 顺带吸收落库复用 mtop 吸收路径(FR-004);单测:成功更新/空昵称保留/风控转信号(fake fetch)

**Checkpoint**: 资料域确定性可用

---

## Phase 3: User Story 1 - 资料同步:昵称与头像 (Priority: P1) ⭐ MVP

**Goal**: 授权后 30 秒内账号卡显示真实昵称+头像;可手动刷新

**Independent Test**: quickstart §实账号走查 1-2 + 确定性单测

- [x] T005 [US1] src/application/accounts/authorize.rs:凭证保存成功后 `tokio::spawn` 一次 ProfileService::refresh(失败仅日志,不阻塞接入;FR-001/研究 D4)
- [x] T006 [US1] src/transport/accounts_api.rs:①GET items 元素增 `avatar_url`/`remark` 字段;②新增 POST `/accounts/{id}/profile-fetch`(202+job;风控→409 转验证提示,契约 §profile-fetch);routes 挂载
- [x] T007 [US1] frontend/src/features/accounts/page.ts:头像改 `<img src=avatar_url>`(onerror 隐藏回退首字占位,FR-003);昵称显示 display_name(首拉后自动为真实昵称);"刷新资料"按钮(进行态→轮询 GET accounts 至更新/失败提示)
- [x] T008 [P] [US1] 新建 tests/contract/accounts_profile_http.rs:items 新字段、profile-fetch 202/409/404;前端 Vitest:头像回退(onerror)、刷新按钮进行态
- [x] T009 [US1] 走查 quickstart §实账号走查 1-2(首拉 30s、刷新 ≤5s),结果记 docs/evidence/ui-acceptance.md(账号页小节)

**Checkpoint**: MVP——资料链路端到端可用

---

## Phase 4: User Story 2 - 删除账号 (Priority: P1)

**Goal**: 软移除三步;审计保留;重扫码=新账号

**Independent Test**: quickstart §实账号走查 5 + 契约测试

- [x] T010 [US2] 新建 src/application/accounts/removal.rs:`remove(account_id) -> Result<Removed, RemoveBlocked>`——①UPDATE accounts SET runtime_enabled=0, status='disabled'(supervisor tick 自然停值守);②DELETE FROM account_credentials(加密凭证清零);③DELETE FROM accounts 行(FK 阻塞则 status='deleted' 兜底,列表过滤);执行前检查 VerificationService 活跃会话→Some 返回 RemoveBlocked(409);单测:三步顺序/凭证 0 残留/订单行保留/重扫码建新账号(UNIQUE 放行)/活跃会话阻塞
- [x] T011 [US2] src/transport/accounts_api.rs:DELETE `/accounts/{id}`(204/409 活跃验证/404;契约 §DELETE);routes 挂载
- [x] T012 [US2] frontend/src/features/accounts/page.ts:删除按钮(danger)→确认流(输入 ID 片段确认,明示"凭证清除不可恢复;订单历史保留")→成功后卡片移除;Vitest:确认输入不匹配拒绝、成功移除
- [x] T013 [US2] 走查 quickstart §实账号走查 5(删测试账号:列表消失/订单保留/凭证核验/重扫码新账号),记证据文档

**Checkpoint**: 接错账号可干净退出

---

## Phase 5: User Story 3 - 完整 ID 与备注 (Priority: P2)

**Goal**: ID 完整显示+复制;备注持久

**Independent Test**: quickstart §实账号走查 3-4

- [x] T014 [US3] src/transport/accounts_api.rs:PATCH `/accounts/{id}`(body {remark};**≤64 字符**校验,超长 400;null=清空;契约 §PATCH);routes 挂载
- [x] T015 [US3] frontend/src/features/accounts/page.ts:①完整显示 external_user_id+一键复制(navigator.clipboard,失败回退选中提示);②备注行(昵称下方,"暂无备注"弱文案)+行内编辑(PATCH 保存/取消);Vitest:复制调用、备注保存请求体、超长前端拦截
- [x] T016 [P] [US3] tests/contract/accounts_profile_http.rs 增:PATCH 200/400(65 字符)/404
- [x] T017 [US3] 走查 quickstart §实账号走查 3-4,记证据文档

**Checkpoint**: 三个故事全部独立可验收

---

## Phase 6: Polish & Cross-Cutting Concerns

- [x] T018 全量门禁:`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`(基线 ≥146+新增,SC-404);前端 typecheck/vitest/build;交付安全不变量测试零修改通过
- [x] T019 文档收口:README 账号章节补资料同步/删除语义;docs/evidence/ui-acceptance.md 汇总 SC-401~404;实账号资料字段名差异记 R11(quickstart §实账号走查)
- [x] T020 重新部署:release 构建→dist 替换→59189 重启→浏览器 Ctrl+F5 走查 6 项全过

---

## Dependencies & Execution Order

- **Setup**: T001→T002 可并行;均阻塞后续
- **Foundational**: T003→T004(顺序)
- **US1**: 依赖 Phase 2;T005/T006 顺序,T007 依赖 T006,T008 可并行
- **US2**: 依赖 T001(列)+T004(验证检查用服务句柄);与 US1 可并行
- **US3**: 仅依赖 T001/T006(items 字段);与 US2 可并行
- **Polish**: 依赖全部

### Parallel Opportunities
- T001/T002;T008 与 T007;US2 全线与 US1 后半;T016 与 T015

---

## Implementation Strategy

### MVP First(Phase 1+2+US1)
迁移+GET 调用+资料服务→首拉接线→端点→前端头像/刷新。完成即:扫码接入显示真实昵称头像。

### Incremental Delivery
MVP → US2 删除(数据卫生)→ US3 ID/备注(辨识)→ Polish(门禁/文档/部署)。

---

## Notes

- 平台字段名差异按 R11 校准;昵称兜底链 displayName→displayNick 已按参考项目实证
- 删除是终局操作:确认流文案必须含"凭证清除不可恢复;订单历史保留"
- 基线任何时刻不得回退;全部 API additive
- 走查证据统一 docs/evidence/ui-acceptance.md

## Phase 7: Convergence

**Purpose**: 2026-09-25 收敛:用户指出与 Ydisks 复刻度差距——操作入口按钮/图标混用不一致、账号编辑弹窗未实现、整体复刻不完整。核对参考源码(AccountCard.tsx 138 行/AccountEditModal 330 行/AccountDeleteDialog 65 行)后确认差距属实。

- [x] T021 操作入口统一为纯图标按钮(对齐 Ydisks AccountCard.tsx:111-127):全部操作改为图标+悬浮提示(title)——⟳刷新资料(RefreshCw)、重新扫码授权(QrCode)、✎编辑账号(Edit2)、⏻启用/停用(Power,绿/灰双色)、🗑删除(Trash2,红);去掉"发起验证"文字按钮改为🛡图标(盾);每行末尾统一排布;图标悬浮有背景色变化 per 用户复刻要求 (contradicts)
- [x] T022 实现账号编辑弹窗(对齐 Ydisks AccountEditModal.tsx):点击✎打开居中弹窗——只读展示(头像大图、昵称、平台 ID)、备注输入(≤64)、自动交付/自动确认/自动平台确认三开关(独立切换)、保存/取消;保存走 PATCH(备注)+ control(开关,确认流) per 用户复刻要求 (missing)
- [x] T023 删除确认弹窗对齐 Ydisks AccountDeleteDialog.tsx:红色警示图标+账号头像/昵称/ID 展示块+后果说明+"删除中…"进行态(删除期间按钮禁用+spinner),移除当前的简单 confirmDialog per 用户复刻要求 (partial)
- [x] T024 账号行/卡信息区对齐:头像右下角在线状态小圆点(绿=在线,灰=离线);昵称+状态徽章+验证徽章同排;第二行"备注 或 暂无备注"+"ID: xxx"(mono);hover 边框高亮 per Ydisks AccountCard.tsx:52-99 (partial)
- [x] T025 状态异常时(授权失效/需验证)在行内显示警示条+「重新授权」红色小按钮(对齐 Ydisks requiresLogin 分支);runtime_message 展示 per Ydisks AccountCard.tsx:96-106 (missing)
