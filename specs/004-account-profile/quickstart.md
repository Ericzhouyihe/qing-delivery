# Quickstart: 004-account-profile 验证指南

**Spec**: [spec.md](spec.md) | **契约**: [contracts/accounts-api.md](contracts/accounts-api.md)

## 确定性测试(无需真实账号)

```powershell
cargo fmt --check; cargo clippy --all-targets -- -D warnings; cargo test   # 基线 ≥226 不回退
cd frontend; npm run typecheck; npm run test; npm run build
```

覆盖:资料解析(displayName→displayNick→空)、迁移 0003 列读写、删除三步(凭证 0 残留/订单保留/重扫码新账号)、备注校验、items 新字段契约。

## 实账号走查(live,59189)

1. **首拉**:重新扫码接入新账号(或对现有账号)→ 30 秒内账号卡显示真实昵称+头像(SC-401)。
2. **刷新资料**:点"刷新资料"→ ≤5s 更新;改名后刷新可同步。
3. **ID+复制**:账号卡完整 ID;点复制到剪贴板。
4. **备注**:编辑备注保存→刷新仍在。
5. **删除**(用一个测试账号):确认流(输入 ID 片段)→ 列表消失;订单中心历史仍在;凭证目录核验 `account_credentials` 该账号行=0;同账号重扫码=全新账号。
6. **头像回退**:断网/防盗链时回退首字占位,无破图。

## 通过标准
上述 6 项全过;基线 226+ 不回退(SC-404);实账号字段名差异记 R11。
