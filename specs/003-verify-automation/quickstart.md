# Quickstart: 安全验证自动化验证指南

**Date**: 2026-09-23 | **Spec**: [spec.md](spec.md) | **契约**: [contracts/verification-api.md](contracts/verification-api.md)

三层验证:确定性测试(无浏览器)→ 夹具滑块页集成(需本机浏览器)→ 实账号端到端(SC-301,授权项)。不向真实买家发送任何内容(宪章 IV)。

## 前置条件

- 本机已装 Chrome 或 Edge(引擎发现自动探测;无则相关集成测试跳过并注明)。
- Rust stable(windows-gnu)+ Node ≥22(前端构建)。
- 环境变量 `QING_BROWSER_TESTS=1` 开启浏览器集成测试;未设置时跳过(输出 SKIP 说明,不算失败)。

## 构建与确定性测试(不需要浏览器)

```powershell
cd frontend; npm ci; npm run typecheck; npm run test; npm run build; cd ..
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test          # 基线 211(80+131)+ 新增状态机/闸门/降级/审计测试
```

## 夹具滑块页集成(需要浏览器)

```powershell
$env:QING_BROWSER_TESTS = "1"
cargo test --test browser_integration     # 自建滑块页:定位→轨迹拖动→成功判定→Cookie 读取
cargo test --test browser_lifecycle       # 空闲回收/退出清理/崩溃重建/队列上限
```

预期:全部通过;结束后任务管理器无残留 chrome/msedge 子进程(SC-303)。

## 手动走查(mock 之外的 live 数据目录)

```powershell
cargo run --release -- serve --data-dir .work\verify-demo --bind 127.0.0.1:59194
```

1. **手动触发**:账号页"发起验证"→ 进度分步呈现(detected→browser_open→solving→…);完成后审计可见(方式=手动)。
2. **失败降级**:设置环境 `QING_FORCE_SLIDER_FAIL=1`(实现提供的故障注入开关)重触发→ 重试 2 次后转人工;待处理出现"安全验证"分组含可点链接;5 分钟内无再次自动拉起。
3. **人工恢复**:点事项链接完成(或清除注入)→ 60 秒内事项自动关闭、账号恢复在线。
4. **无浏览器环境**:临时重命名浏览器探测路径(或 `QING_NO_BROWSER=1`)→ 触发即明确报"浏览器不可用+指引"并转人工,不伪称尝试。
5. **进程卫生**:处置完成 5 分钟后与服务 Ctrl+C 后,`Get-Process chrome,msedge | Where-Object {$_.MainWindowTitle -like '*轻交付*'}` 为空。

## 实账号端到端(SC-301,授权门禁)

真实账号长期值守至平台触发风控(或按平台提示手动触发),观察:自动完成滑块→凭证更新→5 分钟内账号恢复在线、无需人工。结果记入 docs/evidence/real-account-matrix.md(R-矩阵新增 R11),含:触发信号原始片段、URL 来源字段、滑块选择器、成功判定依据——这些平台 specifics 以实账 号观察为准回填实现。

## 通过标准

- 确定性测试全绿,基线 ≥211 不回退(SC-306);不变量测试零修改通过。
- 夹具集成通过且零残留进程(SC-303)。
- 手动走查 5 项全过(降级 100% 可靠,SC-302)。
- 48 小时常驻零误拉起(SC-304)后,实账号项按 R11 记录(通过/失败均如实)。
