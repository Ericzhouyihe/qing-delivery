# Research: 账号管理页面复刻与视图切换(006-accounts-page-revamp)

**Date**: 2026-09-25 | **Spec**: [spec.md](spec.md) | 规格层无 NEEDS CLARIFICATION,本文记录技术决策。

## R1. 改版范围:纯前端,零后端改动

- **Decision**: 只改 `frontend/`;不新增、不修改任何 HTTP 端点、Rust 模块或数据库迁移。
- **Rationale**: 端点盘点确认既有能力已覆盖规格全部行为——`GET /api/v1/accounts`(列表,含 control 嵌套)、`POST /api/v1/accounts/{id}/profile-fetch`(刷新资料)、`GET/POST /api/v1/accounts/{id}/verification`(验证会话与轮询)、`PATCH /api/v1/accounts/{id}`(备注)、`POST /api/v1/accounts/{id}/control`(启停/自动化开关,带 expected_version)、`DELETE /api/v1/accounts/{id}`、扫码接入会话端点(既有 qr-modal)。搜索按规格 Assumptions 在已加载列表上做本地过滤,个人卖家账号规模(几十以内)无需后端检索。宪章 III:不为显示层需求扩大服务面。
- **Alternatives considered**:
  - 后端加 `?q=` 查询参数——为几十条记录增加服务端接口与契约测试负担,收益为零;规格已把规模约束写入假设;弃(记录为未来账号量级增长后的可选演进)。

## R2. 图标方案:自绘内联 SVG 图标表

- **Decision**: 新增 `frontend/src/ui/icons.ts`,以字符串路径提供约 10 个 24×24 线性图标(search、qr、refresh、shield、edit、power、trash、list、grid、calendar 预留),渲染为 `<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">`,颜色继承 `currentColor`。
- **Rationale**: 参考图的操作组是彩色线性图标,现有 Unicode 字形(⟳ 🛡 ✎ ⏻ 🗑)跨平台渲染不一致且无法做出参考图的视觉质感。内联 SVG:①零依赖零体积负担(宪章 III),对比图标动辄数百 KB;②是 DOM 节点,jsdom 可断言存在性与 aria 属性(宪章 IV);③`currentColor` 让悬停变色免费获得;④自绘路径不复制参考项目任何资源(宪章 V 许可边界)。
- **Alternatives considered**:
  - 图标库(lucide/tabler/iconify)——引入依赖与构建面,且只用其中几个图标;弃。
  - 保持 Unicode 字形——无法表达二维码、滑块等语义,Windows/浏览器差异大,"完美复刻"不成立;弃。
  - 图片资源(sprite/字体图标)——额外请求与许可义务;弃。

## R3. 过滤/排序/标签派生:展示层纯函数模型

- **Decision**: 新增 `features/accounts/model.ts`,导出纯函数:`filterAccounts(items, query)`(昵称/备注/账号 ID 不区分大小写子串匹配,query trim)、`sortAccounts(items)`(异常置顶优先级沿用现 `priority()`:需验证/授权失效 > 连接中 > 在线 > 其余,同级按昵称稳定排序)、`deriveCapabilityTags(a)`(真实能力徽标派生,见 R6)、`viewCounts(filtered, total)`。`page.ts` 只装配。
- **Rationale**: 这些逻辑是 SC-001/SC-002/FR-003/FR-008 的验收对象,抽成纯函数后 vitest/jsdom 直接断言,无需起服务;`page.ts` 现已 600+ 行,把"算"与"摆"分离控制复杂度;符合宪章 II"页面只负责交互调用"。
- **Alternatives considered**: 逻辑继续内联在 page.ts——现有写法已导致行/卡片两份渲染各自为政,测试只能间接覆盖;弃。

## R4. 视图切换控件:双图标分段控件 + 既有偏好键

- **Decision**: 新增 `ui/segmented.ts`:容器 `role="group"`,两个 `<button type="button">` 分别代表列表/卡片,`aria-pressed` 表达激活态、`title` 提示文字、激活项高亮(令牌变量控制);点击发出 `onSelect(view)` 回调,控件不自行持久化。偏好读写留在 `page.ts`,沿用既有键 `qing-account-view`("row" | "card"),读入时对未知旧值回退 "row"。
- **Rationale**: 用户批注"只显示图标不显示文字,做的好看一点"——分段控件让两个目标视图常驻可见(当前项高亮),比"单按钮显示目标视图"更易懂,也是参考级 UI 的通行形态;`aria-pressed` + `title` 满足 FR-009 的激活高亮与悬停提示。沿用既有键保证 FR-010 且老用户偏好不丢。
- **Alternatives considered**:
  - 单图标按钮(点击切换到另一视图)——图标语义含糊(显示的是当前还是目标?),与批注"做的好看"有差距;弃。
  - 写 URL query 支持深链——超出规格;弃。
  - 新 localStorage 键——无必要,迁移成本为负;弃。

