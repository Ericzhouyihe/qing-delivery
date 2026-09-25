# Data Model: 账号管理页面复刻与视图切换(006-accounts-page-revamp)

**Date**: 2026-09-25 | **Spec**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md)

本特性**无新增持久化实体、无数据库迁移、无后端 DTO 变更**。全部数据为既有 `GET /api/v1/accounts` 载荷之上的展示层视图模型。

## 1. 既有源模型(只读消费,不修改)

### AccountCard(既有,`features/accounts/page.ts` 的 `parseAccountCard`)

`GET /api/v1/accounts` → `items[]` 的前端解析结果,本特性原样复用:

| 字段 | 类型 | 来源 | 用途 |
|------|------|------|------|
| `id` | string | `id` | 操作端点定位 |
| `platformUserId` | string | `platform_user_id` | ID 行展示、搜索匹配域 |
| `avatarUrl` | string \| null | `avatar_url` | 头像(破图回退首字) |
| `remark` | string \| null | `remark` | 备注行、搜索匹配域 |
| `displayName` | string | `display_name` | 名称行、搜索匹配域 |
| `connectionState` | string | `connection_state` | 徽章/圆点/警示条/排序 |
| `runEnabled` | boolean | `control.run_enabled` | 启停图标态 |
| `autoDelivery` | boolean | `control.auto_delivery_enabled` | 能力徽标"自动发货" |
| `autoConfirm` | boolean | `control.auto_confirm_enabled` | 能力徽标"自动平台确认" |
| `controlVersion` | number | `control_version` | 启停乐观并发 |
| `monitoringSince` | string \| null | `monitoring_since` | 心跳相对时间 |

验证会话状态继续经 `GET /api/v1/accounts/{id}/verification` 轮询(既有逻辑),产出"验证中/待人工验证"动态徽章,不属于本特性数据变更。

## 2. 新增展示视图模型(`features/accounts/model.ts`,纯函数)

### CapabilityTag —— 能力/状态徽标

```text
CapabilityTag { id: string, label: string, tone: "normal"|"warning"|"info"|"neutral" }
```

派生规则(`deriveCapabilityTags(a: AccountCard): CapabilityTag[]`,**只映射真实能力**,FR-008):

| 触发条件 | id | label | tone |
|----------|----|-------|------|
| `connectionState ∈ {verification_required, authorization_expired}` | `needs-verify` | 需要验证 | warning |
| `autoDelivery === true` | `auto-delivery` | 自动发货 | normal |
| `autoConfirm === true` | `auto-confirm` | 自动平台确认 | normal |

- 未列出的参考图徽标(AI、自动评价、每日擦亮)**无派生规则即不渲染**;连接状态徽章(在线/离线/连接中/已暂停/授权失效)沿用既有 `accountBadge()` 单独渲染,不并入此表。
- 测试断言:任意输入下输出 ⊆ 上表(不虚标不变量)。

### 搜索过滤 —— `filterAccounts(items, query)`

| 规则 | 值(规格 FR-003) |
|------|------------------|
| 匹配域 | `displayName`、`remark`、`platformUserId` |
| 匹配方式 | 子串包含;query 与匹配域均 `trim()`、`toLowerCase()` 后比较 |
| 空查询 | 返回全部(保持原顺序,排序由 `sortAccounts` 负责) |
| 复杂度 | O(n·m),n 为账号数(几十级),单帧内完成 |

### 排序 —— `sortAccounts(items)`

沿用既有异常优先级(不变):`verification_required/authorization_expired`(0) > `connecting`(1) > `online`(2) > 其余(3);同级按 `displayName.localeCompare` 稳定排序。规格 FR-006"异常置顶"由此承载。

### 计数 —— `viewCounts(filtered, total)`

`{ shown: filtered.length, total }` → 横幅"当前显示 shown / total 个账号"。

## 3. 本地偏好(非持久化实体)

### ViewPreference

| 项 | 值 |
|----|----|
| 存储 | localStorage 键 `qing-account-view`(沿用既有键,FR-010) |
| 取值 | `"row"` \| `"card"` |
| 读取回退 | 缺失或未知值 → `"row"`(兼容旧写入) |
| 写入时机 | 用户点击分段控件后立即写入 |

## 4. 状态 → 呈现映射(条目视觉,集中一处供两形态共用)

| `connectionState` | 头像圆点 | 连接徽章(既有) | 警示条 + 重新授权 | 图标操作组 |
|-------------------|----------|------------------|--------------------|------------|
| `online` | 绿(在线) | 在线(normal) | 无 | 刷新/编辑/停用/删除 |
| `connecting` | 灰 | 连接中(info) | 无 | 同上(停用态图标) |
| `verification_required` | 红 | 需验证(warning) | 显示(文案:闲鱼要求安全验证,请重新扫码并完成验证) | 刷新/重新授权(强调)/编辑/停用/删除 |
| `authorization_expired` | 红 | 授权失效(warning) | 显示(同一警示条组件) | 同上 |
| `offline` | 灰 | 离线(neutral) | 无 | 同上 |
| `paused` | 灰 | 已暂停(neutral) | 无 | 同上(启用态图标) |

未知状态:徽章走既有 `UNKNOWN` 兜底("未知状态"+原始值悬浮),圆点灰色,不崩溃(SC-205 惯例延续)。
