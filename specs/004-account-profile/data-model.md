# Data Model: 004-account-profile

**Date**: 2026-09-25

## 迁移 0003(accounts 增列,additive)

| 列 | 类型 | 语义 |
|---|---|---|
| avatar_url | TEXT(可空) | 平台头像外链;空=首字占位 |
| remark | TEXT(可空) | 卖家本地备注;不同步平台 |

昵称复用 display_name:授权后首拉覆盖兜底名(US1);历史兜底名在下次刷新资料时修正。

## 实体变化

- **AccountProfile(读模型)**: display_name(平台昵称)、avatar_url、external_user_id(完整 ID)、remark;来源=accounts 单表。
- **AccountRemoval(删除语义)**: 输入 account_id → ①停值守(runtime_enabled=0,status='disabled') ②清凭证(DELETE account_credentials) ③移除账号行(若 FK 阻塞则 status='deleted' 且列表过滤)。约束:执行前无活跃验证会话;输出=受影响行数。
- **资料拉取(ProfileFetch)**: account_id → mtop GET → {nickname(displayName||displayNick), avatar_url};风控→Verification 错误(转 003);成功后 UPDATE display_name/avatar_url;顺带 Cookie 由 mtop 客户端吸收保存(FR-004)。

## 不变量

- 删除后订单/交付/事项审计行完整保留;订单中心账号呈现退化为 ID 字符串。
- 同平台同 ID 重新扫码=create 新账号(既有 UNIQUE 约束在删除后放行)。
- 备注长度 ≤64 字符(输入层校验,超长拒绝并提示)。
