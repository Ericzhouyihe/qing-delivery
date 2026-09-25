# Data Model: 安全验证自动化(003-verify-automation)

**Date**: 2026-09-23

新增两处持久化(迁移 0002,版本化+受保护备份机制沿用)+ 内存态实体。既有表零修改语义,仅 issues 增列。

## 1. 持久化变更(迁移 0002)

### verification_attempts(审计,只增不改)

| 列 | 类型 | 说明 |
|---|---|---|
| id | TEXT PK | `vat-` 前缀 |
| account_id | TEXT NOT NULL FK | 关联账号 |
| trigger_source | TEXT NOT NULL | `mtop` / `ws` / `qr` / `manual` |
| trigger_reason | TEXT NOT NULL | 信号摘要(ret 片段/手动) |
| verification_url | TEXT | 打开的验证页 URL(转人工链接同源) |
| started_at / finished_at | INTEGER NOT NULL / INTEGER | 毫秒;进行中 finished_at 为 NULL |
| outcome | TEXT NOT NULL | `succeeded` / `failed_manual` / `in_progress` |
| duration_ms | INTEGER | 结束时回填 |
| credential_updated | INTEGER NOT NULL DEFAULT 0 | 凭证是否成功回写 |
| failure_reason | TEXT | 失败原因(每次尝试独立一行,满足 US2-3) |

不变量:行只插入与回填终态,不删除不改写历史(FR-010)。

### issues 增列 metadata(TEXT,可空,json)

- 既有行默认 NULL,旧消费者不受影响(additive)。
- `security_verification` 类事项:`{ "verification_url": "...", "attempt_ids": [...] }`。
- allowed_actions 约定:`["open_verification","resolve"]`(打开验证页 / 人工确认已处理)。

## 2. 内存态实体(不落库)

### VerificationSession(验证会话状态机,研究 D3/D5)

```text
detected ──(浏览器可用?否→failed_manual)
   │ 是
   ▼
browser_open ──→ solving ──→ updating_credential ──→ succeeded
   │超时/崩溃         │滑块不过          │保存失败
   └── retry(≤2) ─────┴───────────────┴──→ failed_manual
```

| 字段 | 约束 |
|---|---|
| account_id | 单账号同时至多 1 个活跃会话(重复信号去重合并) |
| state | 上述 7 态;终态后释放交付闸门 |
| retry_count | <2 可重试;总超时 120s(单调时钟) |
| deadline | 超时即 failed_manual,不紧密重试 |
| gate | 与 supervisor 的 per-account 交付闸门联动(D5):会话活跃=闸门关闭 |

### VerificationSignal(统一触发事件,D2)

`{ account_id, source: mtop|ws|qr|manual, url: Option<String>, raw: String }`——仅明确信号构造;关键词集合:`punish`、`FAIL_SYS_USER_VALIDATE`、`rgv587`、`x5secdata`、`滑块`(收窄,SC-304 误报为零)。

### BrowserInstancePolicy / BrowserManager(D7)

- Policy:`max_concurrent=1`、`idle_reap=5min`、`queue_timeout=60s`、`headless=false(默认)`。
- Manager 状态:`idle | busy(account) | queued(n)`;崩溃重建计入会话 retry;shutdown 全量清理并挂 runtime_stop。
- user-data-dir:`<data_dir>/browser-profile`;启动前目录锁检测,被占/损坏→如实报错转人工(边界条款,不删目录)。

### SolveOutcome(Driver 返回)

`Solved { cookie_jar } | Failed { reason, stage }`——Driver 不决定重试(服务层职责,宪章 II)。

## 3. 实体关系

```text
VerificationSignal(3源+manual) ──去重──> VerificationService ──驱动──> VerificationDriver(浏览器)
                                              │ 闸门                       │ Solved
                                              ▼                           ▼
                                    supervisor(交付延迟)          credentials::save + epoch+1
                                              │                           │
                                              ▼                           ▼
                                     verification_attempts(审计)   supervisor 会话热更新
                                              │ failed_manual
                                              ▼
                                    issues(security_verification, metadata.url)
                                              │ 人工完成/凭证恢复探测(60s)
                                              ▼
                                          自动关闭事项
```

## 4. 校验与不变量(来源于规格)

- 降级后自动重试间隔 ≥5 分钟(FR-004):服务层冷却时间戳,不在会话内紧密循环。
- 凭证回写失败保留旧凭证(FR-014):save 成功后才 bump epoch;失败路径不触碰旧值。
- 审计完整(FR-010):每次尝试一行;转人工事项引用 attempt_ids。
- 验证流程不改交付状态机语义(FR-013):闸门只延迟触发,不改变订单/交付/guard 任何状态值。
- 并发上限排队(FR-007):队列超时(60s)的信号直接 failed_manual 转人工(边界:风控风暴)。
