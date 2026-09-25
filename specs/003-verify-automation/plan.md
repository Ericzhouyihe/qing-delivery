# Implementation Plan: 安全验证自动化(003-verify-automation)

**Branch**: `003-verify-automation` | **Date**: 2026-09-23 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/003-verify-automation/spec.md`

## Summary

为长时值守补上平台安全验证(滑块风控)的自动处置能力:三源信号(mtop/WS/QR)→ VerificationService 状态机(重试 2 次/超时 120s/降级转人工)→ chromiumoxide 驱动系统浏览器以人类轨迹完成滑块 → 凭证加密回写+会话热更新 → 账号自动恢复;失败 100% 降级为含一键验证链接的待处理事项,人工完成后 60 秒自动检测恢复。浏览器单例管理(并发 1/空闲 5 分钟回收/退出全清理)。全程本机,默认禁用远程打码(研究 D1-D8,见 [research.md](research.md));交付安全不变量零改动,全部 API additive。

## Technical Context

**Language/Version**: Rust stable 2024(windows-gnu)/ TypeScript 7 + Vite 8(前端仅增量)

**Primary Dependencies**: chromiumoxide 0.9.1(**已在依赖树,零新增 crate**);后端其余不变;前端无新增 npm 依赖

**Storage**: SQLite 迁移 0002(版本化,沿用受保护备份机制):新增 `verification_attempts` 审计表(只增)+ `issues.metadata` 列(json,存 verification_url);见 [data-model.md](data-model.md)

**Testing**: `cargo test`(基线 211 项不得回退)+ clippy -D warnings + fmt;前端 typecheck/vitest/build;浏览器集成测试经 `QING_BROWSER_TESTS=1` 门控(自建滑块夹具页,无浏览器环境明确 SKIP);实账号端到端属授权项 R11

**Target Platform**: Windows 本地 + 本机 Chrome/Edge(启动探测;缺失如实报指引,不打包浏览器)

**Project Type**: 本地服务常驻子系统(验证处置不依赖管理页打开)

**Performance Goals**: SC-307——处置中浏览器峰值内存 ≤500MB;空闲回收 ≤5 分钟;单次处置(不含人工)≤3 分钟;信号识别→开始处置 ≤10 秒

**Constraints**: FR-009 全程本机、默认禁远程打码;FR-013 不改交付不变量;FR-014 凭证复用现有加密存储、失败保留旧值;全部 HTTP additive(SC-306);Ydisks 仅参考流程,移植实质代码须先核许可

**Scale/Scope**: 单管理员;多账号风控排队(并发 1);验证会话单账号至多 1 个活跃

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | 门禁 | Phase 0 | Phase 1 复检(设计后) |
|---|---|---|---|
| 1 | 范围;未支持能力明确拒绝 | PASS——仅验证自动化;无浏览器时明确报不支持 | PASS——capabilities 如实声明;mock 构建 Driver=None 端点如实报 |
| 2 | 付款依据/去重/unknown 处理 | PASS——不触碰触发与交付状态机 | PASS——闸门仅延迟触发不改状态(FR-013);验证失败不产生任何交付副作用 |
| 3 | 平台与业务分离;新依赖必要证据 | PASS——零新增 crate | PASS——VerificationDriver 对象安全端口隔离 CDP;重试策略在应用层 |
| 4 | 关键测试/实账号边界/日志与人工路径 | PASS——三层验证策略 | PASS——确定性(fake driver)+夹具页+实账号 R11 分层;降级人工路径 100% 可构造 |
| 5 | 敏感数据;参考目录隔离 | PASS——凭证走既有加密 | PASS——verification_url 不入日志明文;Ydisks 不入构建/打包(仅阅读参考) |
| 6 | 性能测量条件/升级影响/兼容 | PASS——SC-307 量化 | PASS——迁移 0002 版本化+备份;HTTP additive;浏览器资源预算明确 |

**结论**: 无违例,无需 Complexity Tracking 条目。

## Project Structure

### Documentation (this feature)

```text
specs/003-verify-automation/
├── plan.md              # 本文件($speckit-plan 输出)
├── research.md          # Phase 0:D1-D8 决策
├── data-model.md        # Phase 1:状态机/迁移 0002/内存实体
├── quickstart.md        # Phase 1:三层验证指南
├── contracts/
│   └── verification-api.md   # 端点升级+新端点+capabilities+issues 类别
└── tasks.md             # Phase 2($speckit-tasks 生成)
```

### Source Code (repository root)

```text
migrations/0002_verification.sql        # 审计表 + issues.metadata(版本化迁移)

src/application/verification/
├── mod.rs            # VerificationSignal、SolveOutcome、VerificationDriver(对象安全端口)
├── service.rs        # 状态机/去重合并/重试/超时/冷却/降级/审计写入/凭证恢复探测
└── gate.rs           # per-account 交付闸门(supervisor handoff 前检查)

src/adapters/browser/
├── discover.rs       # 系统 Chrome/Edge 探测(路径+注册表)
├── manager.rs        # BrowserManager:队列(并发1)/空闲回收/退出清理/崩溃重建
├── cdp.rs            # chromiumoxide 实现 VerificationDriver(会话/user-data-dir/锁检测)
└── slider.rs         # 滑块定位 + 人类轨迹拖动 + 成功判定

src/adapters/xianyu/
├── mtop/client.rs    # Verification 载荷结构化(携带 URL;内部破坏性,随调用方更新)
└── events.rs         # MessageKind::VerificationRequired{url}

src/runtime/supervisor.rs   # 闸门接线(handoff 前检查)+ 凭证热更新钩子
src/transport/
├── accounts_api.rs   # POST verification 升级 + GET verification(会话/审计)
└── routes.rs         # capabilities 增 browser 块
src/main.rs           # build_runtime 增挂 VerificationService/Driver(mock=None)

tests/
├── fixtures/slider-page.html      # 自建滑块夹具页
├── browser_integration.rs         # QING_BROWSER_TESTS 门控
├── browser_lifecycle.rs           # 回收/清理/队列
└── contract/verification_http.rs  # 新端点契约

frontend/src/features/
├── accounts/   # 验证徽章(验证中/待人工)+ 发起验证入口(能力门控)
├── issues/     # security_verification 分组(链接打开+我已处理)
└── settings/   # 浏览器策略卡(available/engine)
```

**Structure Decision**: 完全沿用既有分层(ports→application→adapters→transport)与对象安全驱动注入模式(QrDriver/ItemSyncDriver 同型第三例);adapters/browser 为新适配目录(平台无关,未来其他平台复用);前端仅增量扩展三个既有页面。构建链与发行打包不变(浏览器运行时探测,不入包)。

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

无违例——六项双轮 PASS;本节为空。
