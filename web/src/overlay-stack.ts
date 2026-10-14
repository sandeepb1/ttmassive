// The system Back button (Android, a desktop browser) closes the top overlay before it leaves the app.
//
// Every overlay (a sheet, a menu, a sidebar that slid over the page, the image viewer, the command bar) registers
// here while it is open. While at least one is open there is exactly ONE extra history entry on top of the page's own:
// Back consumes it and we close the top overlay, then put the entry back if more overlays remain. One entry, not one
// per overlay, because history.back() is asynchronous and a menu that closes as the sheet it launches opens would
// otherwise pop the wrong entry. Whether the entry should exist is settled in `reconcile`, after the dust has settled.

export interface HistoryLike {
  pushState(state: unknown, title: string): void;
  back(): void;
}

export interface OverlayStack {
  /** Note that an overlay is open. `close` closes it. Returns the function to call when it is closed by anything else. */
  push(close: () => void): () => void;
  /** The browser went back (wire to `popstate`). */
  popped(): void;
  /** Overlays currently registered. */
  readonly depth: number;
}

export function createOverlayStack(history: HistoryLike): OverlayStack {
  const stack: { close: () => void }[] = [];
  let pushed = false; // our extra history entry is there
  let backPending = false; // we asked for it to go and the browser has not said so yet

  function reconcile() {
    if (backPending) return; // popped() calls this again
    const want = stack.length > 0;
    if (want && !pushed) {
      history.pushState({ ttmOverlay: true }, '');
      pushed = true;
    } else if (!want && pushed) {
      backPending = true;
      pushed = false;
      history.back();
    }
  }

  return {
    push(close) {
      const entry = { close };
      stack.push(entry);
      reconcile();
      return () => {
        const at = stack.indexOf(entry);
        if (at < 0) return;
        stack.splice(at, 1);
        // Settle after the current task: an overlay that closes as another opens keeps the one entry.
        queueMicrotask(reconcile);
      };
    },
    popped() {
      if (backPending) {
        backPending = false;
        reconcile();
        return;
      }
      if (!pushed) return; // not ours: some other navigation
      pushed = false; // Back took our entry
      const top = stack[stack.length - 1];
      if (top) {
        top.close();
        // An overlay that did not release itself on close is dropped anyway, so Back cannot get stuck on it.
        const at = stack.indexOf(top);
        if (at >= 0) stack.splice(at, 1);
      }
      reconcile();
    },
    get depth() {
      return stack.length;
    },
  };
}
