# Implementation Plan: 管理界面重构(002-management-ui)

**Branch**: `002-management-ui` | **Date**: 2026-09-22 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/002-management-ui/spec.md`

## Summary

把 001 已可用但裸表格的管理界面重构为成体系的管理后台:统一布局外壳(侧边栏/顶栏/卡片化)+ CSS 设计令牌主题体系 + 通用组件词汇(徽章/抽屉/弹窗/时间线/三态块),并重写六个页面与登录/初始化体验。**后端唯一改动**是在既有 `GET /api/dashboard` 响应上增量新增 `orders_today/delivered_today` 两个只读计数字段;匹配预览完全复用既有引擎端点(T059)。技术路线(研究结论 D1-D7,见 [research.md](research.md)):沿用 TypeScript + Vite + 原生 DOM 组件函数模式,不引入前端框架;交付安全不变量与 API 契约零破坏(SC-206)。

## Technical Context

**Language/Version**: Rust stable 2024(windows-gnu 工具链,rust-toolchain.toml 锁定)/ TypeScript 7.0.2 + Vite 8.3.0 + Vitest

**Primary Dependencies**: 后端不变(Axum 0.8、rusqlite 0.40、Tokio;无新增 crate);前端无新增 npm 依赖(原生 DOM + CSS 自定义属性)

**Storage**: SQLite(既有,无 schema 变更;仅新增只读聚合查询 `count_today`,见 [data-model.md](data-model.md))

**Testing**: `cargo test`(基线 97 项不得回退)+ `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check`;前端 `npm run typecheck` / `npm run test`(Vitest 行为测试)/ `npm run build`

**Target Platform**: Windows 本地服务 + 桌面浏览器(最新版 Edge/Chrome/Firefox;不做移动端完整适配)

**Project Type**: 本地 Web 服务(单体) + 嵌入式 WebUI(include_dir 编译进二进制)

**Performance Goals**: SC-207——前端构建产物压缩后 ≤1.5MB;概览页首屏可交互 ≤2 秒(本地);30 秒轮询不产生布局跳变

**Constraints**: FR-022——不改既有 API 契约语义(dashboard additive 除外);交付不变量(unknown 不自动重发、guard 互斥、重试预算)在界面上的呈现必须与引擎语义一致;发布产物不依赖 Node/参考目录(宪章 V)

**Scale/Scope**: 六个页面 + 登录/初始化;单管理员;概览轮询 30s;订单分页 50/页(沿用 001)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

依据宪章(2.0.0)「开发流程与交付门禁」六项逐条检查;Phase 0 前与 Phase 1 后各评一次。

| # | 门禁 | Phase 0 评估 | Phase 1 复检(设计后) |
|---|---|---|---|
| 1 | 符合当前范围;未支持平台/交易类型明确拒绝 | PASS——仅 001 已定义六页;FR-011 要求不可用能力明示原因 | PASS——消费图谱(contracts/ui-consumption-map.md)未越 001 边界;匹配预演无自由文本模糊匹配(拒绝超界方案,D4) |
| 2 | 可信付款依据/身份关联/持久化去重/未知结果处理 | PASS——不触碰触发、匹配执行与重试路径 | PASS——界面只观察与发起人工操作;guard 禁止动作禁用+原因(FR-017);未知枚举兜底(FR-021) |
| 3 | 平台与业务分离;新部署依赖有必要证据 | PASS——零新增依赖(D1/D2) | PASS——聚合 SQL 在 adapters 仓储层、transport 仅序列化(D3);前端无 SQL/协议/规则判定(消费图谱越界禁令) |
| 4 | 关键测试/实账号边界/日志与人工路径 | PASS——纯 mock 验证(quickstart) | PASS——徽章映射、三态、确认流、dashboard 新字段均有测试要求(契约/data-model);不消费真实账号 |
| 5 | 敏感数据保护;参考目录隔离 | PASS——内容揭示沿用授权端点 | PASS——揭示不进前端持久化;预演内容仅在 admin 会话呈现;Ydisks 目录不进构建与打包 |
| 6 | 性能测量条件/升级影响/数据兼容 | PASS——SC-207 量化 | PASS——SC-207 明确本地环境口径;dashboard additive 向后兼容;无 schema 迁移 |

**结论**: 无违例,无需 Complexity Tracking 条目。

## Project Structure

### Documentation (this feature)

```text
specs/002-management-ui/
├── plan.md              # This file ($speckit-plan command output)
├── research.md          # Phase 0 output ($speckit-plan command)
├── data-model.md        # Phase 1 output ($speckit-plan command)
├── quickstart.md        # Phase 1 output ($speckit-plan command)
├── contracts/           # Phase 1 output ($speckit-plan command)
│   ├── dashboard-api.md
│   ├── match-preview-api.md
│   └── ui-consumption-map.md
└── tasks.md             # Phase 2 output ($speckit-tasks command - NOT created by $speckit-plan)
```

### Source Code (repository root)

```text
frontend/src/
├── main.ts                    # 入口:主题引导 + 路由启动(重写)
├── app/
│   ├── router.ts              # hash 路由(保留,挂载 shell)
│   ├── shell.ts               # 新增:布局外壳(侧边栏/顶栏/内容区/主题切换/登出)
│   ├── session.ts             # 会话引导(保留)
│   └── auth.ts                # 初始化/登录品牌卡片(重写,US8)
├── ui/                        # 新增:通用组件词汇表(D5/D6/D7)
│   ├── dom.ts                 # 基于 shared/dom.ts 的组件工厂扩展
│   ├── badge.ts               # StatusTone 唯一映射 + 未知枚举兜底
│   ├── states.ts              # asyncBlock 三态(骨架/空态/错误重试)
│   ├── modal.ts / drawer.ts   # 叠层管理(Esc/脏确认/焦点回归)
│   ├── timeline.ts            # 三轴状态时间线
│   ├── form.ts                # 行内校验表单控件
│   ├── time.ts                # 相对时间/等待时长 tick(页面隐藏暂停)
│   └── poll.ts                # 可见性门控轮询(30s)
├── features/                  # 六页重写(消费图谱限定端点)
│   ├── overview/  accounts/  catalog/  orders/  issues/  settings/
├── shared/
│   ├── http.ts / contracts.ts # 保留;contracts 增补 dashboard 新字段类型
│   └── dom.test.ts            # 既有测试适配
└── styles/
    ├── tokens.css             # 双主题设计令牌(D2)
    ├── base.css / components.css / pages.css

src/                           # 后端(唯一触点)
├── adapters/sqlite/repos/orders.rs   # +count_today 只读聚合(带单测)
└── transport/dashboard_api.rs        # 响应序列化 +2 字段(带集成测试)

frontend/src/…(tests)          # Vitest:徽章映射全枚举/三态/确认流/预览呈现
tests/                         # 既有 e2e 回归不动(SC-206)
```

**Structure Decision**: 沿用 001 确立的「frontend(TS)+ src(Rust 分层单体)」双树结构;前端新增 `ui/`(组件词汇)与 `styles/`(令牌)两个纯增量目录,features 由单文件升级为目录以便承载页面+测试;后端零新文件、零新依赖。构建链不变:`vite build → src/webui → include_dir 嵌入`(webui.rs 的重建跟踪修复保持有效,quickstart §性能)。

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

无违例——宪章检查六项双轮 PASS,本节为空。
