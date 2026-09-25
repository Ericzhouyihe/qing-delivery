import { describe, expect, it } from "vitest";
import { barTitle, bucketLabel, renderTrendChart, type TrendBar } from "./chart";

const HOUR = 3_600_000;
const DAY = 86_400_000;

function bars(n: number, granularity: "hourly" | "daily", seedValue = 1000): TrendBar[] {
  return Array.from({ length: n }, (_, i) => ({
    bucketStart: (granularity === "hourly" ? i * HOUR : i * DAY) + 1000,
    minorUnits: i % 3 === 0 ? 0 : seedValue + i,
    orderCount: i % 3 === 0 ? 0 : i + 1,
    granularity
  }));
}

describe("renderTrendChart 桶→SVG 映射(005/T013)", () => {
  it("柱节点数 = 桶数;零值柱使用 empty 类(零高度可见语义)", () => {
    const list = bars(7, "daily");
    const node = renderTrendChart(list);
    const svg = node.querySelector("svg");
    expect(svg).not.toBeNull();
    const rects = svg!.querySelectorAll("rect.trend-bar, rect.trend-bar-empty");
    expect(rects.length).toBe(7);
    // 第 0 桶为零值 → empty 类
    expect(rects[0]!.getAttribute("class")).toBe("trend-bar-empty");
    expect(Number(rects[0]!.getAttribute("height"))).toBe(0);
    // 非零柱有正高度且 ≤ 绘图区
    const h = Number(rects[1]!.getAttribute("height"));
    expect(h).toBeGreaterThan(0);
  });

  it("每桶都有悬浮 title,含金额与笔数", () => {
    const node = renderTrendChart(bars(3, "hourly"));
    const titles = node.querySelectorAll("svg > g > title");
    expect(titles.length).toBe(3);
    expect(titles[1]!.textContent).toContain("¥");
    expect(titles[1]!.textContent).toContain("单");
  });

  it("空数组不抛错并给出占位", () => {
    const node = renderTrendChart([]);
    expect(node.querySelector("svg")).toBeNull();
    expect(node.textContent).toContain("暂无数据");
  });

  it("柱多时轴标签抽样(≤8 个文本标签)", () => {
    const node = renderTrendChart(bars(30, "hourly"));
    const labels = node.querySelectorAll("text.trend-label");
    expect(labels.length).toBeGreaterThan(0);
    expect(labels.length).toBeLessThanOrEqual(8);
  });

  it("峰值参考线只在有非零柱时出现", () => {
    expect(renderTrendChart(bars(5, "daily")).querySelector("line.trend-peak")).not.toBeNull();
    const zeros = bars(3, "daily").map((b) => ({ ...b, minorUnits: 0 }));
    expect(renderTrendChart(zeros).querySelector("line.trend-peak")).toBeNull();
  });
});

describe("桶标签与提示文案(005/T013)", () => {
  it("日粒度标签为 M/D;小时粒度为 H:00", () => {
    const d = new Date(2026, 8, 25, 14, 0).getTime();
    expect(bucketLabel(d, "daily")).toBe("9/25");
    expect(bucketLabel(d, "hourly")).toBe("14:00");
  });

  it("barTitle 组装时间/金额/笔数", () => {
    const t = barTitle({ bucketStart: new Date(2026, 8, 25, 9, 0).getTime(), minorUnits: 1234, orderCount: 2, granularity: "hourly" });
    expect(t).toBe("9月25日 9:00 · ¥12.34 · 2 单");
  });
});
