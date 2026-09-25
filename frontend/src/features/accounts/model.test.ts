import { afterEach, describe, expect, it } from "vitest";
import {
  deriveCapabilityTags,
  filterAccounts,
  parseAccountCard,
  readViewPreference,
  sortAccounts,
  viewCounts,
  writeViewPreference,
  type AccountCard
} from "./model";

const acct = (over: Partial<AccountCard>): AccountCard => ({
  id: "a",
  platformUserId: "123456",
  avatarUrl: null,
  remark: null,
  displayName: "小店",
  connectionState: "online",
  runEnabled: true,
  autoDelivery: false,
  autoConfirm: false,
  controlVersion: 1,
  monitoringSince: null,
  ...over
});

describe("搜索过滤 filterAccounts(US1/FR-003,数据模型 §2)", () => {
  const items = [
    acct({ id: "1", displayName: "一人一口粥", remark: "主号", platformUserId: "3996151559" }),
    acct({ id: "2", displayName: "二手书店", remark: "测试号", platformUserId: "8877" }),
    acct({ id: "3", displayName: "Shop-A", remark: null, platformUserId: "666" })
  ];

  it("昵称子串命中", () => {
    expect(filterAccounts(items, "一口").map((a) => a.id)).toEqual(["1"]);
  });

  it("备注子串命中", () => {
    expect(filterAccounts(items, "测试").map((a) => a.id)).toEqual(["2"]);
  });

  it("账号 ID 子串命中", () => {
    expect(filterAccounts(items, "3996").map((a) => a.id)).toEqual(["1"]);
  });

  it("不区分大小写", () => {
    expect(filterAccounts(items, "shop").map((a) => a.id)).toEqual(["3"]);
    expect(filterAccounts(items, "SHOP-A").map((a) => a.id)).toEqual(["3"]);
  });

  it("query 首尾空格被 trim", () => {
    expect(filterAccounts(items, "  测试  ").map((a) => a.id)).toEqual(["2"]);
  });

  it("空 query 返回全部且保持原顺序", () => {
    expect(filterAccounts(items, "").map((a) => a.id)).toEqual(["1", "2", "3"]);
    expect(filterAccounts(items, "   ").length).toBe(3);
  });

  it("无命中返回空数组", () => {
    expect(filterAccounts(items, "不存在")).toEqual([]);
  });

  it("remark 为 null 不参与匹配也不抛错", () => {
    expect(filterAccounts([acct({ remark: null })], "null")).toEqual([]);
  });
});

describe("计数 viewCounts(US1/FR-004)", () => {
  it("返回 shown/total 原值", () => {
    expect(viewCounts(1, 2)).toEqual({ shown: 1, total: 2 });
    expect(viewCounts(0, 5)).toEqual({ shown: 0, total: 5 });
  });
});

describe("账号解析(迁移自 page.ts,保持既有行为)", () => {
  it("解析 control 嵌套与缺省", () => {
    const a = parseAccountCard({
      id: "acct-1",
      platform_user_id: "1234567890abcdef",
      display_name: "小店",
      connection_state: "online",
      control: { run_enabled: true, auto_delivery_enabled: false, auto_confirm_enabled: false, transition: "stable" },
      control_version: 3,
      monitoring_since: "2026-09-22T02:00:00Z"
    });
    expect(a.runEnabled).toBe(true);
    expect(a.autoDelivery).toBe(false);
    expect(a.controlVersion).toBe(3);
    expect(a.monitoringSince).not.toBeNull();

    const b = parseAccountCard({ id: "x" });
    expect(b.connectionState).toBe("paused");
    expect(b.controlVersion).toBe(1);
  });
});

