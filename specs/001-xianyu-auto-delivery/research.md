# Research: 轻交付首版实现决策

日期：2026-09-22。依据：[需求](spec.md)、宪章 2.0.0，以及上游
`42a476fa8f325791509fbadae819984ae12fccfa`。本轮完成文档及源码研究，
未编译 Rust、未连接卖家账号、未实测资源占用。当前工作环境可用 Node/npm，未发现 Rust/Cargo。

## R1. Rust 模块化单体与原生 TypeScript 界面

**Decision**：Rust 2024 edition、Tokio 1.53.1、Axum 0.8.9；前端 TypeScript 7.0.2、
Vite 8.3.0，使用原生 DOM 与按功能拆分的页面模块，不预先引入 React 或桌面外壳。
开发环境使用 Node 24 LTS；Rust 采用实施启动时 stable 工具链并在 `rust-toolchain.toml`
锁定实测通过的完整版本，最低不低于 1.85，若依赖 MSRV 更高则以依赖为准。
其余库在首个构建实验中选择兼容发布并提交 Cargo.lock/package-lock.json。

**Rationale**：首版只有账号、规则、订单、异常等表单和列表，原生 TS 足以构建功能适配层与
取消请求管理；用户仅固定 Rust+TS，没有要求 React。主程序嵌入编译后的页面，普通用户无需 Node。
运行状态由服务维护，关页面不会停发货。

**Alternatives considered**：React/Vue 可降低复杂交互维护成本，但首版不为复用上游界面
引入整个框架及其依赖；若未来页面复杂度有实际证据，再记录架构决策。Electron/Tauri 暂无必要。
不选择 Node 后端或保留 Go 辅助服务，以免两套业务实现和运行环境长期并存。

