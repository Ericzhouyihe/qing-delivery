# Data Model: 管理界面重构(002-management-ui)

**Date**: 2026-09-22

本特性**不新增持久化实体**(规格假设:界面重构不新增业务数据存储)。本文档定义界面层实体、视图模型与 API DTO 的形状及校验规则;持久化模型仍以 001 的实现为准。

## 1. 持久化层变更(仅一处,增量)

| 表 | 变更 | 说明 |
|---|---|---|
| (无表结构变更) | 只读聚合查询 | `orders` 表新增聚合函数 `count_today(conn, since_ms) -> (orders_today, delivered_today)`,按 `paid_at >= 本地午夜` 统计;`content_state='delivered'` 计入 delivered。无写入、无迁移、无 schema 版本变化 |

## 2. API DTO(契约详情见 contracts/)

### DashboardSummary(既有端点 `GET /api/dashboard`,响应增量扩展)

| 字段 | 类型 | 来源 | 备注 |
|---|---|---|---|
| accounts[] | 既有 | accounts::list | 不变 |
| open_issue_count / active_job_count / restore / stopping / persistence | 既有 | dashboard(T075) | 不变 |
| **orders_today** | number | 新增聚合 | 本地时区自然日付款订单数 |
| **delivered_today** | number | 新增聚合 | 本地时区自然日内容状态=delivered 数 |

增量字段向后兼容:旧消费者忽略新字段,SC-206 不受影响。

### MatchPreviewResult(既有端点,本文档仅作消费方记录)

`{ result: "matched" | "none" | "ambiguous", rule_id?, rule_version? }` — 引擎精确匹配判定,UI 不重新解释匹配语义。

### RuleContentPreview(既有端点 `POST .../rules/preview`)

输入模板内容,输出占位符渲染后的最终内容预览;UI 不在本地实现占位符替换。

## 3. 界面层实体(无持久化,会话内状态)

### ThemePreference

- 属性:`mode: "light" | "dark"`。
- 规则:默认 light;记忆于 `localStorage`;初始化时读取,非法值回退 light(FR-002)。

### StatusTone(语义色映射,D6)

- 属性:`tone: "normal" | "warning" | "danger" | "neutral" | "info"`,`label: string`,`rawValue?: string`。
- 规则:账号状态与订单三轴状态的全部 001 枚举值必须有映射;未收录值 → `neutral` + label"未知状态" + rawValue 保留原始值(FR-021);映射表为全局唯一,两主题下 tone 可辨(FR-004)。

### LayoutShell(布局外壳,D7)

- 属性:`activeRoute`, `sidebarCollapsed: boolean`(窗口宽度驱动), `overlayStack: Modal | Drawer[]`(至多一层可交互)。
- 规则:登录后挂载一次;路由切换仅替换内容区;overlay 关闭时焦点回归触发元素,脏表单需确认(键盘可用性边界)。

### AsyncBlock 状态机(三态,D5)

- 状态:`loading → ready | empty | error`;`error →(retry)→ loading`;`ready/empty →(reload)→ loading`。
- 规则:loading 渲染骨架屏且占位尺寸与 ready 一致(无布局跳变,US2-6);empty 必含引导动作;error 必含重试(FR-003)。

### 表单校验状态(RuleEditor,FR-013)

- 字段级:`content` 必填、字数 ≤ 001 规则上限、占位符必须属于 001 定义的合法占位符集合;`delay` 数值范围取 001 既有约束。
- 状态:`pristine | dirty | invalid | submitting`;dirty 且非 submitting 时关闭抽屉需确认;invalid 时保存拒绝并显示行内错误。

## 4. 视图模型(每页组装,不落库)

| 视图模型 | 组装来源 | 关键属性 |
|---|---|---|
| OverviewView | dashboard + orders(最近 10)+ issues(最近 5) | 统计卡 4 张、恢复横幅、账号速览(异常优先、心跳时间)、最近订单(双轴标签)、待处理摘要、快捷动作 |
| AccountCard | accounts + qr-sessions | 昵称、脱敏平台号、StatusTone、run_enabled、心跳时间、监控范围标签 |
| CatalogView | items + rules | 商品行(筛选:账号/已配置/关键字)、规则行、同步结果(news/updates/failures 计数与失败明细) |
| OrderRow / OrderDetail | orders + 详情 + reveal(授权) | 双轴标签、三轴时间线节点[]、尝试记录[]、内容(折叠揭示)、guard 允许/禁止动作(含禁止原因) |
| IssueGroup | issues + restore/reviews | 类别、计数、条目(等待时长、风险文案、允许操作)、最久等待 |
| SettingsView | capabilities + installation 信息 | 版本、数据目录、监听地址、执行模式、会话剩余、最近备份、恢复后果说明 |

## 5. 实体关系

```text
LayoutShell ──渲染──> 各页 ViewModel ──消费──> API DTO(dashboard/orders/issues/accounts/catalog/restore)
     │                                        └─ 新增: DashboardSummary.orders_today/delivered_today(只读)
     ├── ThemePreference(localStorage)
     ├── AsyncBlock(每异步块一个实例)
     └── StatusTone(全局唯一映射表,被所有 ViewModel 引用)
```

## 6. 校验与不变量(来源于规格)

- 破坏性操作(停用账号、恢复备份、放行核对、重发)一律走确认流;提交中按钮锁定防重复(FR-005)。
- 改变交付结果的人工操作原因必填(FR-018)。
- 交付内容默认折叠,揭示仅经授权端点,不进入前端持久化状态(宪章 V;复用 001 FR-026 行为)。
- guard 禁止的动作以禁用态+原因呈现,禁止"点击后才报错"(FR-017)。
- 概览轮询仅页面可见时进行;tick 计时器页面隐藏即暂停(资源取向,宪章 III)。