describe("异常置顶排序 sortAccounts(US2/FR-006,数据模型 §2)", () => {
  it("需验证/授权失效 > 连接中 > 在线 > 其余", () => {
    const items = [
      acct({ id: "paused", displayName: "甲", connectionState: "paused" }),
      acct({ id: "online", displayName: "乙", connectionState: "online" }),
      acct({ id: "connecting", displayName: "丙", connectionState: "connecting" }),
      acct({ id: "verify", displayName: "丁", connectionState: "verification_required" }),
      acct({ id: "expired", displayName: "戊", connectionState: "authorization_expired" })
    ];
    expect(sortAccounts(items).map((a) => a.id)).toEqual([
      "verify",
      "expired",
      "connecting",
      "online",
      "paused"
    ]);
  });

  it("同级按昵称 localeCompare 排序,且不改动输入数组", () => {
    const items = [
      acct({ id: "b", displayName: "乙店", connectionState: "online" }),
      acct({ id: "a", displayName: "甲店", connectionState: "online" })
    ];
    const sorted = sortAccounts(items);
    expect(sorted.map((a) => a.id)).toEqual(["a", "b"]);
    expect(items.map((a) => a.id)).toEqual(["b", "a"]);
  });
});

describe("能力徽标派生 deriveCapabilityTags(US2/FR-008,数据模型 §2)", () => {
  it("需验证/授权失效 → 需要验证(warning)", () => {
    for (const state of ["verification_required", "authorization_expired"]) {
      expect(deriveCapabilityTags(acct({ connectionState: state }))).toEqual([
        { id: "needs-verify", label: "需要验证", tone: "warning" }
      ]);
    }
  });

  it("开关开启 → 对应能力徽标(normal);关闭不出现", () => {
    expect(deriveCapabilityTags(acct({ autoDelivery: true }))).toEqual([
      { id: "auto-delivery", label: "自动发货", tone: "normal" }
    ]);
    expect(deriveCapabilityTags(acct({ autoConfirm: true }))).toEqual([
      { id: "auto-confirm", label: "自动平台确认", tone: "normal" }
    ]);
    expect(deriveCapabilityTags(acct({ autoDelivery: true, autoConfirm: true })).map((t) => t.id)).toEqual([
      "auto-delivery",
      "auto-confirm"
    ]);
  });

  it("在线且全关 → 无能力徽标(连接状态由徽章/圆点表达)", () => {
    expect(deriveCapabilityTags(acct({ connectionState: "online" }))).toEqual([]);
  });

  it("不虚标不变量:任意输入输出 ⊆ 真实能力集合,绝不出现 AI/自动评价/每日擦亮", () => {
    const allowed = new Set(["needs-verify", "auto-delivery", "auto-confirm"]);
    const forbidden = ["AI", "自动评价", "每日擦亮", "ai", "auto-review"];
    const states = ["online", "offline", "connecting", "verification_required", "authorization_expired", "paused", "unknown-x"];
    for (const connectionState of states) {
      for (const autoDelivery of [true, false]) {
        for (const autoConfirm of [true, false]) {
          const tags = deriveCapabilityTags(acct({ connectionState, autoDelivery, autoConfirm }));
          for (const t of tags) {
            expect(allowed.has(t.id)).toBe(true);
            for (const f of forbidden) expect(t.label).not.toContain(f);
          }
        }
      }
    }
  });
});

describe("视图偏好读写(US3/FR-010,契约 §3)", () => {
  afterEach(() => {
    localStorage.removeItem("qing-account-view");
  });

  it("键缺失回退 row", () => {
    localStorage.removeItem("qing-account-view");
    expect(readViewPreference()).toBe("row");
  });

  it("合法值 card 记忆", () => {
    localStorage.setItem("qing-account-view", "card");
    expect(readViewPreference()).toBe("card");
  });

  it("未知旧值回退 row", () => {
    localStorage.setItem("qing-account-view", "grid-legacy");
    expect(readViewPreference()).toBe("row");
  });

  it("writeViewPreference 写入后可读回", () => {
    writeViewPreference("card");
    expect(readViewPreference()).toBe("card");
    writeViewPreference("row");
    expect(readViewPreference()).toBe("row");
  });
});
