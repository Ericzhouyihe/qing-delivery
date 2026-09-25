# Implementation Plan: 账号管理页面复刻与视图切换(006-accounts-page-revamp)

**Branch**: `006-accounts-page-revamp` | **Date**: 2026-09-25 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/006-accounts-page-revamp/spec.md`

## Summary

把账号页对齐 Ydisks"账号管理"参考图,补齐当前复刻缺口:内容区页头(大标题"账号管理"+副标题)、工具栏(左侧搜索框"搜索昵称 / 备注 / 账号ID",右侧双图标分段视图切换 + "扫码添加新账号"主按钮)、计数横幅("当前显示 X / Y 个账号" + 刷新资料/重新授权固定提示)、条目视觉复刻(头像+右下角状态圆点、昵称+能力徽标行、备注/完整 ID 行、需验证红色警示条+行内"重新授权"、右侧纯图标操作组)。搜索为前端本地过滤(纯函数),视图切换沿用既有 row/card 双形态与 localStorage 记忆,仅把文字按钮改为无文字的双图标分段控件。**零后端改动**:既有 `/api/v1/accounts*` 端点已覆盖全部所需行为(列表、资料刷新、验证会话、编辑、启停、删除、扫码接入)。

## Technical Context

**Language/Version**: TypeScript 7.0.2(前端,`type: module`);本特性无 Rust 改动

**Primary Dependencies**: vite 8.3(构建/开发代理)+ vitest 3.2(jsdom);无 UI 框架,原生 DOM(既有惯例);**不新增任何运行时依赖**——图标为内联 SVG(见 research R2)

**Storage**: 无新存储;视图偏好沿用既有 localStorage 键 `qing-account-view`("row" | "card",兼容旧值)

**Testing**: `npm run test`(vitest,过滤/排序/标签派生/分段控件行为测试)+ `npm run typecheck` + `npm run build`;无后端改动故不新增 cargo 契约测试,视觉验收走 quickstart 对照清单(SC-001)

**Target Platform**: Windows 本地单进程桌面服务内嵌管理页(浏览器访问)

**Performance Goals**: 本地过滤在几十账号规模下无可感知延迟(SC-002,≤1s,预期 <16ms 单帧)

**Constraints**: FR-008 不虚标能力(徽标只映射真实能力);FR-012 既有行为不回退(扫码/刷新/编辑/启停确认/删除确认/验证入口);宪章 III 轻量——零新增依赖、无新部署面;宪章 II——页面层只做交互调用,过滤/排序/标签派生是展示层纯函数,不含业务规则

**Scale/Scope**: 单页面改版:1 个 feature 模块重构(page.ts)+ 1 个新展示模型(model.ts)+ 2 个新 UI 组件(icons.ts、segmented.ts)+ 样式扩展;无路由、无后端、无迁移改动

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 结论 | 依据 |
|------|------|------|
| I. 以已付款订单和可恢复交付为核心 | ✅ 通过 | 纯管理界面呈现改版,不触碰交付、去重、恢复、凭证语义;启停/删除仍走既有确认流与端点 |
| II. 平台接入与发货业务分离 | ✅ 通过 | 页面只做交互调用(HTTP/桌面页面不得含交易规则);新增的过滤/排序/标签派生均为展示层纯函数,输入输出是既有 DTO 字段,无业务规则、无 SQL、无平台协议知识 |
| III. 轻量部署,以测量决定优化 | ✅ 通过 | 零新增运行时依赖(内联 SVG 替代图标库,无字体/图片资源);不引入新服务、构建工具或桌面外壳 |
| IV. 关键交易行为必须可验证、可追溯 | ✅ 通过 | 前端门禁齐全:类型检查 + vitest 行为测试(过滤/排序/标签/控件)+ 构建;FR-008"不虚标"由标签派生纯函数的单测保证;无交易行为变更 |
| V. 本地数据边界与参考源码隔离 | ✅ 通过 | 不新增敏感数据展示(完整 ID 展示为 004 已定契约);视图偏好仅本地 localStorage;参考项目仅作视觉对照,不复制其代码/资源,图标为自绘内联 SVG |

**Post-Phase 1 复核**: 设计产物为展示层视图模型与组件契约——无新持久化实体、无 HTTP 契约变更、无新增依赖,五项原则维持通过,无需 Complexity Tracking 豁免。

## Project Structure

### Documentation (this feature)

```text
specs/006-accounts-page-revamp/
├── plan.md              # 本文件
├── research.md          # Phase 0:技术决策与依据
├── data-model.md        # Phase 1:展示视图模型(无新持久化实体)
├── quickstart.md        # Phase 1:端到端验证指南(含 SC-001 对照核对表)
├── contracts/
│   └── ui-contract.md   # Phase 1:页面/组件契约与既有端点消费清单
└── tasks.md             # Phase 2 输出($speckit-tasks 生成)
```

### Source Code (repository root)

```text
frontend/src/
├── features/accounts/
│   ├── page.ts              # [改版] 装配页头/工具栏/计数横幅;搜索状态接入;
│   │                        #   行/卡片两种条目复用共享装配(徽标行/警示条/操作组)
│   ├── model.ts             # [新增] 展示层纯函数:filterAccounts/sortAccounts/
│   │                        #   deriveCapabilityTags/viewCounts(可 vitest 单测)
│   └── model.test.ts        # [新增] 上述纯函数行为测试(含 FR-008 不虚标断言)
├── ui/
│   ├── icons.ts             # [新增] 内联 SVG 图标表(search/qr/refresh/shield/edit/
│   │                        #   power/trash/list/grid),无依赖、可单测
│   ├── segmented.ts         # [新增] 双图标分段切换控件(aria-pressed、激活高亮、
│   │                        #   change 回调、title 提示)
│   └── segmented.test.ts    # [新增] 控件渲染与切换行为测试
└── styles/
    ├── components.css       # [扩展] 页头/工具栏/搜索框/计数横幅/分段控件样式;
    │                        #   条目视觉对齐参考(徽标软色底、警示条、图标操作组)
    └── tokens.css           # [扩展] 徽标软色底等少量语义变量(如既有变量可覆盖则不改)

src/webui/                   # vite build 产物输出目录(既有,emptyOutDir),不手改
```

**Structure Decision**: 沿用既有前端分层(features/ 页面装配 + ui/ 通用组件 + shared/ 基础设施 + styles/ 设计令牌),不引入新顶层目录、不引入框架;展示逻辑收敛到 `model.ts` 纯函数以便测试,`page.ts` 只做装配与既有 API 调用(宪章 II)。后端与迁移目录零改动。

## Complexity Tracking

> 无违例,无需豁免。
