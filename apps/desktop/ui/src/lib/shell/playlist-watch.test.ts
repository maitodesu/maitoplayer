import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPlaylistWatcher } from './playlist-watch';

describe('playlist folder watcher', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('rescans only while active and visible', async () => {
    let visible = true;
    let notifyVisibility: () => void = () => undefined;
    const scan = vi.fn(async () => undefined);
    const watcher = createPlaylistWatcher({
      scan,
      intervalMs: 1_000,
      isVisible: () => visible,
      subscribeVisibility: (listener) => {
        notifyVisibility = listener;
        return () => undefined;
      },
    });

    watcher.setActive(true);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(scan).toHaveBeenCalledTimes(1);

    visible = false;
    notifyVisibility();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(scan).toHaveBeenCalledTimes(1);

    visible = true;
    notifyVisibility();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(scan).toHaveBeenCalledTimes(2);

    watcher.setActive(false);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(scan).toHaveBeenCalledTimes(2);
    watcher.stop();
  });

  it('coalesces manual and periodic scans into one in-flight request', async () => {
    let finishScan: () => void = () => undefined;
    const scan = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishScan = resolve;
        }),
    );
    const watcher = createPlaylistWatcher({ scan, intervalMs: 1_000, isVisible: () => true });
    watcher.setActive(true);

    const first = watcher.scanNow();
    const second = watcher.scanNow();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(scan).toHaveBeenCalledTimes(1);

    finishScan();
    await Promise.all([first, second]);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(scan).toHaveBeenCalledTimes(2);
    watcher.stop();
  });

  it('cleans up its scheduled rescan', async () => {
    const scan = vi.fn(async () => undefined);
    const watcher = createPlaylistWatcher({ scan, intervalMs: 1_000, isVisible: () => true });
    watcher.setActive(true);
    watcher.stop();

    await vi.advanceTimersByTimeAsync(2_000);
    expect(scan).not.toHaveBeenCalled();
  });
});
