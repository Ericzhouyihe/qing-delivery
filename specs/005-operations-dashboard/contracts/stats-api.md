# Contract: 运营统计 API(005,additive)

**Spec**: FR-001~016 | 基线: 002 `/api/v1/dashboard` 契约不变,本契约为新增端点

## GET /api/v1/stats/overview(新增)

查询参数:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| from | int(epoch ms) | 是 | 区间起点(含),前端按本地时区自然日对齐解析 |
| to | int(epoch ms) | 是 | 区间终点(不含) |
| granularity | — | — | 忽略(服务端推导并回显) |

授权:管理员(与 GET /api/v1/dashboard 同级)。

响应 200:

```json
{
  "range": { "from": 1758720000000, "to": 1759324800000, "granularity": "daily" },
  "revenue": {
    "minor_units": 123450,
    "currency": "CNY",
    "order_count": 12,
    "previous_minor_units": 67890,
    "change_percent": 82
  },
  "accounts": { "online": 1, "total": 2 },
  "pending_issues": 4,
  "stopping": false,
  "restore": null,
  "trend": [
    { "bucket_start": 1758720000000, "minor_units": 5000, "order_count": 1 },
    { "bucket_start": 1758806400000, "minor_units": 0, "order_count": 0 }
  ]
}
```

口径(全部服务端保证):

- `revenue.minor_units`:`paid_at ∈ [from,to)` 且 `currency='CNY'` 且金额非空的合计;**退款/关闭订单不排除**(付款事实口径)。
- `revenue.order_count`:仅按 `paid_at` 付款事实计数,不筛币种/金额/状态。
- `change_percent`:`prev==0` 时为 `null`;否则整数四舍五入(可负)。
- `granularity`:`to-from ≤ 48h` → `"hourly"`,否则 `"daily"`;`trend` 覆盖全部桶,空桶补零;Σ`trend[].minor_units` == `revenue.minor_units`;Σ`trend[].order_count` == `revenue.order_count`。
- `range` 回显服务端复核后的 `from/to`。
- `stopping`/`restore`:服务停止与恢复隔离状态,随统计同响应返回,页面单请求装配(研究 R7);`restore` 为 null 表示未隔离,形状与 `/api/v1/dashboard` 的 `restore` 一致。

错误:

| 状态 | 场景 |
|------|------|
| 400 | from/to 缺失或非整数;to ≤ from;跨度 > 92 天 |
| 401 | 未登录管理员会话 |
| 500 | 持久化不可用(沿用既有错误信封 `{ "error": { "code": "persistence_unavailable", … } }`) |

不变式:

- 严格只读:请求前后 `orders`/`issues`/`accounts` 行与状态零变化(契约测试断言)。
- 响应字段对旧前端 additive;不改动任何既有端点。

## 前端消费契约(shared/contracts.ts)

- `parseStatsOverview(v: unknown): StatsOverview`:逐字段容错解析;`change_percent: null` 合法;`trend` 非数组时按空数组降级为"统计不可用"占位(不抛错中断整页)。
- 预设范围解析(纯函数 `resolvePresetRange(preset, nowMs): {from, to}`):今天/昨天/三天内/7天内/一个月内,本地自然日对齐;自定义由日期输入解析,跨度 >92 天由前端预校验并提示(不发请求)。

## 测试要求(宪章 II/III)

- 契约测试(`tests/contract/stats_http.rs`):鉴权 401;400 三态(倒置/超限/非数字);口径三例(CNY 过滤、缺失金额排除、退款不回冲);分桶正确(小时/日);只读不变式;空区间全零桶 + `change_percent=null`。
- 前端测试(vitest):`resolvePresetRange` 五种预设边界(含跨月/跨年);`parseStatsOverview` 容错;图表桶→SVG 节点映射;徽标 null 隐藏。
