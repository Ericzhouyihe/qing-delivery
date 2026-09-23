import { describe, expect, it } from "vitest";
import { elapsedSeconds, formatDuration, formatRelative, parseIso } from "./time";

describe("formatRelative 相对时间", () => {
  const now = Date.UTC(2026, 8, 22, 12, 0, 0);

  it("秒/分/小时/天逐级格式化", () => {
    expect(formatRelative(new Date(now - 5_000).toISOString(), now)).toBe("刚刚");
    expect(formatRelative(new Date(now - 30_000).toISOString(), now)).toContain("秒");
    expect(formatRelative(new Date(now - 5 * 60_000).toISOString(), now)).toContain("分钟");
    expect(formatRelative(new Date(now - 3 * 3_600_000).toISOString(), now)).toContain("小时");
    expect(formatRelative(new Date(now - 2 * 86_400_000).toISOString(), now)).toContain("天");
  });

  it("超过 30 天退化为绝对时间;非法输入原样返回", () => {
    const out = formatRelative(new Date(now - 40 * 86_400_000).toISOString(), now);
    expect(out).not.toContain("天前");
    expect(out).toContain("2026");
    expect(formatRelative("not-a-date", now)).toBe("not-a-date");
  });
});

describe("等待时长", () => {
  it("elapsedSeconds 计算并钳负", () => {
    const iso = new Date(Date.UTC(2026, 8, 22, 11, 59, 30)).toISOString();
    expect(elapsedSeconds(iso, Date.UTC(2026, 8, 22, 12, 0, 0))).toBe(30);
    expect(elapsedSeconds(iso, Date.UTC(2026, 8, 22, 11, 59, 0))).toBe(0);
  });

  it("formatDuration 人性化分级", () => {
    expect(formatDuration(45)).toBe("45 秒");
    expect(formatDuration(192)).toBe("3 分 12 秒");
    expect(formatDuration(120)).toBe("2 分");
    expect(formatDuration(7500)).toBe("2 小时 5 分");
    expect(formatDuration(276_600)).toBe("3 天 4 小时");
  });

  it("parseIso 非法返回 null", () => {
    expect(parseIso("bad")).toBeNull();
    expect(parseIso("2026-09-22T12:00:00Z")).toBe(Date.UTC(2026, 8, 22, 12, 0, 0));
  });
});
