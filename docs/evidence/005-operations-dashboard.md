# 走查证据:005-operations-dashboard(quickstart S1~S7)

**日期**:2026-09-25 | **方式**:真实运行服务(隔离数据目录,127.0.0.1:59293)+ 浏览器实操 + 契约/单元测试

## Phase 7 收敛补验($speckit-analyze → $speckit-converge → $speckit-implement,2026-09-25)

| 项 | 结果 | 证据 |
|----|------|------|
| 宪章门禁:`cargo fmt --check` | ✅ PASS(修复了 repos/stats.rs、domain/stats.rs、time_util.rs、application/stats.rs、tests/contract/stats_http.rs 的格式 diff) | 本节记录,命令可复跑 |
| 宪章门禁:`cargo clippy --all-targets` | ✅ 0 警告(修复"time_util.rs 测试模块位置""stats_http.rs 复杂类型"两条) | 同上 |
| T022 竞态守卫 | ✅ `loadStats` 增序号守卫;page.test.ts「旧范围迟到响应不覆盖新范围数据」通过 | frontend/src/features/overview/page.test.ts |
| T025 未付款用例 | ✅ 种子增 o9(paid_at NULL),契约断言订单数不含未付款单 | tests/contract/stats_http.rs |
| T026 样本 ≥10 组 | ✅ change_percent 单测 13 组断言(含 .5 远离零取整边界) | src/domain/stats.rs |
| T027 失败占位 | ✅ 500 → 趋势区"统计暂时不可用"+重试按钮,统计卡保留占位不闪 0;重试恢复 | page.test.ts 第二用例 |
| T028 SC-001 计时 | ✅ 实测统计接口端到端时延 8 次:1.35~2.29ms(阈值 2s);页面装配+渲染路径由 page.test.ts(141ms 内完成整页工厂+全部加载)佐证 | 本节记录 |
| SC-003 口径注记 | 以"一个月内"(30 自然日、1 万订单)更宽窗口覆盖字面"7天内"场景(超集验证) | 下节性能抽查 + plan 门禁第 6 条 |
| plan 文档 | Constitution Check 补齐原则 IV/V 逐项评估与门禁六问;Testing 补记 fmt/clippy;项目树路径修正 | specs/005-operations-dashboard/plan.md |

## 环境与种子

- `cargo build`(前端产物已嵌入)+ `qing-delivery.exe serve --data-dir .e2e-run --bind 127.0.0.1:59293`
- 管理员经 `/api/v1/auth/initialize` 创建;经浏览器登录页进入工作台
- 种子:2 账号(1 在线/1 离线)、8 订单(o1 ¥10 今日 CNY;o2 今日 USD;o3 今日无金额;o4 今日 ¥8 退款态;o5 昨日 ¥25.50;o6 前日 ¥15;o7 前日 ¥40;o8 4 天前 ¥22.50)、2 开放事项

## 逐项结果

| 场景 | 结果 | 证据要点 |
|------|------|---------|
| S1 空态 | ✅ | 自定义 2020-01-01~01-07:营收 ¥0.00、徽标隐藏、趋势区"暂无营收数据/所选时间范围内暂无订单记录" |
| S2 口径 | ✅ | 7天内:¥121.00/8 单(o2 USD、o3 无金额排除合计但计入订单数;o4 退款不回冲);今天:¥18.00/4 单、徽标 -29%(对比昨日 ¥25.50);自定义 9/10~9/24:¥103.00/4 单,徽标隐藏(前区间无订单) |
| S3 范围切换 | ✅ | 今天(小时粒度,轴标 0:00~21:00)/昨天/三天内/7天内/一个月内原地更新;自定义应用后轮询与刷新保持区间 |
| S4 自定义 | ✅ | 倒置区间:行内提示"区间无效…",`fetch` 未发起;恰 92 天通过、93 天拒绝(单测);应用后 active chip=自定义 |
| S5 保留区块 | ✅ | 账号状态速览/最近待处理/最近订单照常;活跃账号→/accounts、订单数→/orders、待处理→/issues 下钻在位 |
| S6 权限只读 | ✅ | 401/400 三态/缺参见契约测试 `tests/contract/stats_http.rs`(9 用例);只读不变式:连续查询前后库零变化 |
| S7 停止/恢复隔离 | ✅ | 恢复隔离:置顶 warning 横幅 + 右上状态"● 恢复隔离中 · 待核对 1 笔",统计照常展示 |
| SC-003 性能 | ✅ | `overview_一万订单三十天窗口三秒内返回`:1 万订单播种+查询总计 0.16s(阈值 3s) |

## 截图

- `C:\Users\Administrator\.zcode\cli\artifacts\sess_84838b34-6a19-4686-b7cd-afa881882a04\call_5a33751cd2e7452f9ac0d8a3-tool-result-d9b539cf-bb93-49bc-8c04-5a9fccdee87c.png`(7天内总览,含趋势图)
- `C:\Users\Administrator\.zcode\cli\artifacts\sess_84838b34-6a19-4686-b7cd-afa881882a04\call_bd598a53bd454114afaed9e6-tool-result-8a697454-874f-44aa-8b0e-c40493c2f2f5.png`(今天视图:-29% 徽标、小时分桶)
- `C:\Users\Administrator\.zcode\cli\artifacts\sess_84838b34-6a19-4686-b7cd-afa881882a04\call_2374fe216f254039bde89f1d-tool-result-431233db-2554-4899-b7bc-a7c579d9b265.png`(恢复隔离横幅+状态)

## 走查中发现并修复的缺陷

1. `.ov-badge`/`.range-custom` 的显式 `display` 覆盖了 `hidden` 属性 → 补 `[hidden]{display:none}` 规则(components.css)。
2. 趋势分桶测试自身桶数算错(domain/stats.rs 测试修正,实现无误)。
3. 契约测试对比徽标用例种子错位(种子补 o7 前区间订单,实现无误)。

## 备注

- 走查期间工作区存在 006-accounts-page-revamp 的并行实现(同仓库另一会话):其改动集中于 accounts/ui icons 等,与本特性文件无交集;最终全量 `cargo test`、`vitest`(134/134)、`tsc --noEmit` 均绿,表明两特性共存无冲突。
