# Implementation Plan: 轻交付首版——闲鱼虚拟商品自动发货

**Branch**: `main` | **Date**: 2026-09-22 | **Spec**: [spec.md](spec.md)

**Input**: `specs/001-xianyu-auto-delivery/spec.md`；宪章 2.0.0。

**Status**: 设计完成，可进入任务拆分；未创建业务实现、未运行平台验收。

环境说明：setup-plan 返回的逻辑标识 BRANCH 为 `001-xianyu-auto-delivery`，
实际 `git branch --show-current` 为 `main`。本次不切换或创建分支，功能定位以
`.specify/feature.json` 为准。不得据脚本逻辑标识声称已创建 Git 分支。

## Summary

用 Rust 服务承载闲鱼接入、订单核验、固定文字交付和异常恢复；TypeScript 提供本地管理页面。
采用 SQLite 保存账号、规则、订单、任务和发送证明，首版不用 Redis、微服务、Go 常驻进程或桌面外壳。
前端采用原生 TypeScript 页面模块＋Vite，普通用户运行嵌入页面的单个主程序；浏览器兜底按需启动。

不是逐行翻译上游：参考协议和失败处理，以本项目需求决定首版范围。
必须区分内容交付与平台确认，发送未知停止重放；平台直连优先，但不能假设所有账号均无浏览器依赖。
本版不实现卡密、小刀免拼、淘宝通道和营销功能，也不建立未使用的库存表或通用插件框架。

## Technical Context

**Language/Version**: Rust 2024 edition（实施启动时锁定验证通过的 stable 完整版本，最低 1.85，
最终以依赖 MSRV 为准）；TypeScript 7.0.2。开发/构建 Node 24 LTS，发行版不依赖 Node。

**Primary Dependencies**: Tokio 1.53.1、Axum 0.8.9、rusqlite 0.40.2（bundled/backup）、
Vite 8.3.0；reqwest、tokio-tungstenite、serde、rmpv、tracing、Argon2id、aes-gcm、windows
等按首个兼容构建锁版本。按需浏览器适配采用 chromiumoxide 0.9.1。
只核对了部分库发布存在，不宣称组合已经编译通过；取舍见 [research.md](research.md)。

**Storage**: 本地 SQLite WAL＋FULL，专用 DB 线程、窄操作队列、版本化迁移；
凭证与交付内容列加密，数据密钥由当前用户 Windows DPAPI 包装。
不迁移上游数据库，不支持网络共享目录。

**Testing**: Rust 单元与集成测试、临时真实 SQLite、可控 HTTP/WS 平台、进程级故障注入；
前端 Vitest＋DOM 测试、开发期 Playwright 页面验收。Rust fmt/clippy/test，TS typecheck/test/build。
Playwright 只用于开发验证，不作为卖家运行依赖；实际平台能力另有授权实单门禁。

**Target Platform**: Windows 11 x64，当前用户权限、浏览器管理页；本地 127.0.0.1:59189。
其他系统与无人交互 Windows 服务不在本版分发范围；不宣称锁屏时交互验证可用。

**Project Type**: 本地自托管管理工具，Rust 模块化单体＋嵌入式静态前端。

**Performance Goals**: SC-001—SC-009 全部继承：5 分钟配置规则，完整付款事实后正常交付
P95≤10 秒，漏通知在恢复后 5 分钟内有交付或可处理结果，查询 P95≤2 秒，
页面异常提示≤10 秒。平台等待与本地耗时分别记录。

**Constraints**: 单数据目录单执行实例；未知外部结果不重放；无依赖参考源码的构建；
默认不开启自动交付；不将模拟测试表述为实账号支持。资源占用必须测量，暂不虚设内存承诺。

**Scale/Scope**: 1 管理员、3 闲鱼账号、100 商品、10,000 历史订单验收基准；
普通单与完整 SKU、固定文字每单一次，多件数量不增加发送次数。

## Constitution Check

| 宪章检查 | 研究前评估 | 设计后证据与结论 |
| --- | --- | --- |
| 已付款、身份、去重与未知结果 | 必须覆盖正常及失败路径 | data-model 的唯一初始任务、attempt与状态机；平台契约四类结果，符合 |
| 平台与业务分离 | 不搬运全部 Go 架构 | 单 crate 内 domain/application/adapters 分层，平台不得写业务表，符合 |
| Rust＋TS、轻量与测量 | SQLite与无外壳为拟选 | research R1/R2 确定；按需浏览器计入进程树测量，符合 |
| 测试与可观测 | 需要确定性夹具及实单门禁 | quickstart V01—V12、SC映射与研究风险实验，符合（执行证据待实现） |
| 敏感数据与源码隔离 | 上游仅参考 | DPAPI/AEAD/日志脱敏/发行白名单/不依赖上游，符合 |
| 范围与升级兼容 | 不把淘宝/卡密混入首版 | 状态迁移、恢复隔离、显式非目标、备份版本验证，符合 |

