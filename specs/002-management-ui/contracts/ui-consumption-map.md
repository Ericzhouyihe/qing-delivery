# Contract: 界面 × API 消费图谱

**Date**: 2026-09-22 | **Spec**: FR-022(界面原则上仅消费既有 API)

除 dashboard 增量字段(见 [dashboard-api.md](dashboard-api.md))外,本特性消费的全部端点均为 001 既有契约,不修改。图谱约束每个页面可调用的端点集合,防止界面层越界直连数据。

## 认证与壳层

| 界面 | 端点 | 用途 |
|---|---|---|
| 初始化/登录卡片 | `POST /auth/initialize`、`POST /auth/login`、`GET /auth/session`、`POST /auth/logout` | 会话生命周期;401/403 自动回落登录并重登恢复原页面 |
| 全局 | `GET /capabilities` | 当前执行配置能力声明 → 不可用入口明示原因(FR-011) |

## 概览(30 秒可见性轮询 + 手动刷新)

| 端点 | 用途 |
|---|---|
| `GET /dashboard` | 统计卡(账号/今日订单/今日交付/待处理)、恢复横幅、stopping 提示 |
| `GET /accounts` | 账号状态速览(异常排序、心跳 monitoring_since) |
| `GET /orders?limit=10…` | 最近订单列表 |
| `GET /issues?…`(按需,limit=5) | 最近待处理摘要 |

## 账号页

| 端点 | 用途 |
|---|---|
| `GET /accounts` | 账号卡片网格 |
| `POST /accounts/qr-sessions`、`GET /accounts/qr-sessions/{qr_id}`、`…/cancel`、`GET …/image` | 扫码接入弹窗全流程(轮询/倒计时/过期重取/取消) |
| `POST /accounts/{id}/control` | 启停切换(确认流+过渡态) |
| `POST /accounts/{id}/verification` | 发起官方验证任务(响应含 instruction 与 browser_available;不可用时不伪称可用,FR-011) |
| `POST /accounts/{id}/item-syncs`、`GET /accounts/{id}/item-syncs` | 商品同步触发与结果(新增/更新/失败反馈) |

## 商品与发货规则页

| 端点 | 用途 |
|---|---|
| `GET /accounts/{id}/items` | 商品列表(客户端筛选:关键字/已配置) |
| `GET /accounts/{id}/rules`、`POST /accounts/{id}/rules`、`PUT …/rules/{rule_id}` | 规则行与编辑抽屉 |
| `POST /accounts/{id}/rules/match-preview` | 匹配预演判定(见 [match-preview-api.md](match-preview-api.md)) |
| `POST /accounts/{id}/rules/preview` | 最终发送内容渲染预览 |

## 订单中心与详情

| 端点 | 用途 |
|---|---|
| `GET /orders`(account_id/platform_state/paid_from/paid_to/cursor/limit) | 组合筛选+分页(翻页保留筛选) |
| `GET /accounts/{id}/orders/{order_id}` | 详情基本信息与三轴时间线/尝试记录 |
| `GET /accounts/{id}/orders/{order_id}/content` | 交付内容揭示(默认折叠,授权端点) |
| `POST …/resends`、`POST …/takeovers`、`POST …/terminate`、`POST …/mark-received` | 人工动作区(按 guard 启停;禁用附原因) |

## 待处理工作台

| 端点 | 用途 |
|---|---|
| `GET /issues` | 分组条目(类别/计数/等待时长/允许操作) |
| `GET /restore/reviews`、`POST /restore/reviews/{id}/decisions` | 恢复核对条目与决策(原因必填) |
| `POST /accounts/{id}/orders/{order_id}/…`(同订单人工动作) | unknown 核对允许的操作 |

## 设置页

| 端点 | 用途 |
|---|---|
| `GET /capabilities`、`GET /auth/session` | 运行信息/执行模式/会话剩余 |
| `GET /restore`、`POST /restore`(既有语义) | 备份/恢复指引与恢复确认流(后果说明) |

## 越界禁令(对实现的任务级约束)

- 前端不得调用本图谱之外的端点;需要新数据时先修订本契约而非绕行。
- 界面层不出现 SQL、平台协议解析或交付规则判定逻辑(宪章 II)。
- 页面关闭后核心交付持续运行——界面仅观察与发起人工操作,不承载交付状态机(宪章 II)。
