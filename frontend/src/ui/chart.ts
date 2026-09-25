/** 趋势图(005,T013):手写 SVG 柱状图,零运行时依赖(宪章 III/研究 R5)。
 *  输入为服务端趋势点(空桶已补零);每柱带 <title> 悬浮提示(本地时间 · 金额 · 笔数);
 *  轴标签按柱数抽样,避免小时粒度(≤49 桶)拥挤。纯 DOM 结构,可被 jsdom 断言。 */

const SVG_NS = "http://www.w3.org/2000/svg";

export type TrendGranularity = "hourly" | "daily";

export interface TrendBar {
  bucketStart: number;
  minorUnits: number;
  orderCount: number;
  granularity: TrendGranularity;
}

/** 桶轴标签:日粒度 "M/D",小时粒度 "H:00"。 */
export function bucketLabel(bucketStart: number, granularity: TrendGranularity): string {
  const d = new Date(bucketStart);
  if (granularity === "hourly") {
    return `${d.getHours()}:00`;
  }
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

/** 悬浮提示文本:本地时间 · 金额 · 笔数。 */
export function barTitle(bar: TrendBar): string {
  const d = new Date(bar.bucketStart);
  const day = `${d.getMonth() + 1}月${d.getDate()}日`;
  const time = bar.granularity === "hourly" ? ` ${d.getHours()}:00` : "";
  return `${day}${time} · ¥${(bar.minorUnits / 100).toFixed(2)} · ${bar.orderCount} 单`;
}

const HEIGHT = 170;
const PAD_TOP = 10;
const PAD_BOTTOM = 26;
const SLOT = 48;

function svgEl(tag: string, attrs: Record<string, string | number> = {}): SVGElement {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

/** 渲染柱状趋势图;bars 为空时返回占位节点(空态语义由调用方处理)。 */
export function renderTrendChart(bars: TrendBar[]): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "trend-chart";
  if (bars.length === 0) {
    const empty = document.createElement("p");
    empty.className = "muted";
    empty.textContent = "暂无数据";
    wrap.append(empty);
    return wrap;
  }
  const max = Math.max(...bars.map((b) => b.minorUnits), 1);
  const width = bars.length * SLOT;
  const baseY = HEIGHT - PAD_BOTTOM;
  const plotH = baseY - PAD_TOP;

  const svg = svgEl("svg", {
    viewBox: `0 0 ${width} ${HEIGHT}`,
    preserveAspectRatio: "xMidYMid meet",
    role: "img",
    "aria-label": "营收趋势柱状图"
  });
  svg.append(
    svgEl("line", {
      x1: 0,
      y1: baseY,
      x2: width,
      y2: baseY,
      class: "trend-axis",
      "stroke-width": 1
    })
  );

  // 峰值参考线(仅当有非零柱)
  if (max > 1) {
    svg.append(
      svgEl("line", {
        x1: 0,
        y1: baseY - plotH,
        x2: width,
        y2: baseY - plotH,
        class: "trend-peak",
        "stroke-width": 1,
        "stroke-dasharray": "4 4"
      })
    );
  }

  // 标签抽样:柱多时隔行显示,保证不拥挤
  const labelStep = Math.max(1, Math.ceil(bars.length / 8));
  bars.forEach((bar, i) => {
    const x = i * SLOT;
    const g = svgEl("g");
    const title = svgEl("title");
    title.textContent = barTitle(bar);
    const value = bar.minorUnits;
    const h = Math.round((plotH * value) / max);
    const hover = svgEl("rect", {
      x,
      y: 0,
      width: SLOT,
      height: baseY,
      class: "trend-hover",
      fill: "transparent"
    });
    const rect = svgEl("rect", {
      x: x + SLOT * 0.2,
      y: baseY - h,
      width: SLOT * 0.6,
      height: h,
      rx: 3,
      class: value > 0 ? "trend-bar" : "trend-bar-empty"
    });
    g.append(title, hover, rect);
    if (i % labelStep === 0) {
      const label = svgEl("text", {
        x: x + SLOT / 2,
        y: HEIGHT - 8,
        "text-anchor": "middle",
        class: "trend-label"
      });
      label.textContent = bucketLabel(bar.bucketStart, bar.granularity);
      g.append(label);
    }
    svg.append(g);
  });

  wrap.append(svg);
  return wrap;
}
