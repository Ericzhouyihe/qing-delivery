# Implementation Plan: 运营概览统计首页(005-operations-dashboard)

**Branch**: `005-operations-dashboard` | **Date**: 2026-09-25 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/005-operations-dashboard/spec.md`

## Summary

把概览首页改造成 Ydisks 风格的运营统计页:时间范围筛选(预设自然日对齐 + 自定义 ≤92 天)、四张统计卡(区间营收+对比徽标、活跃账号/总数、区间订单数、待人工处理)、营收趋势图(≤48h 按小时、其余按本地自然日分桶)。后端按宪章 II 的分层新增只读统计能力:`src/domain/stats.rs` 定义区间/分桶/口径(付款事实口径、仅 CNY、退款不回冲),`src/adapters/sqlite/repos/stats.rs` 提供只读查询,`src/application/stats.rs` 组装用例,传输层新增薄 handler `GET /api/v1/stats/overview`(additive,不动既有 `/api/v1/dashboard` 契约)。前端为原生 TS DOM,图表用手写 SVG(不新增运行时依赖),既有运营区块(账号速览/最近待处理/最近订单)保留在统计区下方。

## Technical Context

**Language/Version**: Rust 2024 edition(后端,当前工具链编译通过即可)+ TypeScript 7.0.2(前端,`type: module`)

**Primary Dependencies**: axum 0.8.9、rusqlite 0.40.2(bundled)、tokio 1.53、chrono 0.4(Local 时区)、serde/serde_json;前端 vite 8.3 + vitest 3.2(无 UI 框架,原生 DOM)

**Storage**: SQLite 单文件(`QingData/main.db`),经 `DbThread::call(|conn| ...)` 线程池访问;复用既有 `orders(paid_at, amount_minor, currency)`(已有 `idx_orders_paid_at`)、`accounts`、`issues` 表,**无新表、无迁移**

**Testing**: `cargo test`(tests/contract/*.rs 契约测试 + 模块内 `#[cfg(test)]` 单测)+ `cargo fmt --check` + `cargo clippy --all-targets`(宪章交付门禁);前端 `npm run test`(vitest)+ `npm run typecheck` + `npm run build`

**Target Platform**: Windows 本地单进程桌面服务(后端嵌入前端构建产物,浏览器管理页)

**Performance Goals**: 92 天区间、1 万订单规模下统计接口 <3s 返回(SC-003);预期实际 <100ms(SQLite 聚合 + 已有索引)

**Constraints**: 统计严格只读(FR-012),不触碰交付/值守路径;聚合口径在领域层定义,HTTP 层无 SQL(FR-011/宪章 II);接口需管理员授权(FR-016);additive 演进,不破坏 002 冻结契约

**Scale/Scope**: 单机单卖家,账号数 ≤10,订单量万级/年;单页面改版 + 1 个新端点 + 1 个新应用服务

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 结论 | 依据 |
|------|------|------|
| I. 以已付款订单和可恢复交付为核心 | ✅ 通过 | 统计只读,基于 `paid_at` 付款事实,不推断状态;不改变交付/去重/恢复语义;退款不回冲已在规格澄清固定 |
| II. 平台接入与发货业务分离 | ✅ 通过 | 新增 `domain/stats`(口径与分桶规则)→ `application/stats`(用例)→ `repos/stats`(只读查询)→ `transport/stats_api`(鉴权+调用,无 SQL)。现状债:`dashboard_api.rs` 内联 SQL——本特性不沿用该模式、不扩大它 |
| III. 轻量部署,以测量决定优化 | ✅ 通过 | 零新增运行时依赖(图表为手写 SVG);模块化单体、本地 SQLite;无 Redis/中间件/云服务;数据访问经仓储模块与既有事务边界 |
| IV. 关键交易行为必须可验证、可追溯 | ✅ 通过 | 契约测试覆盖口径矩阵(未付款/币种/金额缺失/退款不回冲)与只读不变式;前后端契约有明确类型与运行时校验(contracts.ts parseStatsOverview);统计加载守卫丢弃过期响应;性能实测记录于证据文档 |
| V. 本地数据边界与参考源码隔离 | ✅ 通过 | 服务默认回环绑定(沿用既有安装);统计响应仅含聚合数字,无凭证/Cookie/明文敏感数据;不依赖或打包上游参考目录 |

