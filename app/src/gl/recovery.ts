/**
 * Recovery from a lost graphics context.
 *
 * A brief loss (the GPU process restarting) is restored by the browser and the renderer rebuilds
 * itself. After a display driver or compositor crash the context can stay lost: the island is
 * then a blank rectangle until Sudo restarts. This watches for that and asks for the restart.
 */

/** How long the browser gets to restore the context before the island gives up on it. */
export const RESTORE_GRACE_MS = 4000;
/** At most this many relaunches within `RECOVERY_WINDOW_MS`, so a machine that cannot draw does not loop. */
export const RECOVERY_LIMIT = 3;
export const RECOVERY_WINDOW_MS = 10 * 60_000;
const KEY = "sudo.graphics.recoveries";

type Store = Pick<Storage, "getItem" | "setItem">;

/** Records a relaunch at `now` and reports whether it is within the limit. */
export function allowRecovery(now: number, store: Store): boolean {
  let recent: number[] = [];
  try {
    const saved: unknown = JSON.parse(store.getItem(KEY) ?? "[]");
    if (Array.isArray(saved)) {
      recent = saved.filter((at): at is number => typeof at === "number" && at <= now && now - at < RECOVERY_WINDOW_MS);
    }
  } catch {
    // Unreadable history counts as none.
  }
  if (recent.length >= RECOVERY_LIMIT) return false;
  try {
    store.setItem(KEY, JSON.stringify([...recent, now]));
  } catch {
    // Without a record the limit cannot hold, so do not relaunch.
    return false;
  }
  return true;
}

/**
 * Calls `recover` when the canvas loses its context and does not get it back within `graceMs`.
 * Returns a function that stops watching.
 */
export function watchGraphics(canvas: EventTarget, recover: () => void, graceMs = RESTORE_GRACE_MS): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const lost = () => {
    clearTimeout(timer);
    timer = setTimeout(recover, graceMs);
  };
  const restored = () => {
    clearTimeout(timer);
    timer = undefined;
  };
  canvas.addEventListener("webglcontextlost", lost);
  canvas.addEventListener("webglcontextrestored", restored);
  return () => {
    restored();
    canvas.removeEventListener("webglcontextlost", lost);
    canvas.removeEventListener("webglcontextrestored", restored);
  };
}
