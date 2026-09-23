/** 可见性门控轮询(T005,研究 D5):页面可见时按固定间隔执行;
 *  隐藏即暂停、恢复可见立即刷新一次(FR-006/宪章 III 资源取向)。 */

export interface PollController {
  /** 立即触发一次(手动刷新按钮使用);并发中的调用会被忽略。 */
  refresh(): void;
  /** 停止轮询并移除可见性监听;页面卸载/路由切换必须调用。 */
  stop(): void;
}

export function startPoll(task: () => Promise<void>, intervalMs = 30_000): PollController {
  let timer: number | undefined;
  let running = false;
  let stopped = false;

  const run = (): void => {
    if (running || stopped) return;
    running = true;
    Promise.resolve(task())
      .catch(() => undefined)
      .finally(() => {
        running = false;
      });
  };

  const suspend = (): void => {
    if (timer !== undefined) {
      window.clearInterval(timer);
      timer = undefined;
    }
  };

  const resume = (): void => {
    suspend();
    if (stopped || document.hidden) return;
    run();
    timer = window.setInterval(run, intervalMs);
  };

  const onVisibility = (): void => {
    if (document.hidden) suspend();
    else resume();
  };

  document.addEventListener("visibilitychange", onVisibility);
  resume();

  return {
    refresh: run,
    stop(): void {
      stopped = true;
      suspend();
      document.removeEventListener("visibilitychange", onVisibility);
    }
  };
}