版本证据：本轮读取 crates.io 和 npm 元数据确认以上发布存在；这不是编译兼容性验证。
Rust 官方 stable manifest 请求超时，因此不虚构当前 stable 版本。
[Axum 文档](https://docs.rs/axum/latest/axum/)、[Vite 文档](https://vite.dev/guide/)。

## R2. SQLite 与明确的连接所有权

**Decision**：rusqlite 0.40.2，启用 bundled 和 backup；本地磁盘 SQLite、WAL、
synchronous=FULL、foreign_keys=ON、busy_timeout=2000ms。一个专用数据库线程独占业务连接，
Tokio 用有界命令队列和一次性响应提交窄仓储操作。短事务只包含本地校验和状态变化。
备份用独立源/目标连接分步执行，不在主数据库线程 sleep。数据库文件不得位于网络共享目录。

**Rationale**：目标只有 3 账号、100 商品、10,000 历史订单，单写者足够且部署最简单。
数据库拥塞或持久化失败禁止新的外部动作；平台请求和人工等待绝不放在事务内。
应用事务请求已入队但调用方超时，不能假设事务已取消：必须按操作 ID 查询结果后再决定发送。

**Alternatives considered**：SQLx 适合异步数据库多连接，但当前多方言/高吞吐不是需求；
MySQL/Postgres 增加安装成本；JSON 文件难以可靠表达去重、恢复与多表原子更新。

[SQLite WAL](https://www.sqlite.org/wal.html)、
[synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous)、
[rusqlite Connection](https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html)。
SQLite 默认不加密，WAL/FULL 只涉及并发和耐久性，不提供保密性。

## R3. 闲鱼协议直连，浏览器按事实需要启用

**Decision**：正常路径用 HTTP/WebSocket；Rust 库候选为 reqwest、tokio-tungstenite、
serde、rmpv/rmp-serde、base64 与 MD5 协议摘要实现。浏览器边界独立，需兜底时使用
chromiumoxide 0.9.1 控制当前用户安装的兼容 Edge/Chrome，专用受限 profile，不读取用户日常 profile。
缺少兼容浏览器且平台要求它时显示不可用，不能假装验证或规格获取成功。
浏览器桥接与人工官方验证属于首版兼容能力；可按需启动，不能删除它来换取更小体积。
不移植自动滑块、密码登录、验证码绕过或上游整个 Playwright 运行时。

**Rationale**：上游扫码、续期、商品/已售列表、详情和发送均有协议实现。
`internal/browser/orders.go` 也是详情协议优先，仅缺少金额或规格时回退页面。
官方验证可能依赖原临时会话，不能以随意打开一个 URL 代替会话延续。
若目标账号必须浏览器才能满足完整 SKU 验收，浏览器兜底就是发布必需项，不能降为未来功能。

**Alternatives considered**：全部浏览器操作资源更高且对页面敏感；永久无浏览器会把未验证假设
变成支持承诺；捆绑 Go 或 Node Playwright 会增加本项目长期运行依赖。

[CDP 协议](https://chromedevtools.github.io/devtools-protocol/)、
[chromiumoxide](https://docs.rs/chromiumoxide/0.9.1/chromiumoxide/)。
CDP 架构可行性不等于平台验证兼容性；重定向、Cookie 域路径及浏览器版本在实现实验中核验。

## R4. 参考行为与独立验收

| 能力 | 上游参考路径（均位于被忽略参考目录内） | 独立实现要求 |
| --- | --- | --- |
| 扫码授权 | internal/xianyu/qrlogin/client.go、confirmed.go、face_verification.go | 授权完成后核对账号，再建立运行连接；取消与迟到完成不能替换新会话 |
| 签名与 Cookie | internal/xianyu/protocol/sign.go、cookies.go；mtop/cookie_session.go | 签名 data 必须与实际发送字节一致；Cookie 保留 domain/path/expires 等 |
| 凭证续期 | internal/xianyu/renew/service.go、mtop/token.go | 签名 Token 失效、登录失效、验证要求分开；代次控制阻止旧结果覆盖 |
| 批量推送 | internal/xianyu/ws/client.go、sync.go；protocol/decrypt.go | 处理全部条目；坏条目不吞后续付款；帧 ACK 与业务事实保存分开 |
| 发送证明 | internal/xianyu/ws/sync.go、send_error.go；engine/outgoing_echo_confirmation.go | HTTP/WS code=200 或写 socket 成功不足以证明接收；需要匹配的 mid、买家、会话、正文及平台消息标识 |
| 商品同步 | internal/xianyu/mtop/items.go | 成功缺 cardList 是空列表；完整分页后才标记下架 |
| 订单同步 | internal/xianyu/mtop/sold_orders.go、order_detail.go | 完整性与追溯区间明确；不能把缺数量默认为已核验的 1 |
| 平台确认 | internal/xianyu/mtop/consign.go | 使用原交付证明；独立结果，不因确认失败再发正文 |
| 恢复 | internal/automation/card_delivery.go、pending_ship_scheduler.go | 保留未知结果、当前资格复核和原内容补发思想，不逐行迁移 |

上游 LICENSE 为 Apache-2.0，NOTICE 有原作者署名；README 有学习研究说明。
复用实质代码前核对适用许可及文件来源，保留必要声明；不把参考目录作为构建依赖。
MD5 仅用于平台协议签名，不能用于管理密码或数据保密。

## R5. 外部动作与暂停边界

**Decision**：数据库记录持久任务和尝试，账号执行器独占连接及发送入口。
发送前保存正文快照、请求关联 ID 和 dispatching 尝试，再向传输层交付。
崩溃后 dispatching 无可信结果一律 unknown，不自动重发；传输明确证明未提交才可安全重试。
重试为首次之外最多 3 次，间隔 30/60/120 秒，预算随任务持久保存。

暂停先持久化意图及控制代次，再由账号执行器串行化暂停屏障与 transport handoff。
未交给传输的动作取消，已交出的动作继续确认或归类 unknown。屏障完成前仅返回“暂停处理中”，
不能谎报已生效；不靠跨网络长时间持锁实现互斥。
规则禁用、凭证换代与恢复隔离同样在最终 handoff 前核对版本。

**Alternatives considered**：发送前只检查开关存在竞争窗口；自动重试所有超时会重复交付；
先发再写记录无法恢复。允许“未知则人工”是可审计的保守取舍，不声称网络层恰好一次。

## R6. 凭证、会话与备份

**Decision**：随机数据密钥经当前 Windows 用户 DPAPI 包装，不启用 LOCAL_MACHINE；
凭证、规则正文及交付快照使用成熟 AEAD（aes-gcm），随机 nonce，AAD 包含用途/实体/版本。
管理员密码 Argon2id；随机服务端会话，HttpOnly、SameSite=Strict、本地同源 Cookie，
服务端校验 Host/Origin 与 CSRF。完整正文只在授权详情中显示，不写 localStorage。

首版备份只支持停机独占CLI，采用 SQLite online backup 一致快照＋带版本清单的加密归档；
这里的 online 是SQLite技术名称，不表示首版支持服务运行中备份。
恢复命令须取得数据目录锁，在发布恢复目录前写全部账号暂停、恢复隔离、清除管理会话。
隔离覆盖快照中所有未完任务及快照后可能交付的区间；pending_ship 不能证明没有发过文字。
首版恢复仅支持同机器同 Windows 用户，DPAPI 失败不得生成新密钥覆盖旧密钥。
备份期间禁止密钥轮换；恢复前保留原数据安全副本，不通过普通启动自动合并或导入。

**Alternatives considered**：裸复制运行中 .db 会遗漏 WAL；仅记录 restore 标志到日志不能防止
下一次启动恢复发送；管理员密码直接充当密钥耦合改密与数据解密。

[SQLite Backup](https://www.sqlite.org/backup.html)、
[rusqlite backup](https://docs.rs/rusqlite/latest/rusqlite/backup/index.html)、
[Windows CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)。

## R7. 规格默认值的明确解释

- 监控起点比较可信 paid_at；下单在前但付款在后属于新付款。缺可信付款时间不能猜测，转人工。
- 自动交付关闭阻止新内容发送，包括人工补发；人工操作先明确重新开启。账号暂停阻止所有新平台动作。
- 人工标记买家已收到不自动确认平台发货，必须另选独立操作并复核。
- 内容版本更新不改旧快照；自动与人工执行前仍需规则当前启用且商品有效。
- 本版固定内容采用产品上限 1000 Unicode 标量且 UTF-8 不超过 4000 字节，不拆分。
  这是产品限制，不是已证实的平台限制。兼容性实验如证实平台上限更低，按较低值拒绝；
  拒绝必须显示当前限制，不以截断降级。发布必须有中英文与边界样本证据。
- 确定未发送自动重试耗尽之后，只能显式人工操作建立新的审计动作；普通重启不新建周期。
- UI 每 3 秒刷新本地摘要、隐藏页面停止轮询；交易执行不依赖 UI 轮询。
- 首版绑定 127.0.0.1:59189，避免与本地上游 59188 竞争；端口可配置，仅允许回环。

## R8. 已决策但尚待实现证据的风险

| 风险 | 最小实验与通过条件 | 失败时处理 |
| --- | --- | --- |
| Rust/库组合 | 原生 Windows 构建＋TLS/SQLite/DPAPI/静态资源烟测 | 选兼容补丁并锁定，不能宣布构建通过 |
| 扫码与官方验证 | 本地重定向夹具＋授权实账号；身份及 Cookie 会话连续 | 补浏览器桥接或重新扫码，验证期间账号暂停 |
| 完整规格/金额/类型 | 订单夹具矩阵＋普通及多 SKU 实单 | 页面兜底；仍缺事实就待处理，该兼容场景未验收 |
| 可靠消息证明 | 本地 WS 多种 ACK/回显＋实单关联 | 无可靠证明一律 unknown，不开放无条件重试 |
| 追溯范围与文本限制 | 已售分页边界＋有限长度样本 | 明示缺口/更低限制，不声称无条件支持 |
| 资源目标 | 3 账号/100 商品/10k 订单计时与进程树测量 | 分清浏览器与服务成本，再设置数字预算 |

研究中的选型歧义已作决定，不留阻塞设计的待澄清标记。
这些实验是后续实现/发布门禁，本次未执行；不能把它们写成“兼容性已验证”。
