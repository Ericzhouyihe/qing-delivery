# 轻交付(qing-delivery)

本地自托管的闲鱼虚拟商品自动发货管理工具:买家付款后自动发送固定的资料链接与提取码,
可靠保存订单与发货记录,提供查询、人工补发与备份恢复能力。

- **后端**:Rust 单二进制(内嵌管理页面),本地 SQLite(WAL + FULL),Windows 11 x64
- **前端**:原生 TypeScript + Vite 构建后嵌入二进制;普通用户无需 Node/Rust
- **访问边界**:仅绑定回环地址(默认 `127.0.0.1:59189`),不暴露局域网

## 快速开始

```powershell
.\qing-delivery.exe serve                       # 默认数据目录 %LOCALAPPDATA%\QingDelivery\data
.\qing-delivery.exe serve --data-dir D:\QingData --bind 127.0.0.1:59189
```

浏览器打开 `http://127.0.0.1:59189`,首次运行设置两次一致、至少 12 字符的管理员密码。
无账号时显示空状态,不会自动连接平台。健康检查:

```powershell
Invoke-RestMethod -Uri 'http://127.0.0.1:59189/health'
```

`Ctrl+C` 停止:不再接受新任务,已提交操作的结果或未知状态保留。管理页面关闭不影响值守。

## 账号与自动发货

1. 「账号」页扫码接入闲鱼账号(二维码 3 分钟有效,超时可重新获取)
2. 首次启用"自动交付"必须确认监控范围:生效时间之前的订单仅展示,需逐单显式接管
3. 三个开关独立:运行、自动交付、自动平台确认;新账号默认全部关闭
4. 商品规则:为商品(或完整规格组合)配置一段固定文字;正文 ≤1000 字符/4000 字节,
   保留换行与链接,不截断、不拆分;预览不会向买家发送

## 安全模型

- 管理密码 Argon2id;会话 12 小时绝对有效期,HttpOnly + SameSite=Strict + CSRF
- 数据密钥由 Windows DPAPI(当前用户)包装;凭证、规则正文、交付快照均为 AES-GCM 密文
- 列表/日志/诊断不包含 Cookie、Token 或完整交付内容;正文只在授权订单详情出现

## 备份与恢复(停机执行)

```powershell
# 服务停止后
.\qing-delivery.exe backup  --data-dir D:\QingData --output D:\Backups\manual.qdbak
.\qing-delivery.exe restore --input  D:\Backups\manual.qdbak --data-dir D:\QingRestore
.\qing-delivery.exe serve   --data-dir D:\QingRestore
```

- 备份是加密归档(AES-GCM + 校验和);**仅同机同 Windows 用户可恢复**
- 恢复后所有账号暂停、全部会话失效;备份期间的未完任务与"快照后可能已发货"的订单
  进入逐单隔离,须在「设置」页核对(received / approved_not_sent / terminated)后才能处理;
  **重新启用账号总开关不能清除逐单隔离**
- 版本兼容:仅支持恢复到相同或更新 schema;拒绝降级

## 无浏览器初始化/密码重置

```powershell
.\qing-delivery.exe init-admin --data-dir D:\QingData          # 首次
.\qing-delivery.exe init-admin --data-dir D:\QingData --reset  # 忘记密码(交互确认)
```

`--reset` 只重置密码哈希、废弃会话并暂停全部账号;不解密展示凭证、不清订单、不解除隔离。

## 支持与不支持(首版)

| 支持 | 不支持(明确拒绝,不显示为可用) |
| --- | --- |
| 闲鱼普通已付款订单、完整规格匹配、固定文字交付 | 淘宝千牛、卡密库存、按件独立权益 |
| 多账号分别控制;漏单补偿;未知结果人工核对;受控补发 | 小刀/免拼、自动议价、评价/上架等运营功能 |
| 本地备份恢复、版本化迁移 | 网络共享目录数据、跨机器恢复、多实例同目录 |

退出码:`0` 成功;`2` 参数/前提;`3` 目录被占用;`4` DPAPI 失败;`5` 数据库/备份;`6` 版本不兼容;`7` 停止超时待核对。

## 开发

```powershell
scripts\verify.ps1        # 全部质量门禁(fmt/clippy/test/build/前端三件套)
scripts\release.ps1       # 白名单发行打包
cargo test --locked --all-targets
npm ci --prefix frontend; npm run test --prefix frontend -- --run
```

详见 `specs/001-xianyu-auto-delivery/`(需求/计划/数据模型/契约)与 `docs/`(依赖验证、发行说明)。
