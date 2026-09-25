# UI Contract: 账号管理页面复刻与视图切换(006-accounts-page-revamp)

**Date**: 2026-09-25 | **Spec**: [spec.md](spec.md)

本特性**不新增、不修改任何 HTTP 端点**;契约对象为页面结构与新增可复用组件。既有端点消费清单见 §4。

## 1. 页面结构契约(`features/accounts/page.ts`)

账号页内容区自上而下:

```text
┌─ .page-head ────────────────────────────────────────────────┐
│ h1 "账号管理"                                                │
│ p  副标题:管理您的闲鱼授权账号及设置。建议给账号填写备注,便于多账号区分。 │
├─ .account-toolbar ─────────────────────────────────────────┐
│ [搜索框 .search-input placeholder="搜索昵称 / 备注 / 账号ID"] │  ← 左侧
│                           [SegmentedViewToggle][扫码添加新账号] │  ← 右侧
├─ .count-banner(浅色横幅,常驻) ────────────────────────────┐
│ "当前显示 X / Y 个账号"        固定提示:如果某个账号只显示 ID,   │
│                              点该账号右侧"刷新资料";若刷新失败,  │
│                              先点二维码重新授权。                │
├─ 列表区(asyncBlock:骨架/错误重试/空态) ────────────────────┤
│ .account-rows(列表形态) 或 .account-grid(卡片形态)           │
└─────────────────────────────────────────────────────────────┘
```

行为契约:

| 编号 | 契约 | 规格依据 |
|------|------|----------|
| P1 | 搜索输入(无回车/按钮)即时过滤;`input` 事件驱动;清空恢复全部 | FR-003 |
| P2 | 横幅 X = 过滤后可见数,Y = 总数;随过滤同步更新 | FR-004 |
| P3 | 主按钮文案"扫码添加新账号"(带二维码图标),点击打开既有扫码接入弹窗,授权成功刷新列表 | FR-002/FR-012 |
| P4 | 搜索无匹配:列表区显示"无匹配账号"轻空态 + "清空搜索"按钮;与总数为 0 的引导空态互斥 | FR-011/R8 |
| P5 | 过滤与排序(异常置顶)在 row/card 两形态下行为一致 | FR-010 |
| P6 | 加载中/失败时页头与工具栏保持稳定(骨架仅出现在列表区) | FR-011 |

## 2. 组件契约

### 2.1 SegmentedViewToggle(`ui/segmented.ts`)

```ts
interface SegmentedOptions<V extends string> {
  options: Array<{ value: V; icon: string; label: string }>; // icon → icons.ts 键名
  value: V;                    // 当前激活项
  onSelect: (value: V) => void;
}
function createSegmented<V extends string>(opts: SegmentedOptions<V>): HTMLElement;
```

- 容器 `role="group"`, `aria-label="视图切换"`;每个选项为 `<button type="button">`,内容**仅图标无文字**(FR-009),`title` = `label`,`aria-pressed` 表达激活态,激活项带高亮样式类。
- 点击未激活项 → 更新内部激活态并调用 `onSelect`;点击已激活项 → 不触发(幂等)。
- 控件不读写 localStorage、不发起请求(装配职责归 `page.ts`,便于复用与测试)。

### 2.2 图标表(`ui/icons.ts`)

```ts
function icon(name: IconName, size?: number): SVGSVGElement;
```

- 键名(首版):`search` `qr` `refresh` `shield` `edit` `power` `trash` `list` `grid`。
- 输出 `<svg viewBox="0 0 24 24" width=size(默认 18) height=size fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true" focusable="false">`;颜色继承 `currentColor`,悬停变色由按钮样式控制。
- 未知键名按编程错误抛出(编译期字面量联合约束)。
- 全部路径自绘,不复制第三方图标资产(宪章 V)。

### 2.3 条目操作组(共享装配,两形态一致)

| 图标 | title | 行为(全部为既有行为,FR-012) | 样式变体 |
|------|-------|-------------------------------|----------|
| refresh | 刷新资料 | `POST /accounts/{id}/profile-fetch`,进行中禁用+旋转,完成重载 | 悬停中性 |
| qr(仅需验证/授权失效) | 重新授权 | 打开扫码接入弹窗,授权成功重载 | 强调色底(浅红) |
| shield | 发起安全验证 | `POST /accounts/{id}/verification`(409 视为进行中) | 悬停中性 |
| edit | 编辑账号 | 既有编辑弹窗(备注/自动化开关) | 悬停中性 |
| power | 启用账号/停用账号 | 既有确认流(`applyControl`,含"暂停中"过渡) | 启用/停用双色 |
| trash | 删除账号 ×× | 既有删除确认弹窗 | 危险红悬停 |

## 3. 本地偏好契约

| 项 | 值 |
|----|----|
| 键 | `qing-account-view` |
| 值 | `"row"` \| `"card"`;缺失/未知 → `"row"` |
| 写入方 | `page.ts`(点击分段控件后) |

## 4. 消费的既有端点(零变更)

| 端点 | 用途 |
|------|------|
| `GET /api/v1/accounts` | 列表(含 control 嵌套) |
| `GET /api/v1/accounts/{id}/verification` | 验证会话徽章轮询 |
| `POST /api/v1/accounts/{id}/verification` | 发起验证 |
| `POST /api/v1/accounts/{id}/profile-fetch` | 刷新昵称/头像 |
| `PATCH /api/v1/accounts/{id}` | 保存备注 |
| `POST /api/v1/accounts/{id}/control` | 启停/自动化开关(expected_version) |
| `DELETE /api/v1/accounts/{id}` | 删除账号 |
| 扫码接入会话端点(既有 qr-modal 内部) | 扫码添加新账号/重新授权 |

前端请求统一经 `shared/http.ts`(鉴权/CSRF/错误路径),本特性不新增请求语义。
