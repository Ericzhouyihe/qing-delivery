# Implementation Plan: 账号资料同步与管理增强(004-account-profile)

**Branch**: `004-account-profile` | **Date**: 2026-09-25 | **Spec**: [spec.md](spec.md)

## Summary

补齐账号管理 4 差距:①授权完成自动拉取平台资料(`mtop.idle.web.user.page.nav`,昵称 displayName→displayNick 兜底+头像外链),账号卡"刷新资料"可重试;②删除账号=软移除三步(停值守→清加密凭证→移除账号行,审计保留);③完整平台 ID+一键复制;④本地备注(PATCH 持久)。迁移 0003 增 avatar_url/remark 两列,全部 API additive,参考项目流程按宪章重新实现(研究 D1-D6)。

## Technical Context

**Language/Version**: Rust stable 2024(windows-gnu)/ TypeScript 7+Vite 8(增量)

**Primary Dependencies**: 无新增(mtop 客户端/chromiumoxide 等全复用)

**Storage**: SQLite 迁移 0003:accounts 增 `avatar_url TEXT`、`remark TEXT`(additive;SUPPORTED_SCHEMA→3)

**Testing**: cargo(基线 ≥146 不回退)+ 契约测试(PATCH/DELETE/profile-fetch)+ 前端 vitest(备注表单/复制/头像回退);实账号走查见 quickstart

**Target Platform**: Windows 本地;实账号资料字段按 R11 校准

**Constraints**: FR-004 Cookie 顺带保存复用 mtop 吸收;FR-005 风控转 003 管道;删除前清验证活跃会话;备注 ≤64 字符;删除后重扫码=新账号(UNIQUE 放行)

## Constitution Check

| # | 门禁 | Phase 0 | Phase 1 复检 |
|---|---|---|---|
| 1 | 范围 | PASS——仅账号资料/删除/ID/备注 | PASS——契约 additive,不含平台写操作 |
| 2 | 付款/去重/unknown | PASS——不触交付链 | PASS——删除只停值守,不动订单/guard |
| 3 | 分离/依赖 | PASS——零新增依赖 | PASS——资料走 mtop 适配层;软移除语义在应用层 |
| 4 | 可验证 | PASS——三层验证 | PASS——契约+行为+实账号走查(R11) |
| 5 | 敏感数据 | PASS——凭证清除即目的 | PASS——删除清凭证;头像外链不落盘;审计保留(宪章 I 优先) |
| 6 | 性能/兼容 | PASS——首拉异步 | PASS——迁移 0003 版本化+备份;基线锁定 |

**结论**: 无违例。

## Project Structure

```text
specs/004-account-profile/          # 本特性文档(plan/research/data-model/contracts/quickstart)
migrations/0003_account_profile.sql # avatar_url/remark 两列
src/adapters/xianyu/mtop/client.rs  # GET 型调用(user.page.nav;D1)
src/adapters/xianyu/profile.rs      # 资料拉取与解析(displayName/displayNick/avatar)
src/application/accounts/profile.rs # ProfileService(首拉/手动刷新/落库)+ removal.rs(软移除三步)
src/application/accounts/authorize.rs # complete 后 spawn 首拉(D4)
src/transport/accounts_api.rs       # PATCH/DELETE/profile-fetch + items 增字段
frontend/src/features/accounts/    # 头像 img+回退、完整 ID+复制、备注编辑、刷新资料按钮、删除确认流
tests/contract/accounts_profile_http.rs
```

**Structure Decision**: 沿用 001/002 分层;profile 域归入 application/accounts(与 qr/control 并列);不新建顶层模块。

## Complexity Tracking

无违例——本节为空。
