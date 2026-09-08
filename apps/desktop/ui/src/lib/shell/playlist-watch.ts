export const PLAYLIST_RESCAN_INTERVAL_MS = 15_000;

interface PlaylistWatcherOptions {
  scan: () => Promise<void>;
  intervalMs?: number;
  isVisible?: () => boolean;
  subscribeVisibility?: (listener: () => void) => () => void;
}

export interface PlaylistWatcher {
  setActive(active: boolean): void;
  scanNow(): Promise<void>;
  stop(): void;
}

function documentIsVisible(): boolean {
  return typeof document === 'undefined' || document.visibilityState === 'visible';
}

function subscribeToDocumentVisibility(listener: () => void): () => void {
  if (typeof document === 'undefined') return () => undefined;
  document.addEventListener('visibilitychange', listener);
  return () => document.removeEventListener('visibilitychange', listener);
}

export function createPlaylistWatcher(options: PlaylistWatcherOptions): PlaylistWatcher {
  const intervalMs = options.intervalMs ?? PLAYLIST_RESCAN_INTERVAL_MS;
  const isVisible = options.isVisible ?? documentIsVisible;
  const subscribeVisibility = options.subscribeVisibility ?? subscribeToDocumentVisibility;
  let active = false;
  let stopped = false;
  let inFlight: Promise<void> | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function clearTimer(): void {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  }

  function schedule(): void {
    clearTimer();
    if (stopped || !active || !isVisible() || inFlight) return;
    timer = setTimeout(() => {
      timer = null;
      void scanNow();
    }, intervalMs);
  }

  async function scanNow(): Promise<void> {
    if (stopped || !active || !isVisible()) return;
    if (inFlight) return inFlight;
    clearTimer();
    inFlight = Promise.resolve().then(options.scan);
    try {
      await inFlight;
    } finally {
      inFlight = null;
      schedule();
    }
  }

  const unsubscribeVisibility = subscribeVisibility(() => {
    if (isVisible()) schedule();
    else clearTimer();
  });

  return {
    setActive(nextActive: boolean) {
      active = nextActive;
      schedule();
    },
    scanNow,
    stop() {
      stopped = true;
      active = false;
      clearTimer();
      unsubscribeVisibility();
    },
  };
}
