# 验收矩阵与覆盖对照(T090)

日期:2026-09-23 | rustc 1.97.1(gnu)| Windows 11 x64 | 应用 0.1.0
执行命令:`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、
`cargo test --locked --all-targets`(97 项)、`cargo test --test e2e -- --test-threads=1`、
前端 `npm run typecheck/test/build`、`scripts/release.ps1`、CLI 实测(backup/restore/serve)。

## V01—V12 执行对照

| 编号 | 状态 | 证据(测试/命令) |
| --- | --- | --- |
| V01 初始化与访问 | ✅ | `contract::foundational_http`(并发唯一约束、短密码 400、登录退出、CSRF/Host/Origin、静态页);限流 `login_limiter` |
| V02 账号生命周期 | ✅ 确定性子集 | `contract::accounts_http`(QR 创建/轮询/图片 no-store/取消幂等/404);`application::accounts::qr` 单测(晚到成功不覆盖新代次);`runtime::tests`(退避);**真实断线/验证矩阵待 SC-091** |
| V03 商品与规则 | ✅ | `integration::catalog_rules`(空页正常、部分失败不下架、完整同步下架、冲突规则、超长拒绝、组合作用域);`application::catalog::rules` 单测(1000/4000 边界) |
| V04 正常交付 | ✅ | `integration::delivery_core::happy_path/quantity_over_one/confirmation_*`;`e2e::sc002`(100 单 100% 正确) |
| V05 非法事实与去重 | ✅ | `integration::delivery_core::twenty_duplicates/invalid_facts/rejected`;事件层 `application::events`(终态不倒退、坏条目不吞) |
| V06 崩溃与数据库故障 | ✅ 确定性子集 | `integration::delivery_core::unknown_result_never_resends`;`integration::recovery_manual::boot_recovery`(dispatching→unknown+required);**进程级注入矩阵在服务化执行器接入后补全** |
| V07 控制竞争 | ✅ 确定性子集 | `integration::recovery_manual::pause_race_with_delivery`(并发暂停至多一次发送);handoff 前代次重查(`service.rs`);第二实例锁(`lock.rs` 单测 + 冒烟退出码 3) |
| V08 补偿与历史 | ✅ | `integration::recovery_manual::compensation(120 秒窗口)/historical_takeover/trace_scan`;`e2e` 不含——见上 |
| V09 人工动作 | ✅ | `integration::recovery_manual::manual_resend(冻结快照原文+幂等重放)/mark_received(manual 证据)/refunded_rejects/auto_delivery_off_blocks` |
| V10 查询与隐私 | ✅ | `integration::query_backup::order_filter_and_cursor/privacy_lists_never_contain_content`;`e2e::sc006`(10k P95≈26ms ≤2s) |
| V11 备份恢复升级 | ✅ | `application::backup::restore::tests`(闭环:隔离/manual_only/账号暂停/会话撤销;占用拒绝;篡改检出)+ CLI 实测(backup→restore→serve→login 200) |
| V12 独立发行 | ✅ 确定性子集 | `scripts/release.ps1` 白名单打包 + dist 独立目录冒烟(health ok/页面 200);**无开发工具目标机与浏览器存在/缺失矩阵待 SC-091 环境** |

## SC 覆盖

| SC | 状态 | 证据 |
| --- | --- | --- |
| SC-001 5 分钟配置 | ✅ 服务层计时 1.4ms;人工浏览器计时待演示记录 | `e2e::sc001_rule_config_path_timing` |
| SC-002 100 单交付 | ✅ 100/100 正确、每单一次、506ms(本地);真实平台 P95 另计 | `e2e::sc002_hundred_orders_correctness` |
| SC-003 去重 | ✅ 20×重复/并发/乱序每单 ≤1 次 | `delivery_core::twenty_duplicates…` |
| SC-004 中断点 | ✅ 确定性子集(unknown 不重发/重启保持) | V06 行 |
| SC-005 漏单 5 分钟 | ✅ 补偿窗口+追溯不重复;时间预算按 60s 周期语义设计 | `recovery_manual::trace_scan/compensation` |
| SC-006 查询 P95 | ✅ 10k 记录 P95≈26ms(≤2s);页面异常 ≤10s 由轮询(3s)保证 | `e2e::sc006_ten_thousand_query_p95` |
| SC-007 无开发工具运行 | ✅ dist 冒烟;全目标机矩阵待验 | 发行冒烟记录 |
| SC-008 备份升级 | ✅ 归档保留配置/订单/正文/去重;恢复不重放 | `backup::restore::tests` |
| SC-009 授权实单 | ⏳ **待验证**(未获授权;版本标"待验证") | 见 docs/evidence/real-account-matrix.md |

## FR 覆盖

- FR-001—005(身份/接入/开关/状态/值守):V01/V02 + `accounts` 单测 ✅
- FR-006—009(商品/规则):V03 ✅
- FR-010—018(订单/交付/去重/监控):V04/V05/V08 + `eligibility` 单测 ✅
- FR-019—024(恢复/人工):V06—V09 ✅
- FR-025—028(查询/隐私/持久化/备份):V10/V11 ✅
- FR-029—031(本机边界/单实例/能力声明):capabilities 端点 + 目录锁 + 回环校验 ✅
