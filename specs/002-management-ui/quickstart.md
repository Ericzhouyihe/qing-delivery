# Quickstart: 管理界面重构验证指南

**Date**: 2026-09-22 | **Spec**: [spec.md](spec.md) | **契约**: [contracts/](contracts/)

本指南用 mock/dev 执行配置在本地端到端验证界面重构。不使用真实账号、不发送真实消息(宪章 IV)。

## 前置条件

- Node.js ≥22(仅构建前端;终端用户运行无需 Node)。
- Rust stable(windows-gnu 工具链,rust-toolchain.toml 已锁定)。
- PowerShell 5.1+(或等价 shell)。

## 构建

```powershell
# 前端:类型检查 → 行为测试 → 构建(产物进 src/webui,由 cargo 嵌入)
cd frontend; npm ci; npm run typecheck; npm run test; npm run build; cd ..

# 后端:格式/静态检查/测试(SC-206 回归门禁,97 项基线不得回退)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 启动(带场景夹具)

```powershell
cargo run --features dev-fixtures -- --execution mock --data-dir .work/ui-002 --port 59190
# 首次访问 http://127.0.0.1:59190 完成初始化(密码 ≥12 字符),或用 init-admin CLI
```

夹具加载参考 001 的 dev/scenarios 端点与既有种子方式;无种子时页面必须呈现空态引导而非空白(US 各页空态场景)。

## 逐项验证场景(对应规格验收)

1. **布局框架(SC-201/US1)**:登录后六页切换,侧边栏高亮一致;主题切换且刷新后记忆;窄窗口侧栏折叠;构造请求失败(停服)看错误块+重试。
2. **概览(US2/SC-204)**:统计卡数字与订单页实际条目一致;待处理非零时警告色并可下钻;30 秒无操作数字自动刷新且无布局跳变(US2-6);恢复隔离横幅出现时点击直达核对。
3. **账号(US3)**:空态引导→扫码弹窗(倒计时/轮询/过期重取);启停确认流与"暂停中"过渡态。
4. **规则(US4)**:筛选"仅已配置";同步结果反馈;抽屉表单行内校验(空内容/超字数拒绝保存);未保存关闭需确认;匹配预览:选商品+规格 → matched/none/ambiguous 三种结果呈现,最终内容由 `rules/preview` 渲染。
5. **订单(US5/SC-203)**:组合筛选+翻页保留条件;详情抽屉三轴时间线与尝试记录;内容默认折叠可揭示;guard 禁止动作呈禁用态+原因;概览→订单详情 ≤3 次交互。
6. **待处理(US6)**:分组与计数;确认流原因必填(空原因无法提交);成功后条目移出、计数联动;处理完显示正向空态。
7. **设置(US7)**:运行信息与实际一致;恢复入口展示后果说明。
8. **登录/初始化(US8)**:密码错误行内提示;会话过期后操作回落登录,重登回原页面;初始化两次密码不一致被阻止。
9. **未知枚举(SC-205)**:在夹具数据中注入未收录状态值 → "未知状态"徽章+原始值悬浮,无崩溃。

## 性能与体积(SC-207)

```powershell
# 产物体积:前端构建输出总体积(vite 报告)压缩后 ≤1.5MB
# 首屏可交互:浏览器 DevTools Performance/Network,本地环境概览页 ≤2s
# 版本一致性:src/transport/webui.rs 的 index.html 重建跟踪仍生效
#   (改前端→npm run build→cargo build→确认磁盘与服务端资源哈希一致,001 已有此坑的回归验证)
```

## 通过标准

- 全部构建/测试命令零失败;`cargo test` 基线不少于 97 项且全绿(SC-206)。
- 上文 9 组场景逐条走查通过并记录(视觉走查清单:布局一致/语义色一致/三态覆盖)。
- release 白名单打包(`scripts/release.ps1`)产物可独立运行(不依赖 Node/参考目录)。
