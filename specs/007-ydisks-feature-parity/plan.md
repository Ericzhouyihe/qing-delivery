# Implementation Plan: Ydisks 功能缺口复刻·多域增量(007-ydisks-feature-parity)

**Branch**: `007-ydisks-feature-parity` | **Date**: 2026-09-25 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/007-ydisks-feature-parity/spec.md`

## Summary

按规格补齐与 Ydisks 的功能缺口,以七个可独立交付的增量落地:卡密库存、发货模板、自动化规则扩展(变体/优先级/关键词/默认回复/求评)、在线聊天、通知渠道、订单同步与人工交付动作、系统与 AI 设置。技术路线完全沿用现有模块化单体:Rust 后端按 domain / application / adapters / transport 四层扩展每个限界上下文,SQLite 迁移只追加(0004~0006),前端保持零运行时依赖的 vanilla TS,新页面复用既有 UI 积木与三态/确认流模式。内容来源统一收敛到交付管线的 `freeze_snapshot` 转换点;规则唯一性约束按规格澄清修订为"多条并存+优先级选取"。

## Technical Context

**Language/Version**: Rust(edition 2024,工具链由 rust-toolchain.toml 固定)+ TypeScript(strict, verbatimModuleSyntax,零运行时依赖)

**Primary Dependencies**: 既有:tokio 1.53 / axum 0.8(ws 已启用) / rusqlite 0.40(bundled+backup) / reqwest 0.12(native-tls+json) / tokio-tungstenite / aes-gcm / argon2 / chrono / tracing。本特性新增(见 research.md D11):`lettre 0.11`(仅 smtp-transport+builder,邮件渠道)、`calamine`(xlsx 批量导入)、`csv 1.x`(CSV/TSV 导入)、axum `multipart` feature(图片上传/导入文件)。

**Storage**: SQLite(单文件,`DbThread` 单线程串行访问;迁移经编译期常量表追加,只增不改)

**Testing**: `cargo test`(单元+集成,`dev-fixtures` feature 提供 mock 档)+ frontend `vitest`(jsdom,mock fetch 按 URL 分发)+ `seed-ui.exe` 演示播种 + 浏览器走查

**Target Platform**: Windows 本地运行(`qing-delivery.exe serve`,默认 127.0.0.1:59189),浏览器管理页

**Project Type**: 模块化单体(Rust 后端嵌入前端构建产物)+ vanilla TS SPA

**Performance Goals**: 概览含第 5 张卡后单请求装配仍为一次 DB 快照;聊天轮询可见时 ≤3s 且页面不可见暂停;卡密预留/扣减为同事务 O(1);通知单渠道投递超时 10s 不阻塞事件管道(异步派发)

**Constraints**: 网络调用绝不进 DB 事务;结果未知不回库/不自动重发;写接口全部 CSRF+幂等键;迁移只追加且迁移前自动备份;新页面零新增 CSS 为目标、颜色只引 token

**Scale/Scope**: 单管理员、账号数十以内、卡密数千、消息数万级;7 个新前端页面/改版 + 约 30 个新 HTTP 端点 + 3 个迁移文件

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 检查项 | 结论 |
|---|---|---|
| I. 以已付款订单和可恢复交付为核心 | 卡密"原子预留→绑定订单→快照→发送→确定成功扣减",结果未知不回库不换卡(research D3);模板/卡密内容在 `freeze_snapshot` 先冻结再发送(D2/D4);关键词/默认/AI 回复独立于交付管线,不产生订单状态变化、不得视为付款证明(D7);聊天发送复用 SendOutcome 四分类,unknown 不自动重发(D6) | ✅ 通过 |
| II. 平台接入与发货业务分离 | 聊天收发、图片、历史回填均经 `PlatformAdapter` 端口扩展(能力声明门禁,D6);通知渠道与 AI 是外部服务,各自经端口(adapters/notify、adapters/aiclient),业务判定在 application;transport 只做 handler 装配 | ✅ 通过 |
| III. 轻量部署,以测量决定优化 | 不引入任何外部服务/进程;新增 3 个纯 Rust 库 + 1 个 feature 开关,均记录理由与替代方案(D11);聊天实时性首版轮询(WS 后置);聊天 3s 轮询的空闲开销纳入本特性走查测量项 | ✅ 通过 |
| 不变量修订 | 001 FR-008"同范围仅一条启用规则"按规格 Clarifications 修订为优先级模型:重建唯一索引为 `(account_id,item_id,sku_key,trigger_type,priority) WHERE enabled=1`,应用层 `EnabledConflict` 语义改为"同优先级冲突"(D1);防重复交付改由"仅执行最高一条+既有执行互斥"保障 | ✅ 已声明,plan/tasks 需回写 001 说明 |

**Phase 1 复查**:设计完成后所有新增外部动作(通知/AI/API 取卡/聊天发送)均有尝试持久化与结果分类路径;无新增进程/服务;复查通过(见 research.md 各 Decision 的宪章对齐行)。

## Project Structure

### Documentation (this feature)

```text
specs/007-ydisks-feature-parity/
├── plan.md              # 本文件
├── research.md          # Phase 0:15 项设计决策(D1~D15)
├── data-model.md        # Phase 1:实体/表/状态机/迁移划分
├── quickstart.md        # Phase 1:七增量端到端验证指南
├── contracts/
│   └── http-api.md      # Phase 1:全部新增 HTTP 端点契约
└── tasks.md             # Phase 2 产出($speckit-tasks,本命令不创建)
```

### Source Code (repository root)

```text
migrations/
├── 0004_cards_templates_rules.sql   # 增量1:卡密池/条目、模板、规则扩展列+变体+关键词+默认回复+求评状态
├── 0005_chat.sql                    # 增量2:会话、消息、快捷回复、买家备注
└── 0006_notify_ai.sql               # 增量3:通知渠道/绑定/投递、系统设置/机密、账号AI列、无订单issue去重索引

