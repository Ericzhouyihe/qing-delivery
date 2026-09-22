# CLI Contract: 轻交付首版

日期：2026-09-22。设计契约，尚未实现。程序发行名 `qing-delivery.exe`，Rust 二进制内嵌
TypeScript 页面产物。普通卖家无需安装 Rust、Node、Go 或参考项目。
默认回环地址 `127.0.0.1:59189`；Windows 当前用户运行，不默认安装系统服务、托盘或开机启动。

## 通用行为

```text
qing-delivery.exe <serve|init-admin|backup|restore> [options]
qing-delivery.exe --version
qing-delivery.exe <command> --help
```

- `--data-dir <path>` 可指定本地数据目录；相对路径按当前目录解析并规范化为绝对路径，
  默认 `%LOCALAPPDATA%\QingDelivery\data`。
  拒绝 UNC/网络映射存储；实际实现检查卷类型。ACL 限当前 Windows 用户及系统必需主体。
- live 数据目录持有进程级 OS 独占锁到退出，不用可遗留的 PID 文件代替；第二实例报明确错误。
  数据目录以实例 ID、profile 和 schema 版本绑定；路径归一化后检查，别名路径不能绕过锁。
- 密码和密钥不得作为命令行参数或环境变量传入；需要凭据时隐藏交互输入，不回显/记录。
- 退出码：`0` 成功；`2` 参数/操作前提错误；`3` 目录已被占用；`4` 认证/DPAPI 解密失败；
  `5` 数据库、备份、磁盘或完整性错误；`6` 版本不兼容；`7` 安全退出超时且有待核对动作。
  错误文本不包含原始凭证、完整交付正文或未经清理的平台响应。
- 本文示例均为实施后的预期命令，当前文档阶段不声称可执行。

## serve

```powershell
.\qing-delivery.exe serve
.\qing-delivery.exe serve --data-dir 'D:\QingDeliveryData' --bind '127.0.0.1:59189'
```

选项：`--data-dir`、`--bind <loopback:port>`、`--profile live|mock`（默认 live）。
拒绝 `0.0.0.0`、非回环地址和默认暴露局域网；端口冲突明确报错，不静默切换。

启动顺序：目录/profile 检查 → 独占锁 → 读取数据密钥和校验迁移版本 → 数据库迁移与校验 →
恢复未决尝试状态 → 构造账号执行器 → 绑定 HTTP → 启动允许运行的账号。
迁移前生成受保护安全备份，失败不继续；首次创建才允许生成新数据密钥。
已有数据密钥缺失或 DPAPI 失败时停止，不能新建密钥覆盖旧内容。

控制台仅显示版本、profile、回环管理地址、数据目录和状态摘要。
首次运行访问管理页完成密码初始化，不使用默认密码。不存在管理员时不启动平台写动作。
管理页关闭不停止值守。`Ctrl+C` 首次触发停止接新任务、持久停止意图、等待已提交结果；
到达有限等待期限后将无法确定的操作保留 unknown，再退出并释放锁。
明确区分正常退出和存在待人工核对的退出码 7，不宣称已发操作可撤回。

恢复数据带 quarantine 时始终先暂停；不因数据库保存了旧 enabled 字段恢复自动发货。
账号状态、有效授权及可信订单齐全之前不显示在线或可自动交付。

## init-admin

```powershell
.\qing-delivery.exe init-admin --data-dir 'D:\QingDeliveryData'
.\qing-delivery.exe init-admin --data-dir 'D:\QingDeliveryData' --reset
```

用途为无浏览器初始化或丢失管理密码后的本机交互恢复。服务必须停止并取得目录独占锁；
要求同 Windows 用户可解密数据密钥。首次运行无管理员时录入两次密码，至少 12 个字符。
已存在管理员而未给 `--reset` 返回前提错误；reset 明确显示将使所有管理会话失效并要求交互确认。

重置只改变 Argon2id 密码哈希、废弃会话并记录本地审计，不解密展示账号凭证、不改变数据密钥、
不清空订单，不解除恢复隔离。为避免失控恢复，reset 后所有账号保持/设置暂停，再由管理员明确恢复。
Ctrl+C 在事务提交前不产生修改；已提交必须报告成功，不把交互输出中断等同未执行。

## backup

首版只支持停止服务后独占备份；使用 SQLite online backup API 并不要求应用在线。

```powershell
.\qing-delivery.exe backup --data-dir 'D:\QingDeliveryData' --output 'D:\QingDeliveryBackups\manual.qdbak'
```

- 取得目录锁并验证 DPAPI；服务正在运行时退出码 3，提示先安全停止，不绕过锁读取数据。
- 使用 SQLite backup API，不能裸复制数据库忽略 WAL。
  备份冻结密钥轮换，保存数据密钥 ID、schema/app/格式版本、实例 ID、快照开始/完成时间和校验信息。