以上是设计合规检查通过，不是运行验收通过。未发现需放宽宪章的设计偏离。
未知平台兼容性已转换为实施实验和发布门禁，不以隐藏失败或缩减需求解决。

## Project Structure

### Documentation (this feature)

```text
specs/001-xianyu-auto-delivery/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── http-api.md
│   ├── platform-adapter.md
│   └── cli.md
└── checklists/requirements.md
```

`tasks.md` 由后续 `$speckit-tasks` 生成，本次不创建。

### Source Code (repository root)

以下为后续实施目标，本次不建立源码骨架：

```text
qing-delivery/
├── Cargo.toml / Cargo.lock / rust-toolchain.toml
├── src/
│   ├── main.rs                 # 启动、配置、信号与退出
│   ├── lib.rs
│   ├── domain/                 # 身份、订单事实、规则、交付状态与纯决策
│   ├── application/            # 账号、同步、交付、人工处理、查询、备份用例及消费者接口
│   ├── runtime/                # 账号执行器、调度、取消、暂停屏障与运行生命期
│   ├── adapters/
│   │   ├── xianyu/             # auth/credentials/mtop/ws/codec/catalog/orders/delivery
│   │   ├── browser/            # 会话连续性、详情兜底、人工官方验证
│   │   ├── sqlite/             # DB线程、仓储、短事务、迁移、备份
│   │   └── windows/            # DPAPI、ACL、目录锁与受控子进程
│   ├── transport/              # HTTP DTO、认证、校验、CLI
│   └── webui/                  # 编译产物嵌入
├── migrations/                 # 新项目本地数据结构，不依赖上游迁移
├── frontend/
│   ├── package.json / package-lock.json / tsconfig.json / vite.config.ts
│   └── src/
│       ├── app/                # 路由、会话、导航
│       ├── features/           # accounts/catalog/rules/orders/issues/settings
│       └── shared/             # HTTP、错误、契约类型、通用DOM辅助与样式
├── tests/
│   ├── fixtures/               # 重新脱敏/合成的协议及订单样本
│   ├── contract/               # HTTP及平台契约
│   ├── integration/            # 数据库、生命周期及故障点
│   └── e2e/                    # 页面操作与独立发行版
├── scripts/                    # 验证与发行白名单，不执行远程安装脚本
└── Ydisks-Xianyu-Helper/        # 仅本地参考，Git和打包均排除
```

**Structure Decision**: 一个 Rust crate，内部模块表达边界，不预先拆多个 workspace crate。
应用层定义窄仓储/平台接口；domain 不依赖 HTTP/SQLite/浏览器；transport 不执行 SQL 或协议。
前端 feature 内使用 API 适配器，shared 不导入业务页面；列表使用分页，页面请求支持取消和代次校验。
开发期页面通过 Vite 代理同源服务；发行期服务嵌入页面，HTML不持有账号凭证。

## Runtime & Delivery Design

### 运行与资源所有权

主进程取得数据目录独占锁→解密密钥→迁移与校验→启动 DB 线程→HTTP 与账号 supervisor。
数据默认在 `%LOCALAPPDATA%\QingDelivery\data`，可显式选择另一本地目录；数据库失败只允许诊断，禁发货。
每个账号执行器独占 Cookie 代次、WS写入通道、控制代次和待提交动作；接收与外部完成事件异步回报。
不同账号用独立有界队列，队满记录背压与同步缺口，不因丢队列项声称业务已处理。

页面每3秒查询本地摘要，隐藏后停止轮询；不是平台订单轮询。
平台断线指数退避带抖动，初始2秒、最大60秒；成功后重置，验证/失效状态等待人工，不紧密重连。
后台任务由根取消信号派生；退出禁止新任务，最长等待15秒收尾，再将未定已提交尝试交给重启时隔离。
DB线程与浏览器进程均必须等待退出；强制终止只能归类未知，不能记确定失败。

### 正常交付与最终提交边界

1. 平台适配器解析所有通知条目，输出可信事件及身份来源；普通聊天不生成付款事件。
2. 应用先保存规范事件/订单事实；重复事件无新任务。缺字段从单订单详情补齐，必要时浏览器兜底。
3. 核验普通订单、可信 paid_at、身份、规格、金额、数量与当前规则；历史或事实不足进入待处理。
4. 短事务创建唯一初始交付任务、加密内容快照和规则版本；已有任务则转到既有状态处理。
5. 最终准备前刷新资格；新鲜度窗口最多5秒，已知的取消/退款等新事实随时使资格失效。
   检查当前控制、凭证和规则版本；无法证明事实时不猜测。平台外部状态在瞬间变化仍存在竞态，
   不能承诺本地程序可与平台订单状态做原子事务。
