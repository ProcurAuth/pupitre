// A repeated call: it waits for the end of a call before it starts the next one, it stops while the page is hidden,
// and it stops at once when `stop()` runs. Used for the runs, the events and the output (5.17).

export function poller(fn, ms) {
  let timer = 0;
  let running = false;
  let stopped = true;

  async function tick() {
    timer = 0;
    if (stopped) return;
    if (!document.hidden) {
      running = true;
      try {
        await fn();
      } catch (error) {
        // The caller shows its own errors.
      } finally {
        running = false;
      }
    }
    if (!stopped) timer = window.setTimeout(tick, ms);
  }

  return {
    start({ immediate = false } = {}) {
      if (!stopped) return;
      stopped = false;
      timer = window.setTimeout(tick, immediate ? 0 : ms);
    },
    stop() {
      stopped = true;
      if (timer) window.clearTimeout(timer);
      timer = 0;
    },
    // Runs the call now, once, without waiting for the timer.
    async now() {
      if (running) return;
      await fn();
    },
  };
}