## R5. 条目重构策略:两形态共享装配,行为零回退

- **Decision**: `page.ts` 保留 row/card 两个条目形态,但把公共片段抽为共享装配函数:头像+状态圆点、徽标行(`deriveCapabilityTags` + 既有 `accountBadge` 连接徽章)、备注/ID 行、需验证警示条+行内"重新授权"、纯图标操作组(统一经 `iconBtn` 工厂创建,内联 SVG + title + 危险色变体)。既有行为逐项保留:编辑弹窗、删除确认、启停确认流(`applyControl` 与"暂停中"过渡态)、验证会话轮询徽章、头像破图回退首字、ID 复制(卡片形态)。
- **Rationale**: FR-012 要求既有能力零回退;当前 `renderRow`/`renderCard` 各写一遍类似片段,是"复刻不到位"的根因之一;共享装配后参考图视觉只调一处。轮询定时器清理沿用既有 MutationObserver 惯用法。
- **Alternatives considered**: 只改 CSS 不动结构——警示条位置、徽标行、图标组在两形态中 DOM 结构不同,CSS 无法拉齐;弃。
  - 整页重写——丢弃已验证的确认流/轮询逻辑,回归风险大;弃。

## R6. 能力徽标映射(FR-008 如实展示)

- **Decision**: 徽标行 = 连接状态徽章(既有 `accountBadge`:需验证/授权失效/连接中/在线/离线/已暂停) + 能力徽标:`自动发货`(a.autoDelivery,开启时正常色/关闭不显示)、`自动平台确认`(a.autoConfirm,同上);在线状态由头像圆点表达不重复出徽标;"暂停中"过渡徽章沿用既有逻辑。不渲染 AI、自动评价、每日擦亮等未实现能力;未来能力上线按同一 `deriveCapabilityTags` 扩展。
- **Rationale**: 宪章 IV"通过编译或拥有界面不代表发货可用"、004 FR-011"不伪称可用"先例;徽标是能力声明,虚标会误导卖家信任自动化行为。验证会话进行中的"验证中/待人工验证"动态徽章沿用既有轮询逻辑,叠加在徽标行尾。
- **Alternatives considered**: 为对齐参考图展示灰色"未开通"占位徽标——制造能力存在的暗示,且占三列空间无信息量;弃。

## R7. 页头与壳层的关系

- **Decision**: 页头(标题"账号管理"+副标题)渲染在账号页内容区顶部;顶栏 `.page-title`("账号")与侧边导航文案不动(规格 Assumption:全局布局不在范围内)。
- **Rationale**: 壳层标题由路由表统一驱动(`router.ts`),改动会外溢到其他页面;参考图的层级感由内容区页头自足达成,与概览页(005)标题区同构,风格可复用。
- **Alternatives considered**: 改路由 title 为"账号管理"——顶栏与页内标题重复且影响全局;弃。

## R8. 空态与无匹配的区分

- **Decision**: 三种空态各自独立:①账号总数 0 → 既有 `empty-state` 引导接入(不动);②有账号但搜索无匹配 → 列表区显示"无匹配账号"轻空态(图标+文案+清空搜索按钮),横幅计数为"当前显示 0 / N 个账号";③加载/错误 → 既有 `asyncBlock` 骨架与错误重试占位。
- **Rationale**: 规格 Edge Cases 明确要求①②可区分;清空搜索按钮把用户从死路一步拉回,成本一行代码。
- **Alternatives considered**: 无匹配时复用整页空态文案——语义混淆(FR-011);弃。

## R9. 验证策略:单测 + 门禁 + 对照清单

- **Decision**: 自动化门禁 = `npm run typecheck` + `npm run test`(新增 model/segmented/icons 行为测试,含 FR-008 不虚标断言、FR-003 过滤规则断言、FR-010 偏好回退断言)+ `npm run build`;SC-001 视觉齐全率用 quickstart 的对照核对表手动核对(逐元素打勾),不引入截图 diff 工具。
- **Rationale**: 前端改动宪章门禁即"类型检查、行为测试和构建";视觉"完美复刻"的最终裁判是人眼对照参考截图,自动截图比对在本地单机场景投入产出比低(宪章 III 以测量决定优化)。
- **Alternatives considered**: Playwright 截图对比——新依赖、新浏览器面、基线维护成本;弃。
