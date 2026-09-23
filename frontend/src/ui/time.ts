/** 相对时间与计时(T006,研究 D5):Intl.RelativeTimeFormat 格式化;
 *  tick 仅在页面可见时运行,等待时长随时间自然更新(边界条款)。 */

const rtf = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" });

/** 解析 RFC3339 为毫秒时间戳;非法输入返回 null。 */
export function parseIso(iso: string): number | null {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? null : t;
}

/** 相对时间:<60s 刚刚/N 秒前;分钟/小时/天逐级;>30 天退化为绝对日期。 */
export function formatRelative(iso: string, now = Date.now()): string {
  const t = parseIso(iso);
  if (t === null) return iso;
  const diffSec = Math.round((t - now) / 1000);
  const abs = Math.abs(diffSec);
  if (abs < 10) return "刚刚";
  if (abs < 60) return rtf.format(diffSec, "second");
  if (abs < 3600) return rtf.format(Math.round(diffSec / 60), "minute");
  if (abs < 86_400) return rtf.format(Math.round(diffSec / 3600), "hour");
  if (abs < 30 * 86_400) return rtf.format(Math.round(diffSec / 86_400), "day");
  return new Date(t).toLocaleString("zh-CN", { hour12: false });
}

/** 悬浮/次级展示用的绝对时间。 */
export function formatAbsolute(iso: string): string {
  const t = parseIso(iso);
  if (t === null) return iso;
  return new Date(t).toLocaleString("zh-CN", { hour12: false });
}

/** 等待时长(秒):负值钳为 0。 */
export function elapsedSeconds(iso: string, now = Date.now()): number {
  const t = parseIso(iso);
  if (t === null) return 0;
  return Math.max(0, Math.round((now - t) / 1000));
}

/** 人性化等待时长:3 分 12 秒 / 2 小时 5 分 / 3 天 4 小时。 */
export function formatDuration(totalSec: number): string {
  const s = Math.max(0, Math.floor(totalSec));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 === 0 ? `${m} 分` : `${m} 分 ${s % 60} 秒`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 === 0 ? `${h} 小时` : `${h} 小时 ${m % 60} 分`;
  const d = Math.floor(h / 24);
  return h % 24 === 0 ? `${d} 天` : `${d} 天 ${h % 24} 小时`;
}

/** 1 秒 tick:回调在页面可见时按间隔回调,隐藏即暂停。 */
export function startTicker(cb: () => void, intervalMs = 1000): () => void {
  let timer: number | undefined;
  const start = (): void => {
    if (timer === undefined) timer = window.setInterval(cb, intervalMs);
  };
  const stop = (): void => {
    if (timer !== undefined) {
      window.clearInterval(timer);
      timer = undefined;
    }
  };
  const onVisibility = (): void => {
    if (document.hidden) stop();
    else start();
  };
  document.addEventListener("visibilitychange", onVisibility);
  if (!document.hidden) start();
  return () => {
    stop();
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
