# Data Model: 运营概览统计首页(005-operations-dashboard)

**Date**: 2026-09-25 | 本特性为**只读投影**:不新建表、不新增迁移,所有实体为查询期派生值。

## 领域类型(`src/domain/stats.rs`,纯值与纯函数)

### StatsRange(统计区间)

| 字段 | 类型 | 约束 |
|------|------|------|
| from_ms | i64 | UTC epoch 毫秒,含端点下界 |
| to_ms | i64 | UTC epoch 毫秒,开上界(= 结束日次日 00:00 或下一整点) |
| granularity | Granularity | 服务端按规则推导,不由客户端指定 |

- 校验规则:`to_ms > from_ms`;`to_ms - from_ms ≤ 92 天`(FR-009);违反即 `RangeError`(HTTP 400)。
- 预设→区间解析(前端执行,服务端复核):自然日对齐——今天=`[今日00:00, 明日00:00)`;昨天=前一自然日;三天内/7天内/一个月内=含今天共 3/7/30 个自然日(澄清 Q2)。

### Granularity(分桶粒度)

- `Hourly`:`to_ms - from_ms ≤ 48h`(spec FR-007);桶边界=本地自然小时。
- `Daily`:其余;桶边界=本地自然日 00:00(`time_util::local_midnight_ms` 同源逻辑,可回溯 N 天)。
- 派生函数 `bucket_ranges(&StatsRange) -> Vec<(i64, i64)>`:桶数上限 92(日)或 49(小时);末桶不足整日/整小时也成桶。

### RevenueSummary(营收汇总,响应投影)

| 字段 | 类型 | 口径(spec FR-010) |
|------|------|------|
| minor_units | i64 | 区间内 `paid_at ∈ [from,to)` 且 `currency='CNY'` 且 `amount_minor IS NOT NULL` 的合计;**不按退款/关闭状态过滤**(澄清 Q1) |
| currency | "CNY" | 固定;非 CNY 订单排除出合计 |
| order_count | i64 | 区间内 `paid_at ∈ [from,to)` 的订单数(不筛币种/金额/状态) |
| previous_minor_units | i64 | 前一等长区间 `[from-(to-from), from)` 同口径合计 |
| change_percent | i32 \| null | `(cur-prev)/prev*100` 四舍五入取整;`prev==0` 时 `null`(前端隐藏徽标) |

### TrendPoint(趋势点,响应投影)

| 字段 | 类型 | 口径 |
|------|------|------|
| bucket_start_ms | i64 | 桶起始(本地自然日 00:00 或自然小时)|
| minor_units | i64 | 桶内 CNY 营收合计(空桶=0) |
| order_count | i64 | 桶内订单数(不筛币种) |

- 序列必须覆盖区间内**所有**桶(空桶补零),保证图表横轴连续、Σbucket 营收 = RevenueSummary.minor_units(SC-002)。

### AccountActivitySnapshot(账号活跃快照)

| 字段 | 类型 | 口径 |
|------|------|------|
| online | i64 | `accounts` 中连接状态为 online 的行数(与账号页徽标同源 `AccountStatus::parse`) |
| total | i64 | accounts 总行数 |

- 与统计区间无关的"当前值"(spec Key Entities);随每次查询实时取。

### PendingIssues(待人工处理)

- 复用既有口径:`issues WHERE state='open'` 计数(与 `/api/v1/dashboard` 的 `open_issue_count` 一致)。

## 既有表引用(不修改)

- `orders(id, account_id, paid_at, amount_minor, currency, platform_status, …)`,索引 `idx_orders_paid_at`——退款/关闭表现为 `platform_status` 变化,**统计不读该列做过滤**。
- `accounts(id, status, runtime_enabled, …)`
- `issues(id, state, created_at, …)`,索引 `idx_issues_state_created`

## 状态转换

- 无。所有实体为无副作用查询投影;FR-012(只读)由服务不调用任何写路径保证,契约测试断言查询前后库内订单/事项状态不变。

## 验证规则汇总

1. `from < to`,跨度 ≤92 天,否则 400。
2. `from/to` 必须为可解析的 epoch 毫秒,否则 400。
3. `granularity` 仅由服务端推导;请求携带该参数无效(忽略或 400,契约取忽略+回显)。
4. 营收合计只认 CNY + 非空金额;订单数只认付款事实。
5. 空区间(无订单):合计 0、`change_percent=null`、趋势全零桶、前端空态(spec FR-008)。
