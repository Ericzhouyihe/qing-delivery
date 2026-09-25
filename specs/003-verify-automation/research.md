# Research: 安全验证自动化(003-verify-automation)

**Date**: 2026-09-23 | **Status**: Complete(全部未知项已解决)

## 代码库现状核查(事实依据)

| 事实 | 位置 | 对本计划的影响 |
|---|---|---|
| chromiumoxide 0.9.1 已在依赖树(native-tls+zip8) | Cargo.toml:30 | 引擎零新增 crate;宪章 III"浏览器自动化按需使用"可直接落地 |
| mtop 错误分类已有风控类:`MtopError::Verification(String)`,classify_ret 识别 FAIL_SYS_USER_VALIDATE/rgv587/punish/x5secdata | adapters/xianyu/mtop/client.rs:24-79 | 触发信号源已存在;payload 目前是原始 ret 串,**缺验证 URL 提取**(参考项目从响应 JSON gotoUrl 类字段取) |
| WS MessageKind 仅 PaidOrder/OrderCompleted/Other | adapters/xianyu/events.rs:20 | 需新增验证信号事件类(WS 文本含 punish/滑块关键词时) |
| QR 会话已有 verification_required 状态 | application/accounts/qr.rs | 第三信号源;人工验证完成检测可复用 QR 轮询通道语义 |
| 对象安全驱动模式已两次落地(QrDriver、ItemSyncDriver 注入 AppState) | application/accounts/authorize.rs、ports/platform.rs | VerificationDriver 沿用同型注入,transport 不依赖具体适配器类型 |
| 凭证加密存取已就绪:credentials::save/load(DPAPI 包裹数据密钥) | adapters/sqlite/repos/credentials.rs | 凭证回写复用现有加密路径,不新增明文面(FR-014) |
| supervisor 已有 credential_epoch 语义与账号会话恢复(启动时从持久化凭证载入) | runtime/supervisor.rs:318-367 | 验证成功后:bump credential_epoch + 触发该账号会话热更新即可恢复在线 |
| 现有验证端点为占位(返回 instruction+browser_available:false) | transport/accounts_api.rs get_qr_image 附近 verification handler | 升级点:POST 升级为真实触发;新增 GET 会话状态/审计 |
| issues 表结构固定(reason_code/allowed_actions) | migrations/0001_init.sql:263 | 转人工事项需带 verification_url → 迁移 0002 增 metadata 列(json) |
| 002 待处理工作台按 category 分组、allowed_actions 驱动按钮 | features/issues/page.ts | 新类别"安全验证"零结构性改动,前端只需映射与入口 |
| 参考项目 internal/browser(生命周期池 1170 行+滑块三引擎 ~1500 行) | Ydisks-Xianyu-Helper/(仅参考,不入构建) | 流程/策略/降级链思想可参考;许可核对仅当移植实质代码(假设:不移植,重写) |

## 决策记录

### D1: 浏览器引擎——chromiumoxide + 系统已装 Chrome/Edge,零新增依赖

- **Decision**: CDP 驱动用已在依赖树的 chromiumoxide;启动时发现本机浏览器(按序探测 Chrome/Edge 常见安装路径与注册表);不打包浏览器进发行包。
- **Rationale**: 宪章 III 轻量(零新增 crate、发行体积不变);原项目经验表明滑块需要完整 Chromium(非 WebView);Windows 用户普遍已装 Edge。
- **Alternatives**: playwright-driver(引入 Node 运行时,违背无开发环境分发);headless_chrome crate(maintenance 弱于 chromiumoxide);打包 Chromium(+150MB 体积)。
- **默认有头**:有头窗口滑块通过率显著更高且卖家可旁观;`--headless` 为配置项(assumptions 已记)。

### D2: 触发信号——三源统一为内部验证事件,URL 必须随信号传递

- **Decision**: ① mtop:`classify_ret` 的 Verification 分支增强——抛错前从响应 JSON 提取验证 URL(gotoUrl/url 类字段,按实账号响应校准);② WS:入站文本含 punish/滑块/安全验证 关键词 → 新 `MessageKind::VerificationRequired { url: Option<String> }`;③ QR 流 verification_required(已有)。三源收敛到 application 层统一信号 `VerificationSignal { account_id, source, url, raw }`。
- **Rationale**: FR-001 只认明确信号;URL 是打开验证页与转人工链接的前提(参考项目 TokenCaptchaManualVerificationURL 同理)。
- **Alternatives**: 仅 mtop 源(WS 侧风控会漏);文本模糊匹配全部入站消息(误报,违反 SC-304)——关键词集合收窄到 punish 系。

### D3: 应用层服务——VerificationService + 对象安全 VerificationDriver