**门禁六问逐项结论**(宪法「开发流程与交付门禁」):

1. 范围符合:仅新增只读统计端点与概览页改版,未触及未支持平台/交易类型的执行路径。
2. 付款依据:统计口径基于 `paid_at` 付款事实(澄清 Q1);不改变持久化去重/恢复语义。
3. 平台与业务分离:domain/application/repos/transport 四层新增,零新增部署依赖。
4. 关键测试:契约测试覆盖口径矩阵与只读不变式;本特性为纯本地只读查询,无实账号平台操作,实账号验证不适用。
5. 敏感数据:响应仅含聚合数字与账号在线计数;回环绑定与访问控制沿用既有安装。
6. 性能测量:T019 记录 1 万订单、30 自然日窗口实测值(以"一个月内"更宽窗口覆盖 SC-003 字面"7天内"场景,为超集验证);SQLite 单文件,无业务数据兼容问题。

**Post-Phase 1 复核**: 数据模型为纯读取投影(无新实体表)、契约 additive、前端无新依赖——三原则维持通过,无需 Complexity Tracking 豁免。

## Project Structure

### Documentation (this feature)

```text
specs/005-operations-dashboard/
├── plan.md              # 本文件
├── research.md          # Phase 0:技术决策与依据
├── data-model.md        # Phase 1:统计投影实体与口径
├── quickstart.md        # Phase 1:端到端验证指南
├── contracts/
│   └── stats-api.md     # Phase 1:GET /api/v1/stats/overview 契约
└── tasks.md             # Phase 2 输出($speckit-tasks 生成)
```

### Source Code (repository root)

```text
src/
├── domain/
│   ├── stats.rs                 # [新增] StatsRange/Granularity/分桶规则/营收口径(纯函数,可单测)
│   └── time_util.rs             # [扩展] 本地自然日/自然小时边界辅助(沿用 local_midnight_ms 惯用法)
├── application/
│   └── stats.rs                 # [新增] StatsService:组装营收+账号+事项+趋势(db.call + repos)
├── adapters/sqlite/repos/
│   └── stats.rs                 # [新增] 只读聚合查询(区间营收/对比/趋势原料/账号快照/开放事项数)
└── transport/
    ├── stats_api.rs             # [新增] GET /api/v1/stats/overview 薄 handler(鉴权+参数解析+调用服务)
    └── routes.rs                # [修改] 注册新路由(1 行)

frontend/src/
├── features/overview/
│   ├── page.ts                  # [改版] 标题区+范围筛选+四卡+趋势图;保留既有运营区块
│   ├── model.ts                 # [扩展] 范围解析(预设→from/to)、卡片/趋势视图模型、徽标派生
│   └── model.test.ts            # [扩展] 上述纯函数行为测试
├── shared/contracts.ts          # [扩展] StatsOverview 类型与 parse 函数(容错旧服务)
├── shared/contracts.test.ts     # [新增] 契约解析测试(与既有共享测试同层)
└── ui/
    └── chart.ts                 # [新增] SVG 柱状/折线趋势图(无依赖,纯 DOM,可测)

tests/contract/
└── stats_http.rs                # [新增] 契约测试:鉴权、参数校验、口径(退款不回冲/CNY 过滤)、分桶
```

**Structure Decision**: 沿用既有模块化单体分层(domain/application/adapters/transport + 原生 TS 前端),不引入新顶层目录;统计作为独立领域模块挂载,与 004 的 `application/accounts/*` 同构。

## Complexity Tracking

> 无违例,无需豁免。
