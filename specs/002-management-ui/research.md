# Research: 管理界面重构(002-management-ui)

**Date**: 2026-09-22 | **Status**: Complete(全部未知项已解决)

## 代码库现状核查(事实依据)

| 事实 | 位置 | 对本计划的影响 |
|---|---|---|
| 引擎级匹配预演端点已存在(T059) | `POST /accounts/{id}/rules/match-preview` → `Matched{rule_id,version} \| None \| Ambiguous`,src/application/catalog/rules.rs:259 | 澄清决定"后端预演"**零新增端点即可满足**;UI 只做商品定位+规格选择后调用 |
| 模板渲染预览端点已存在 | `POST /accounts/{id}/rules/preview`(rule_service.preview) | "最终发送内容预览"复用此端点,不在前端拼装占位符 |
| 概览摘要端点已存在(T075,设计为 3 秒轮询) | `GET /api/dashboard` → accounts[]、open_issue_count、active_job_count、restore、stopping、persistence | 概览统计卡主体复用;**唯一后端缺口**:无"今日订单数/今日已自动交付数" |
| 订单列表筛选已存在 | `GET /orders`(account_id/platform_state/paid_from/paid_to/cursor/limit)+ 订单详情/揭示/人工动作全套路由 | 订单中心全部功能无需后端改动 |
| 前端现状:6 页共 759 行 TS + 77 行 CSS,原生 DOM | frontend/src/{app,features,shared} | 重构=前端重建(布局外壳+组件库+六页重写),shared/http.ts 与 contracts.ts 模式保留 |
| 测试基线:97 项(cargo)+ Vitest 前端测试 | frontend/package.json scripts: typecheck/test/build | SC-206 回归门禁可直接沿用现有命令 |

## 决策记录

### D1: 前端实现方式——沿用原生 DOM,组件函数模式,不引入框架

- **Decision**: 保持 TypeScript + Vite + 原生 DOM;新增 `frontend/src/ui/` 组件库(纯函数组件工厂,基于现有 `el()` 扩展),不引入 Preact/Lit/React。
- **Rationale**: ①宪章 III 轻量原则——无新依赖、构建链不变;②SC-207 体积预算(≤1.5MB)原生方案富余最大;③现有 6 页与测试已基于原生 DOM,引入框架=全部重写且违背"界面重构不改变既有能力"的低风险取向;④单管理员本地工具,无 SSR/生态需求。
- **Alternatives**: Preact(~4KB)——仍需重写全部页面+调整构建与测试;Lit(Web Components)——生命周期与模板抽象对本地管理页过度;Tailwind——构建依赖与原子类体积,设计令牌用 CSS 自定义属性即可达成同等一致性。

### D2: 主题与设计令牌——CSS 自定义属性 + `data-theme` 属性切换

- **Decision**: `styles/tokens.css` 定义双套设计令牌(色彩语义、间距、字号、圆角、阴影),`<html data-theme="dark|light">` 切换;默认浅色,`localStorage` 记忆,初始化时尊重记忆值。语义色五类(normal/warning/danger/neutral/info)为唯一色彩词汇。
- **Rationale**: 零依赖实现 FR-002;令牌集中一处保证 SC-201 视觉一致性可走查。
- **Alternatives**: CSS-in-JS(需运行时)、双份样式表(维护漂移)。

### D3: 概览数据源——复用 `GET /api/dashboard`,增量补两个字段

- **Decision**: 概览页消费既有 dashboard 端点;后端在响应中**增量新增** `orders_today`、`delivered_today` 两个只读计数字段(SQL `COUNT` 按 `paid_at >= 本地午夜` 与 `content_state='delivered'` 聚合,聚合函数放 adapters/sqlite/repos,transport 仅序列化)。前端按 30 秒可见性轮询(复用 overview.ts 已有的 visibility 处理模式)。
- **Rationale**: 字段为纯增量(additive),不破坏既有消费者(SC-206);避免前端用分页列表自行计数的不准确方案。
- **Alternatives**: 新建 `/api/overview/summary` 独立端点——与 dashboard 职责重叠,多一次往返;前端拼装多个列表接口计数——分页截断导致数字不可信,已否决。
- **边界复核(FR-022)**: 该改动属"概览统计聚合"白名单类只读接口,聚合 SQL 位于 adapters 仓储层、不触平台协议与交付逻辑,符合宪章 II 分层。

### D4: 匹配预览交互流——商品定位在前端,匹配判定全部在后端

- **Decision**: 抽屉内"匹配预览"两步:①输入关键字**过滤本地已同步商品目录**(仅前端过滤展示,不参与判定)定位商品与规格;②调用既有 `match-preview` 得 Matched/None/Ambiguous,Matched 时再调 `rules/preview` 渲染最终内容。Ambiguous 呈现为冲突警告(对应 US4 重复配置警告)。
- **Rationale**: 满足澄清决定"预演=引擎同一套逻辑";前端零匹配算法;None/Ambiguous 语义直接来自引擎。
- **Alternatives**: 按自由文本标题直接匹配——001 规则是 item_id+sku 精确匹配体系,自由文本匹配需引擎新增模糊能力,超出 001 边界,已否决。

### D5: 三态与轮询规范——`ui/states.ts` 统一异步块组件

- **Decision**: `asyncBlock(loader)` 组件统一封装骨架屏/空态(含引导动作)/错误块(含重试);概览 30 秒轮询仅在 `document.visibilityState === 'visible'` 时进行;相对时间(`Intl.RelativeTimeFormat`)与等待时长由 1 秒 tick 更新,页面隐藏即暂停。
- **Rationale**: FR-003/US2-6 的可验收行为集中一处实现;避免每页各写一套轮询/计时逻辑。

### D6: 状态徽章——单一 tone 映射表 + 未知枚举兜底

- **Decision**: `ui/badge.ts` 维护唯一映射:账号状态(在线/离线/令牌过期/已暂停/暂停中/封禁)与订单内容/审核/确认状态(001 全部枚举)→ 五类语义色;未收录值渲染为中性"未知状态"徽章并以 `title` 保留原始值。
- **Rationale**: FR-004/FR-021/SC-205 要求的覆盖率与兜底集中可测(Vitest 遍历枚举表断言)。

### D7: 布局外壳——`app/shell.ts` 单例骨架 + 路由渲染内容区

- **Decision**: 登录后挂载一次布局外壳(侧边栏+顶栏+内容区),路由切换只重渲染内容区;侧边栏窄窗口折叠为图标条(CSS 容器查询/媒体查询);弹窗与抽屉由 `ui/modal.ts`/`ui/drawer.ts` 管理叠层(Esc 关闭、脏表单关闭确认、焦点回归)。
- **Rationale**: FR-001/键盘可用性边界的集中实现点;路由器现有 hash 路由保留。

## 未知项清单(Phase 0 结论)

| Technical Context 未知项 | 结论 |
|---|---|
| 前端框架选型 | 原生 DOM + 组件函数(D1) |
| 匹配预览权威计算方 | 既有后端端点,零新增(D4) |
| 概览聚合端点 | dashboard 增量加 2 字段(D3) |
| 主题实现 | CSS 令牌 + data-theme(D2) |
| 轮询/相对时间/三态 | 集中组件(D5) |
| 徽章语义映射 | 单一映射表(D6) |

无遗留 NEEDS CLARIFICATION。
