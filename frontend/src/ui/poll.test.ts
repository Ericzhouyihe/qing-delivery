import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { startPoll, type PollController } from "./poll";

describe("startPoll 可见性门控轮询", () => {
  let controllers: PollController[] = [];
  beforeEach(() => {
    vi.useFakeTimers();
    controllers = [];
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
  });
  afterEach(() => {
    for (const c of controllers) c.stop();
    controllers = [];
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("启动即执行一次,之后按间隔执行", async () => {
    const task = vi.fn().mockResolvedValue(undefined);
    controllers.push(startPoll(task, 30_000));
    expect(task).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(task).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(task).toHaveBeenCalledTimes(3);
  });

  it("页面隐藏即暂停,恢复可见立即刷新并继续", async () => {
    const task = vi.fn().mockResolvedValue(undefined);
    controllers.push(startPoll(task, 30_000));
    expect(task).toHaveBeenCalledTimes(1);

    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(120_000);
    expect(task).toHaveBeenCalledTimes(1); // 隐藏期间不轮询

    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
    expect(task).toHaveBeenCalledTimes(2); // 恢复立即刷新
    await vi.advanceTimersByTimeAsync(30_000);
    expect(task).toHaveBeenCalledTimes(3); // 并继续按间隔
  });

  it("stop 后不再执行;refresh 忽略并发中的重复触发", async () => {
    let release: (() => void) | undefined;
    const task = vi
      .fn()
      .mockImplementation(() => new Promise<void>((resolve) => (release = resolve)));
    const ctrl = startPoll(task, 30_000);
    controllers.push(ctrl);
    ctrl.refresh(); // 上一次仍在执行中 → 忽略
    expect(task).toHaveBeenCalledTimes(1);
    release?.();
    await vi.advanceTimersByTimeAsync(0);

    ctrl.stop();
    await vi.advanceTimersByTimeAsync(120_000);
    const calls = task.mock.calls.length;
    ctrl.refresh();
    expect(task.mock.calls.length).toBe(calls); // 停止后 refresh 无效
  });

  it("任务抛错不中断后续轮询", async () => {
    const task = vi.fn().mockRejectedValue(new Error("boom"));
    controllers.push(startPoll(task, 30_000));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(task).toHaveBeenCalledTimes(2);
  });
});