6. 短事务写 attempt、请求mid、内容摘要与 dispatching；事务结果不明确不得发送，先查询操作ID。
7. 账号执行器串行化 control 与 transport handoff，不在DB事务中等待网络。
   确定未提交可安全失败；handoff 后超时/取消均未知，只有严格关联确认才记内容成功。
8. 内容成功后独立准备平台确认，核对当前开关和资格，记录独立 attempt。
   平台确认失败或未知只核对该动作，不进入正文发送路径。

“暂停生效”以执行器屏障完成为准，不以HTTP收到请求为准。UI显示 pausing，直到新 handoff 被阻断。
同一订单自动与人工共享执行权；已经提交的未知尝试不因锁超时或重启变成可再次发送。

### 补偿与人工操作

订单发现每60秒限量拉取已售列表，分页上限100页，每页按平台约束；未证明完整时显示缺口。
普通待发货首次被可靠观察后至少等待120秒，再补偿缺事件任务，以给实时消息先处理的机会。
扫描源时钟使用 UTC，间隔使用单调时间；停机较久的范围按实际能追溯的数据报告。
有任务的订单进入任务恢复，不创建另一初始任务；未知或人工隔离禁止自动恢复。

确定未发送的暂时故障在初次外最多3次重试，30/60/120秒；永久拒绝直接待处理。
人工补发带一次性幂等键、预期版本、原因、风险确认，使用同一原文。
允许 shipped但未completed的显式补发；取消、退款、完成、账号错误不得发。
人工“已收到”只记录人工证据，不自动调用平台确认。旧备份隔离必须逐单核验，重新开账号不能清除。

## Delivery Sequence & Requirement Coverage

此表是规划顺序，不是已生成的任务清单；每个阶段都要保留可运行验证。

| 顺序 | 内容与退出条件 | 需求 |
| --- | --- | --- |
| A | 锁定工具链，Rust/TS最小构建、SQLite/DPAPI/协议假服务实验；验证关键依赖可用 | FR-027—030；research R8 |
| B | 初始化认证、数据迁移、目录锁、暂停/恢复和账号状态；无账号也可测 | FR-001—005、026—030 |
| C | 扫码/Cookie/WS/商品/订单事实、完整SKU及按需浏览器，批量事件夹具通过 | FR-002、004、006、010—012、017 |
| D | 规则版本、持久交付、关联确认、平台确认独立、暂停竞争和崩溃矩阵 | FR-007—016、019—023 |
| E | 漏单发现、人工接管/补发、订单查询、待处理、备份恢复完整页面 | FR-017—025、028 |
| F | 独立发行包、3账号规模测试、授权实单矩阵、许可与数据脱敏检查 | FR-026—031、SC-001—009 |

FR-031 通过入口拒绝和 UI 能力标识验证，不创建淘宝或卡密空实现。
所有 P1/P2 都交付后才可称本规格完成，不以只跑通 happy path 宣布首版完成。

## Validation & Release Gates

具体命令与预期见 [quickstart.md](quickstart.md)。计划阶段未运行这些未来命令。
HTTP/平台契约采用 [contracts](contracts/) 的文档定义，实施时将字段落为命名DTO，
并通过真实 handler 和假平台验证；不能用源码字符串匹配替代行为测试。

必须产出：本地协议/浏览器夹具证据、崩溃恢复矩阵、3账号性能样本、备份升级结果、
无开发工具环境运行结果、实单支持矩阵及原作者声明审查。
发行目录使用显式白名单，禁止递归把仓库或上游打包。

## Complexity Tracking

无未解释的宪章偏离。

| 新增复杂度 | 必要性 | 不采用的更简单替代 |
| --- | --- | --- |
| 持久 attempt + unknown + 关联证明 | 外部发送不能与本地提交原子完成 | 超时就重发会重复交付 |
| 专用DB线程与短事务 | 明确SQLite所有权且不阻塞异步网络 | 各handler直连DB难以定义交易边界 |
| 按需浏览器边界 | 兼容需要页面详情及官方验证的账号 | 宣称永久无浏览器会隐藏未满足需求 |
| 恢复隔离与加密归档 | 数据一致快照不能证明快照后未发货 | 裸复制DB再自动启动会误发 |

## Design Completion

Phase 0 研究与 Phase 1 设计已生成；架构选择无待用户澄清阻塞项。
平台实证与依赖构建仍是实现阶段的明确门禁。本次只更新规划文档，
保留既有宪章与规格的工作区改动，不生成任务、不提交、不安装工具链或启动上游。
