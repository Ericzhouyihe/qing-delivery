# Contract: 账号资料与管理 API(004,additive)

**Spec**: FR-001~011 | 基线: 002 契约不变

## GET /api/v1/accounts(items 元素增量)

```json
{ "id": "...", "display_name": "平台昵称或兜底", "platform_user_id": "完整ID",
  "avatar_url": "https://... | null", "remark": "主号 | null", "...": "既有字段不变" }
```

## PATCH /api/v1/accounts/{account_id}(新增)

请求 `{ "remark": "新备注" }`(null=清空;≤64 字符)→ 200 更新后账号摘要;404 账号不存在;400 超长。

## POST /api/v1/accounts/{account_id}/profile-fetch(新增)

触发一次资料拉取(手动刷新)→ 202 `{ "job": {...} }`(异步完成,前端轮询 GET accounts 观察昵称/头像变化);风控错误→ 409 转验证提示(前端引导至 003 流程)。

## DELETE /api/v1/accounts/{account_id}(新增)

破坏性确认由前端承担(输入 ID 片段)。→ 204 删除完成;409 存在活跃验证会话(先等处置结束);404 不存在。删除语义见 data-model(AccountRemoval)。审计数据保留,不可恢复的仅接入态(凭证/值守/列表)。

## 授权完成钩子(内部)

complete 落库+凭证保存后,自动 spawn 一次 ProfileFetch(失败仅日志;FR-001)。

## 测试要求(宪章 IV)

- 契约测试:PATCH 校验/404、DELETE 三步语义(凭证行 0 残留+订单保留)、profile-fetch 202、items 新字段。
- 行为测试:资料解析(displayName/displayNick 兜底/空)、删除后重扫码=新账号、备注持久。
