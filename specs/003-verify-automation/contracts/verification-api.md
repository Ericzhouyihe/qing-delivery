# Contract: 验证 API 与能力声明(003,additive)

**Date**: 2026-09-23 | **Change type**: Additive + 既有端点语义升级 | **Spec**: FR-001/002/005/006/011/012

## 1. POST /api/v1/accounts/{account_id}/verification(语义升级)

既有占位行为(返回指引)升级为真实触发,走 VerificationService 同一管道(手动触发=trigger_source=manual)。

**请求**: `{}`(自动触发不需要;信号由服务内部产生)

**202**:
```json
{
  "operation_id": "op-…",
  "session": { "id": "vs-…", "state": "detected", "trigger": "manual", "retry_count": 0 }
}
```

**错误**: `unsupported_capability`(无浏览器/驱动未接入,含指引文案,不伪称已尝试)| `resource_not_found`(账号不存在)| `conflict`(该账号已有活跃会话→返回既有会话状态)

## 2. GET /api/v1/accounts/{account_id}/verification(新增)

**200**:
```json
{
  "active": { "id": "vs-…", "state": "solving", "trigger": "mtop", "started_at": "…", "retry_count": 1 } | null,
  "recent_attempts": [
    { "id": "vat-…", "trigger_source": "mtop", "outcome": "failed_manual", "duration_ms": 87000,
      "credential_updated": false, "failure_reason": "滑块未通过×2", "verification_url": "…", "started_at": "…" }
  ]
}
```

state 枚举:`detected|browser_open|solving|updating_credential|succeeded|failed_manual`(与 data-model 状态机一致);前端 5 秒轮询活跃会话(仅存在时)。

## 3. capabilities 增量(既有响应 additive)

```json
{ "browser": { "available": true, "engine": "system-chromium", "reason": null } }
// 不可用时 { "available": false, "reason": "未检测到 Chrome/Edge,请安装后重试" }
```

界面:设置页浏览器策略卡、账号页验证入口按此如实启停(002 FR-011 延续)。

## 4. issues 新类别 security_verification

`GET /issues` 既有形状不变,新增 category 取值与 metadata:

```json
{ "id": "iss-…", "category": "security_verification", "reason": "滑块未通过×2",
  "allowed_actions": ["open_verification", "resolve"],
  "state": "open", "created_at": "…",
  "metadata": { "verification_url": "https://…", "attempt_ids": ["vat-…"] } }
```

- `open_verification`:前端以 metadata.verification_url 打开本机默认浏览器(人工验证);无后端调用。
- `resolve`:POST /issues 既有决议路径?——**不新增端点**:人工关闭走既有事项语义经 `POST /api/v1/issues/{id}/resolve`?001 无此端点;约定:`resolve` 仅前端标记意图,真正的关闭由**凭证恢复自动检测**完成(FR-005);若卖家明确点"我已处理",前端触发一次 `POST /accounts/{id}/verification`(轻量会话)做凭证探测——探测通过即自动关闭事项。此路径避免新增 mutations 端点。
- 自动关闭:failed_manual 存在时服务每 60s 探测凭证有效性,恢复→关闭事项+审计补记。

## 5. 内部信号事件(非 HTTP,实现契约)

`MessageKind::VerificationRequired { url: Option<String> }`(events.rs 增枚举);`MtopError::Verification` payload 结构化:`Verification { url: Option<String>, ret: String }`(**破坏性内部变更**,调用方仅 adapter 内部,随任务同步更新;HTTP 契约不变)。

## 6. 不变量与门禁

- 全部 HTTP 变更 additive(002 前端零破坏,SC-306);issues.metadata 旧行 NULL。
- verification_url 属敏感链接:仅授权会话可见,不写日志明文(宪章 V;日志记哈希/域名)。
- 自动触发不提供 HTTP 入口(内部信号驱动),防止外部滥用拉起浏览器。

## 7. 测试要求(宪章 IV)

- 契约测试:新端点鉴权(匿名拒绝/CSRF)、状态枚举、capabilities 形状、issues metadata。
- 行为测试:状态机全迁移、重试/超时/降级、闸门延迟交付、凭证失败保留旧值、冷却 ≥5min——全部确定性(fake driver,无浏览器)。
- 浏览器集成:夹具滑块页(tests/fixtures/slider-page.html)驱动真实 chromiumoxide——**有浏览器才跑**(环境变量 `QING_BROWSER_TESTS=1` 门控),无浏览器环境明确跳过并注明。