- 内容是完整加密归档，使用随机归档密钥，DPAPI 当前用户包装；包含恢复所需已包装数据密钥，
  不包含明文密钥或管理密码。认证加密绑定 manifest，校验和用于传输校验，不能替代认证加密。
- 先写目标同目录临时文件，成功校验后原子发布；目标已存在时拒绝，禁止静默覆盖。
  完成显示归档路径、sha256、快照时间、同机器同用户恢复限制及退出码 0。
- Ctrl+C 只取消尚可取消的备份工作并删除本次未发布临时文件，不改变业务数据。
  不留下看似完整但尚未认证发布的归档。
- 快照生成之后原服务可能重新启动并继续发送，因此恢复仍然必须进入核对隔离。

## restore

```powershell
.\qing-delivery.exe restore --input 'D:\QingDeliveryBackups\manual.qdbak' --data-dir 'D:\QingDeliveryData'
```

仅支持同机器、同 Windows 用户；服务必须停止。先获得目标目录独占锁，检查归档格式、
认证加密、DPAPI、schema 兼容性及数据库完整性，任何一步失败保持现有数据不变。
已有数据时显示快照与当前版本，明确提示回退风险并交互确认；不存在在线恢复 API。

恢复流程：

1. 在同卷受限暂存目录解密并校验，不直接覆盖工作数据库；清理残留 WAL/SHM 仅限受控暂存路径。
2. 原有效目录先生成受保护保留副本；空间不足或备份失败则终止恢复。
3. 在恢复库内事务写入 restore 记录、全部账号暂停、强制隔离和会话失效；原去重历史不删除。
4. 为快照中未完任务建立核对记录，记录快照到恢复的可能发送区间，未知尝试继续 unknown。
5. 发布恢复目录/文件时使用可恢复切换标记；崩溃后 serve 检测未完成切换即停止或完成安全回退，
   不同时接入新旧数据库。保留同一稳定锁文件位置，不能更换持锁对象后留下并发窗口。
6. 完成显示“恢复成功，全部账号暂停，需核对后启用”；退出码 0 不表示已恢复自动发货。

不能按当前 `pending_ship` 就证明历史文字未发过。管理页逐单核对/人工记录，
只解除有证据订单的隔离，旧任务须明确补发/接管。账号总开关不能批量解除旧任务隔离。
管理员明确恢复账号后，可信付款时间晚于恢复完成、非旧任务的新订单可正常处理。
未来 schema 版本拒绝降级读取；支持的旧版本先在暂存区迁移并验证。升级失败保留原数据及归档。

## mock 开发 profile

```powershell
.\qing-delivery.exe serve --profile mock --data-dir 'D:\QingDeliveryMock' --bind '127.0.0.1:59189'
```

mock 模式用于前后端联调、快速入门演示及确定性故障场景，不能作为实账号验收证据。
该能力仅启用Cargo feature `dev-fixtures` 的开发/测试构建提供；正式发行禁止该feature，
不接受 mock 参数或明确报不可用，不静默切 live。

- 必须显式 `--profile mock` 和独立 `--data-dir`；目录元数据标记 mock，不允许 live 复用或互相迁移。
- UI 常驻“模拟环境，不发送真实消息”，capabilities 返回 execution_profile=mock。
  没有默认管理密码，仍需初始化、会话和 CSRF。
- 注入进程内模拟适配器、可控时钟和脱敏 fixtures；禁止访问真实平台网络，
  禁止真实二维码、Cookie、Token 导入，禁止读取 live 数据目录和用户真实浏览器 profile。
- 仅 mock 构建提供 `/api/v1/dev/scenarios` 列表和
  `POST /api/v1/dev/scenarios/{scenario}/runs`（鉴权＋CSRF＋幂等键），返回 202 job。
  场景固定 allowlist：普通付款、多规格、重复事件、发送结果未知、取消退款、恢复缺口。
  不接收任意 JS、URL、文件路径或原始凭证；live 路由不存在并返回 404。
- 固定夹具应覆盖同批多订单、缺 cardList 空列表和 ACK 不足等协议边界。
  mock 备份标记 profile=mock，restore 拒绝导入 live 目录；开发产物和模拟数据不能进正式包。

## 验证要求

验证 help/参数错误、端口冲突、独占锁、不同路径指向同目录、live/mock 隔离、
无开发工具启动、迁移失败、密钥缺失、DPAPI 用户不匹配、备份中断、归档损坏、
恢复发布中断和 Ctrl+C 未知结果。恢复后未经用户核对的历史内容发送次数必须为零。
构建、健康检查与模拟通过不代替授权实账号兼容性验证。

