# Contract: GET /api/dashboard(增量扩展)

**Date**: 2026-09-22 | **Change type**: Additive(向后兼容)| **Spec**: FR-006/FR-007/US2

本特性唯一的 API 变更:在既有 dashboard 响应(T075)上**增量新增两个只读计数字段**。既有字段、语义、鉴权、错误码不变。

## 请求

```text
GET /api/dashboard
Cookie: qing_session=...        # 既有 admin 会话(不变)
```

无请求体;GET 不要求 CSRF 头(遵循 001 既有中间件行为)。

## 响应 200(扩展后)

```json
{
  "accounts": [
    { "id": "...", "display_name": "...", "connection_state": "online|offline|token_expired|paused|pausing|blocked",
      "run_enabled": true, "auto_delivery_enabled": true }
  ],
  "open_issue_count": 0,
  "active_job_count": 0,
  "persistence": "healthy",
  "stopping": false,
  "restore": null,
  "orders_today": 12,
  "delivered_today": 11
}
```

### 新增字段(本契约定义的唯一变更)

| 字段 | 类型 | 语义 | 计算口径 |
|---|---|---|---|
| `orders_today` | integer ≥0 | 今日付款订单数 | `orders.paid_at >= 本地时区当日 00:00:00`(与规格假设"本地时区自然日"一致) |
| `delivered_today` | integer ≥0,≤orders_today | 今日已自动交付数 | 同上时间窗内 `content_state = 'delivered'` 的订单数 |

### 不变量

- `delivered_today ≤ orders_today`(交付必先付款,宪章 I)。
- 计数为只读聚合,不产生写入、不触平台协议、不占用外部网络调用(聚合 SQL 位于 adapters/sqlite 仓储层)。
- 字段缺失(旧版本服务)时前端必须降级为"统计不可用"占位,不得显示 0 冒充真实计数。

## 错误(不变)

`503 {"error":{"code":"persistence_unavailable",...}}` — 摘要不可用(既有行为)。

## 测试要求(宪章 IV)

- Rust:聚合函数单测(跨午夜边界、零订单、delivered≤orders 不变量);dashboard 处理器集成测试断言新字段存在与类型。
- 前端:Vitest 对缺失新字段的响应走降级占位的断言。
