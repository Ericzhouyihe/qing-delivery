# 依赖与工具链验证记录(research R8 实验结果)

日期:2026-09-22 | 机器:Windows 11 x64 | 实验本体:`tests/deps_validation.rs`(可重复执行)

## 结论摘要

| 实验 | 结果 | 证据 |
| --- | --- | --- |
| E1 tokio-tungstenite × 假平台 WS | ✅ 通过 | `ws_client_against_fake_platform_roundtrip`:帧下发、ACK、同批 5 条目解码 |
| E2 reqwest × 假平台 HTTP | ✅ 通过 | `reqwest_against_fake_platform_http`:脚本化 JSON 响应 |
| E3 rusqlite bundled + WAL/FULL + backup API | ✅ 通过 | `sqlite_wal_full_and_backup`:四项 pragma + 一致快照还原 |
| E4 DPAPI 当前用户包装往返 | ✅ 通过 | `dpapi_roundtrip_current_user`(经 `adapters::windows::dpapi`) |
| E5 include_dir 静态嵌入 | ✅ 通过 | `include_dir_embeds_fixtures`;发行页经 `src/webui` 嵌入并提供(契约测试 `static_index_is_served`) |

## 工具链偏差记录(相对 plan.md 预设)

1. **目标三元组改为 `x86_64-pc-windows-gnu`**(原预设 MSVC):本机未安装 VS Build
   Tools/MSVC 链接器,且 Git Bash 的 `/usr/bin/link.exe`(coreutils)污染 PATH;
   gnu 工具链自带 mingw(gcc 15.2.0 亦在 PATH)。`rust-toolchain.toml` 锁定
   `stable-x86_64-pc-windows-gnu`(实测 rustc 1.97.1,满足 ≥1.85)。
2. **精确版本 `1.97.1` 工具链目录损坏**:独立 1.97.1 目录 manifest 缺失,重装下载超时;
   改锁 stable(当前即 1.97.1)。后续在具备网络条件时重装精确版本再收紧。
3. **TLS 改用 native-tls(schannel)**:rustls 默认提供方 aws-lc-rs 在 Windows 构建
   需要 NASM(本机未装);reqwest/tokio-tungstenite/chromiumoxide 统一 native-tls。
4. **chromiumoxide 0.9.1 无 `tokio-runtime` feature**(计划时按 0.7 记忆):实际特性为
   `native-tls`/`rustls`,并需显式选择 zip 后端(`zip8`),fetcher 才能编译。
5. crates 索引走用户已配置的 tuna 镜像。

## 组合验证状态

- `cargo build` / `cargo test`(20 项:单元 10、依赖实验 5、契约 5)/ `cargo clippy
  --all-targets -- -D warnings` / `cargo fmt --check`:全部通过。
- 进程级冒烟:`serve` 启动 → `/health` ok → 匿名会话+CSRF → 初始化 201 → 登录 →
  capabilities(live,仅闲鱼)→ 静态页嵌入 → 第二实例独占锁拒绝 → 停止。
- **不宣称**:release 体积/资源预算(未测)、真实平台兼容性(未接账号)。
