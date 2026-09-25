# Research: 运营概览统计首页(005-operations-dashboard)

**Date**: 2026-09-25 | **Spec**: [spec.md](spec.md) | 规格层无遗留 NEEDS CLARIFICATION(3 项已在 clarify 固化),本文记录技术决策。

## R1. 统计端点形态:additive 新端点 vs 扩展既有 /api/v1/dashboard

- **Decision**: 新增 `GET /api/v1/stats/overview?from={ms}&to={ms}`,既有 `/api/v1/dashboard` 原样保留。
- **Rationale**: 002 契约注释明确 dashboard 增量字段走 additive 且旧消费者不破坏;新端点承载全新查询语义(区间化),避免把 002 的"今日口径"字段与 005 的"区间口径"混在同一载荷里造成双语义。旧端点仍服务旧字段,后续由 tasks 决定是否迁移删除(不在本特性内)。
- **Alternatives considered**:
  - 扩展 `/api/v1/dashboard` 加 `from/to` 参数——破坏"无参即今日摘要"的稳定性,旧前端与新页面共享载荷易出错;弃。
  - 每张卡一个端点——四次往返、口径分散;弃。

## R2. 时间参数与分桶的时区处理

- **Decision**: 前端把预设范围(自然日对齐)解析为 `from/to` epoch 毫秒后传参;服务端用 chrono `Local`(与既有 `time_util::local_midnight_ms` 同一惯用法)解释并再次校验,分桶边界全部在 Rust 侧预计算。DST 解析失败沿用既有回退策略(回退入参,宁多算不报错)。
- **Rationale**: 本机单进程部署,前端与服务共享同一本地时区;自然日对齐(澄清 Q2)与"今天"既有口径一致。分桶在领域层做纯函数(`bucket_ranges(range) -> Vec<(start_ms,end_ms)>`),SQL 只取原料,口径可单测。
- **Alternatives considered**:
  - SQL 内 `paid_at/86400000` 整除分桶——DST 使本地自然日边界在 UTC 毫斯上不等宽,会有错桶;弃。
  - 传预设名(`range=7d`)由服务端解析——把显示语义挪到后端,自定义与预设两套参数;弃。
- **粒度规则**(spec FR-007):`to - from ≤ 48h` 按本地自然小时,否则按本地自然日;服务端计算并在响应中回显,前端不自行决定。

## R3. 聚合执行位置:SQL 聚合 vs 取原料在领域层分桶

- **Decision**: SQL 只做区间过滤(`WHERE paid_at >= ? AND paid_at < ? AND paid_at IS NOT NULL`)取 `(paid_at, amount_minor, currency)` 原料,营收合计/对比/趋势分桶全部在领域层纯函数完成;账号在线数、开放事项数用既有 repo 查询(`COUNT`)。
- **Rationale**: 万级数据取原料 <几 MB,单次查询走 `idx_orders_paid_at`,3s 目标余量巨大(R3 实测预期 <100ms);口径(CNY 过滤、缺失金额排除、退款不回冲=不过滤状态)集中在 `domain/stats.rs` 可单测,符合 FR-011"口径由领域层定义"。
- **Alternatives considered**: `GROUP BY` 在 SQLite 内分桶+求和——快但口径散落 SQL 字符串,测试需起库;若未来数据量涨到十万级,tasks 可在 repo 内加聚合查询替换原料查询,接口不变(预留优化点,首版不做,宪章 III"以测量决定优化")。

## R4. 营收对比徽标(comparison)计算与表示

- **Decision**: 服务端计算前一等长区间 `[from-(to-from), from)` 的营收合计,响应给 `previous_minor_units` 与 `change_percent`(整数,四舍五入,带符号);`previous == 0` 或缺失时 `change_percent: null`,前端隐藏徽标(spec FR-003/SC-005)。
- **Rationale**: 口径(等长区间、百分比取整)放服务端保证"统计卡与核对结果 100% 一致"只验证一处;整数百分比与参考项目 "+0%" 展示形态一致,避免浮点串位。
- **Alternatives considered**: 前端自行再查一次 `from/to` 平移区间——多一次往返且对比逻辑散落前端;弃。

## R5. 趋势图渲染方案

- **Decision**: 手写 SVG 图表组件(`frontend/src/ui/chart.ts`):柱状图(小时粒度)/按日粒度亦柱状,空值桶画零高度柱,hover title 提示金额与笔数;空态走既有 `empty-state` 样式。
- **Rationale**: 页面仅一个图表场景,宪章 III 轻量原则下不引入 Chart.js/ECharts(数百 KB 且带框架假设);SVG 是 DOM 节点,可被 vitest/jsdom 断言,与现有原生 TS DOM 风格一致。
- **Alternatives considered**: Chart.js(canvas,不可 DOM 测试、体积大);纯 CSS 柱图(坐标轴/提示难做);均弃。

## R6. 鉴权与错误码

- **Decision**: 沿用 `require_admin_public(&state, &headers, false)`(与 GET /dashboard 同级,CSRF 免除只读);参数非法/区间倒置/超 92 天返回 400 `ErrorCode::InvalidRequest` 语义,服务不可用沿用 `PersistenceUnavailable`。
- **Rationale**: 与 002 契约错误模型一致,前端 `http.ts` 已有统一错误路径;无新错误码。

## R7. 前端范围状态与刷新交互

- **Decision**: 当前范围存于页面闭包(不写 URL、不持久化),30s 可见轮询与手动刷新按当前范围重查(FR-013);切范围原地更新卡值与图表(FR-015);恢复隔离/停止横幅沿用既有 `deriveStats` 逻辑,数据来自新端点同响应(账号快照/事项数一并返回,避免同屏双请求口径打架)。
- **Rationale**: 单页面局部状态,无分享/深链需求;一次请求装配整屏统计,保证四卡与趋势来自同一时刻快照,轮询成本 1 req/30s。
- **Alternatives considered**: 范围写入 query string 支持深链——超出 spec 范围;弃(记录为后续可选)。

## R8. 既有 overview 页区块兼容

- **Decision**: 新统计区(标题区/筛选/四卡/趋势)插入页面顶部;账号速览、最近待处理、最近订单三区块逻辑不动,继续用各自既有端点;既有四张旧统计卡由新四卡替代(不保留双份"今日订单"卡)。
- **Rationale**: spec FR-014 要求保留运营区块;旧卡口径(今日)被区间卡覆盖,双份数字并存会互相矛盾(7天内 ≠ 今日)。