src/
├── domain/
│   ├── cards.rs          # 卡密组/条目状态机、预留规则、API取卡配置校验
│   ├── templates.rs      # 模板消息结构、占位符语法与校验
│   ├── rules_ext.rs      # 触发类型/优先级/变体匹配、关键词与默认回复模型
│   ├── chat.rs           # 会话/消息模型、发送状态机、未读聚合
│   ├── notify.rs         # 渠道类型/配置模型、事件订阅匹配
│   └── ai.rs             # AI 配置模型、分流顺序常量
├── application/
│   ├── cards/            # 卡密组 CRUD、追加、批量导入、API取卡、预留/扣减/释放用例
│   ├── templates/        # 模板 CRUD、引用检查、渲染
│   ├── catalog/rules.rs  # 扩展:变体/优先级/触发类型(改造既有 RuleService)
│   ├── replies/          # 关键词回复、默认回复、AI 回复分流编排
│   ├── chat/             # 消息摄取、发送、未读、快捷回复、买家备注
│   ├── notify/           # 事件订阅、渠道派发、投递记录
│   ├── orders_sync.rs    # 一键同步任务(复用 TraceScanService)
│   ├── manual/actions.rs # 扩展:trigger_delivery / confirm_shipment
│   ├── settings_sys.rs   # 系统设置/机密、管理员凭据修改
│   └── ports/            # platform.rs 扩展 + ai.rs + notify sender + card supplier
├── adapters/
│   ├── xianyu/           # 事件抽取 ChatMessage 变体、send_chat_message、(能力门禁)图片/历史
│   ├── notify/           # webhook/bark/dingtalk/feishu/wecom/telegram(reqwest)+ email(lettre)
│   ├── aiclient/         # OpenAI 兼容 chat completions + 模型列表
│   └── sqlite/repos/     # cards/templates/rules_ext/replies/chat/notify/settings/stats 扩展
├── transport/
│   ├── cards_api.rs / templates_api.rs / rules_api.rs / chat_api.rs /
│   ├── notify_api.rs / settings_api.rs / orders_api.rs(扩展) / routes.rs(注册)
│   └── error.rs          # 新错误码(见 research D14)
└── runtime/supervisor.rs # dispatch_loop 扩展:ChatMessageReceived 分支、notify 挂钩、求评定时器

frontend/src/
├── app/router.ts         # 11 项导航;/catalog 拆为 /items 与 /rules
├── features/
│   ├── cards/            # 卡密库存页(列表/编辑抽屉/批量导入/API构建器/测试)
│   ├── templates/        # 发货模板页(编辑器+变量指南)
│   ├── rules/            # 自动化规则页(交易自动化/关键词/默认回复三页签;复用 match-preview)
│   ├── items/            # 商品列表页(自 catalog 拆出,保留关联规则跳转)
│   ├── chat/             # 在线聊天(三栏,3s 轮询,快捷回复抽屉,买家备注)
│   ├── notifications/    # 通知设置(渠道 CRUD/测试/绑定/系统SMTP)
│   ├── overview|orders|settings|accounts  # 扩展:库存卡/同步工具栏与新人工动作/AI与改密表单
├── shared/contracts.ts   # 新 DTO + parse 守卫(缺失字段降级模式)
└── ui/                   # 仅扩展:icons 新增路径、badge 新轴;零新依赖
```

**Structure Decision**: 沿用现有单仓库两层结构(Rust 后端 + frontend TS),不新增顶层工程;每个限界上下文在 domain/application/adapters/transport 四层各有落点,跨域只经 application 用例(宪章 II)。迁移文件按增量划分(0004/0005/0006),每增量可独立实现、验收、交付。

## Complexity Tracking

> 无宪章豁免项。001 FR-008 不变量修订已作为显式治理决策记录于 Clarifications 与 research.md D1,由 tasks 回写 001 规格注记。
