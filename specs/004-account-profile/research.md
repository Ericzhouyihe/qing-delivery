# Research: 账号资料同步与管理增强(004-account-profile)

**Date**: 2026-09-25 | **Status**: Complete

## 代码库/参考项目核查(事实)

| 事实 | 位置 | 影响 |
|---|---|---|
| accounts 表无头像/备注列;display_name 在扫码时硬编码兜底 `闲鱼账号 {unb}` | migrations/0001、authorize.rs:114 | 迁移 0003 增列;授权完成处接入首拉 |
| 参考项目资料 API:`mtop.idle.web.user.page.nav` v1.0,GET 型,字段:module.base.{displayName,displayNick,avatar} | Ydisks internal/xianyu/mtop/user_profile.go:147,175-190 | 协议路径完整可循;昵称兜底链 displayName→displayNick |
| mtop 客户端已就绪:签名/重试/Cookie 吸收(MtopClient::call) | src/adapters/xianyu/mtop/client.rs | 资料调用零新增基建 |
| 参考项目在资料响应携带新 Cookie 时顺带更新凭证(account_profile.go:62-67) | 已有 MtopClient Set-Cookie 吸收 | 天然满足 FR-004,无需额外写 |
| 无删除账号端点/服务;订单等表以 account_id 外键关联 | routes.rs / migrations | 软移除:清凭证+值守+列表可见性,不动订单外键 |
| supervisor 每 tick 重读 runtime_enabled 账号列表 | supervisor.rs | 删除=置 runtime_enabled=0 + 状态 disabled → 值守自然停 |
| 前端已有脱敏函数 maskPlatformId、首字头像占位 | accounts/page.ts | 改完整 ID+复制;头像 img 加载失败回退现有占位 |

## 决策

### D1: 资料拉取走 mtop GET(api=mtop.idle.web.user.page.nav)
- Decision: MtopClient 增 GET 调用(现有 call 为 POST 签名型);解析 module.base.{displayName→displayNick 兜底, avatar};风控错误沿既有 Verification 分类自然转 003。
- Rationale: 参考项目验证过的端点;与我们 mtop 基建零摩擦。
- Alternatives: 复用 POST call 签名(参考为 GET query 型,签名字段集不同)。

### D2: 迁移 0003 增列(不建新表)
- Decision: accounts 增 `avatar_url TEXT`、`remark TEXT`;昵称复用 display_name(授权完成后首拉覆盖兜底名)。
- Rationale: 资料是账号的 1:1 属性;避免 join。

### D3: 删除=软移除三步(停值守→清凭证→隐藏)
- Decision: 端点 DELETE /accounts/{id}:①UPDATE accounts SET runtime_enabled=0,status='disabled'(值守 tick 自然停);②DELETE FROM account_credentials;③DELETE FROM accounts(订单表无强 FK 约束则物理删行;若 FK 受限则置 status='deleted' 过滤)。在途验证处置:删除前检查 VerificationService 活跃会话→先 clear。
- Rationale: 宪章 I 审计保留;敏感凭证清零(宪章 V);与 gate/验证管道协同。
- Alternatives: 硬删除全库数据(违宪章 I);仅停用(用户明确要"删除")。

### D4: 首拉时机=授权 complete 落库后异步触发
- Decision: AuthorizationService 在凭证保存后 tokio::spawn 一次资料拉取(失败仅日志+可手动刷新);手动刷新走同一服务方法。
- Rationale: 失败不阻塞接入(FR-001);复用同一路径。

### D5: 头像前端直接外链 + 失败回退
- Decision: <img src=平台URL onerror→隐藏,显示首字占位>;不下载落盘。
- Rationale: 假设已声明;防盗链时优雅回退。

### D6: 备注/ID 契约 additive
- PATCH /accounts/{id} {remark?} 存 remark;GET accounts items 增 avatar_url/remark 字段;前端 ID 完整显示+一键复制(navigator.clipboard)。

## 未知项结论
资料 API=mtop.idle.web.user.page.nav(D1);存储=迁移 0003 两列(D2);删除=软移除三步(D3);首拉异步(D4);头像外链回退(D5);备注 PATCH(D6)。无遗留 NEEDS CLARIFICATION;实账号字段名差异按 R11 模式校准。
