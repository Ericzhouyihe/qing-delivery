import { describe, expect, it } from "vitest";
import {
  accountBadge,
  badgeEl,
  confirmationBadge,
  deliveryBadge,
  itemStatusBadge,
  issueStateBadge,
  qrBadge,
  reviewBadge,
  ruleStateBadge,
  statusPairEl,
  type BadgeSpec
} from "./badge";

/** 001 线上契约的全部枚举值——映射表必须 100% 覆盖(SC-205),未来新增枚举先补表。 */
const ALL_ENUMS: Array<[string, (v: string) => BadgeSpec, string[]]> = [
  ["账号", accountBadge, ["connecting", "online", "offline", "authorization_expired", "verification_required", "paused"]],
  ["内容交付", deliveryBadge, ["awaiting_verification", "ready", "sending", "delivered", "definitely_not_sent", "unknown", "needs_review", "terminated"]],
  ["审核", reviewBadge, ["none", "required", "resolved"]],
  ["平台确认", confirmationBadge, ["disabled", "pending", "dispatching", "succeeded", "failed", "unknown", "terminated"]],
  ["扫码", qrBadge, ["awaiting_scan", "awaiting_authorization", "authorized", "verification_required", "expired", "cancelled", "failed"]],
  ["商品", itemStatusBadge, ["on_sale", "off_sale", "unknown"]],
  ["规则", ruleStateBadge, ["configured", "missing", "disabled", "conflict"]],
  ["事项", issueStateBadge, ["open", "resolved", "terminated"]]
];

describe("StatusTone 映射(FR-004/SC-205)", () => {
  for (const [name, fn, values] of ALL_ENUMS) {
    it(`${name}轴全部枚举均有映射且 tone 合法`, () => {
      for (const v of values) {
        const b = fn(v);
        expect(b.label.length, `${name}:${v}`).toBeGreaterThan(0);
        expect(["normal", "warning", "danger", "neutral", "info"], `${name}:${v}`).toContain(b.tone);
      }
    });
  }

  it("未收录值 → 中性'未知状态',徽章悬浮保留原始值,不崩溃(FR-021)", () => {
    expect(accountBadge("brand_new_state")).toEqual({ tone: "neutral", label: "未知状态" });
    const el = badgeEl(accountBadge("brand_new_state"), "brand_new_state");
    expect(el.className).toBe("badge badge-neutral");
    expect(el.textContent).toBe("未知状态");
    expect(el.title).toBe("原始状态:brand_new_state");
  });

  it("语义色类别差异:危险/警告/正常不可同色(SC-201 可辨性)", () => {
    expect(deliveryBadge("unknown").tone).toBe("danger");
    expect(deliveryBadge("needs_review").tone).toBe("warning");
    expect(deliveryBadge("delivered").tone).toBe("normal");
    expect(confirmationBadge("failed").tone).toBe("danger");
    expect(ruleStateBadge("conflict").tone).toBe("danger");
  });

  it("双轴标签:主=内容,副=确认/审核;none/disabled 副轴不显示", () => {
    const pair = statusPairEl("delivered", "succeeded");
    expect(pair.querySelectorAll(".badge").length).toBe(2);
    expect(pair.textContent).toContain("内容已交付");
    expect(pair.textContent).toContain("平台已确认");
    const single = statusPairEl("delivered", "disabled");
    expect(single.querySelectorAll(".badge").length).toBe(1);
    const review = statusPairEl("sending", "required", "review");
    expect(review.textContent).toContain("待人工审核");
  });
});