- **Decision**: `application/verification/service.rs` 实现状态机/重试/超时/降级/审计/账号闸门;浏览器执行经 `VerificationDriver` 对象安全 trait(`open_and_solve(url, cookie_seed) -> SolveOutcome`)注入,adapters/browser 实现;AppState 增挂点(沿 QrDriver/ItemSyncDriver 同型)。
- **Rationale**: 平台与业务分离(宪章 II):服务层不知晓 CDP;适配层不知晓重试策略;mock 构建注入 None 端点如实报不支持。
- **Alternatives**: 服务直接调 chromiumoxide(分层违规);放进 xianyu adapter(验证不专属闲鱼语义,未来平台复用)。

### D4: 滑块执行——CDP 真实输入事件 + 人类轨迹,确定性夹具页测试

- **Decision**: 滑块定位经 Runtime.evaluate(选择器按实账号校准,初始覆盖 baxia/nc_ 系节点);拖动用 Input.dispatchMouseEvent 合成**人类轨迹**(贝塞尔路径+速度曲线+微抖动);成功判定=punish 退出信号或页面回调;`tests/fixtures/slider-page.html` 自建滑块夹具页供集成测试(无真实账号可测全链)。
- **Rationale**: 平台滑块带行为检测(参考项目结论:纯 JS 派发事件易被识别,真实鼠标请求是其默认);确定性测试是宪章 IV 要求;真实通过率只能实账号验证(SC-301 如实标注)。
- **Alternatives**: 远程打码(FR-009 禁用);纯 JS 模拟(识别率高)。

### D5: 验证期间交付闸门——内存 per-account gate,不改控制开关语义

- **Decision**: 验证会话存在期间,supervisor 的 handoff/交付触发前检查该账号 gate(内存集合),命中则延迟交付入队等待;验证结束(成功/降级)释放。**不使用** account_control 暂停(那会改持久化期望状态,语义过重)。
- **Rationale**: FR-003 只需"暂不用可能失效的凭证发货";闸门是瞬态约束,持久化开关会污染重启恢复语义。
- **Alternatives**: 复用 pausing 屏障(改变 run_enabled 期望,不可);验证期间直接丢弃消息(丢事件,不可)。

### D6: 凭证回写——复用加密存储 + credential_epoch 热更新

- **Decision**: 滑块成功后从浏览器读取完整 Cookie jar → `credentials::save`(现加密路径)→ accounts.credential_epoch+1 → 通知 supervisor 重载该账号会话(重连 WS/mtop,参照启动恢复路径)。保存失败:旧凭证不动,审计记"凭证更新失败",转人工。
- **Rationale**: FR-014;supervisor 启动恢复逻辑已验证过"从持久化凭证重建会话"这条路。

### D7: 浏览器生命周期——单例 BrowserManager(队列+回收+退出清理)

- **Decision**: runtime 内单例管理器:mpsc 请求队列(并发上限 1)、同账号会话复用(user-data-dir=data_dir/browser-profile,启动前锁检测)、空闲 5 分钟自动关闭、shutdown 挂 runtime_stop 收尾链、崩溃重建计入重试。策略参数集中在 BrowserInstancePolicy。
- **Rationale**: FR-007/SC-303;参考项目 lifecycle_pool 的职责划分(池/实例/会话)可借鉴。

### D8: 持久化与 API——迁移 0002(审计表+issues.metadata),验证端点升级

- **Decision**: 迁移 0002:`verification_attempts` 审计表(只增)+ `issues` 增 `metadata TEXT`(json,存 verification_url 等)。API:POST /accounts/{id}/verification 升级为真实触发(202+会话句柄);GET /accounts/{id}/verification 返回当前会话状态+最近尝试;capabilities 增 `browser: { available, reason }`;人工完成检测:failed_manual 事项存在时 60s 轮询凭证有效性(轻量只读探测),恢复即自动关闭。
- **Rationale**: 全部 additive;002 前端零破坏(SC-306);审计只增不改满足 FR-010。

## 未知项清单(Phase 0 结论)

| Technical Context 未知项 | 结论 |
|---|---|
| 浏览器引擎/打包策略 | chromiumoxide+系统浏览器,不打包(D1) |
| 触发信号源与 URL 获取 | 三源统一事件,mtop 响应提 URL(D2) |
| 分层归属 | application/verification + VerificationDriver(D3) |
| 滑块自动化与可测性 | CDP 人类轨迹+自建夹具页(D4) |
| 验证期间交付处理 | 内存 gate,不动控制开关(D5) |
| 凭证热更新路径 | 加密存储+epoch+会话重载(D6) |
| 生命周期治理 | 单例管理器队列+回收(D7) |
| 存储/API 契约 | 迁移 0002+additive 端点(D8) |

无遗留 NEEDS CLARIFICATION。滑块选择器与成功判定的**平台 specifics**标注为实账号校准项(任务中显式跟踪),不在计划期臆造。
