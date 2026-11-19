import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { allowRecovery, RECOVERY_LIMIT, RECOVERY_WINDOW_MS, RESTORE_GRACE_MS, watchGraphics } from "./recovery";

function memory(initial?: string) {
  let value = initial ?? null;
  return {
    getItem: () => value,
    setItem: (_key: string, next: string) => { value = next; },
  };
}

describe("allowRecovery", () => {
  it("allows a few relaunches, then refuses until the window has passed", () => {
    const store = memory();
    const start = 1_000_000;
    for (let i = 0; i < RECOVERY_LIMIT; i++) expect(allowRecovery(start + i * 1000, store)).toBe(true);
    expect(allowRecovery(start + 60_000, store)).toBe(false);
    expect(allowRecovery(start + RECOVERY_WINDOW_MS + 5000, store)).toBe(true);
  });

  it("treats unreadable history as none", () => {
    expect(allowRecovery(5000, memory("not json"))).toBe(true);
    expect(allowRecovery(5000, memory('{"a":1}'))).toBe(true);
  });

  it("refuses when the relaunch cannot be recorded", () => {
    const store = { getItem: () => null, setItem: () => { throw new Error("full"); } };
    expect(allowRecovery(5000, store)).toBe(false);
  });
});

describe("watchGraphics", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("recovers only when the context stays lost", () => {
    const canvas = new EventTarget();
    const recover = vi.fn();
    watchGraphics(canvas, recover);

    canvas.dispatchEvent(new Event("webglcontextlost"));
    vi.advanceTimersByTime(RESTORE_GRACE_MS - 1);
    canvas.dispatchEvent(new Event("webglcontextrestored"));
    vi.advanceTimersByTime(RESTORE_GRACE_MS * 2);
    expect(recover).not.toHaveBeenCalled();

    canvas.dispatchEvent(new Event("webglcontextlost"));
    vi.advanceTimersByTime(RESTORE_GRACE_MS);
    expect(recover).toHaveBeenCalledTimes(1);
  });

  it("stops watching when disposed", () => {
    const canvas = new EventTarget();
    const recover = vi.fn();
    const stop = watchGraphics(canvas, recover);
    canvas.dispatchEvent(new Event("webglcontextlost"));
    stop();
    canvas.dispatchEvent(new Event("webglcontextlost"));
    vi.advanceTimersByTime(RESTORE_GRACE_MS * 2);
    expect(recover).not.toHaveBeenCalled();
  });
});
