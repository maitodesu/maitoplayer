<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import type {
    AppErrorV1,
    CapabilityCandidateV1,
    MediaSessionV1,
    PlaybackCheckpointV1,
    PlaybackStateV1,
  } from '../../contracts/generated';
  import { api, asAppError } from '../../shell/api';
  import { formatTime, microsecondsToSeconds, secondsToMicroseconds } from '../../shell/time';
  import {
    canAutoHidePlayerControls,
    mediaShortcutAction,
    PLAYER_CONTROLS_HIDE_DELAY_MS,
    shortcutTargetKind,
  } from './player-controls';

  interface Props {
    session: MediaSessionV1;
    active?: boolean;
    fullscreenTarget?: HTMLElement | null;
    onTime: (positionUs: number) => void;
    onSessionUpdate: (session: MediaSessionV1) => void;
    onError: (error: AppErrorV1) => void;
  }

  let {
    session,
    active = true,
    fullscreenTarget = null,
    onTime,
    onSessionUpdate,
    onError,
  }: Props = $props();
  let video: HTMLVideoElement;
  let playerRoot: HTMLElement;
  let controlsRoot: HTMLElement;
  let speedMenu: HTMLDetailsElement | undefined;
  let playing = $state(false);
  let positionUs = $state(0);
  let volume = $state(1);
  let playbackRate = $state(1);
  let selectedAudio = $state<string | null>(null);
  let revision = 0;
  let capabilityRevision = 0;
  let lastCoarseCheckpoint = 0;
  let frameRequest: number | null = null;
  let animationRequest: number | null = null;
  let conversionBusy = $state(false);
  let automaticAttempt = $state('');
  let reportedPlaybackUrl = '';
  let conversionProgress = $state<number | null>(null);
  let progressTimer: ReturnType<typeof setInterval> | null = null;
  let progressPollInFlight = false;
  let conversionGeneration = 0;
  let destroying = false;
  let controlsIdle = $state(false);
  let controlsHovered = $state(false);
  let controlsFocused = $state(false);
  let scrubbing = $state(false);
  let rateMenuOpen = $state(false);
  let fullscreenActive = $state(false);
  let playbackErrored = $state(false);
  let buffering = $state(false);
  let bufferedPercent = $state(0);
  let lastAudibleVolume = 1;
  let controlsHideTimer: ReturnType<typeof setTimeout> | null = null;
  let videoClickTimer: ReturnType<typeof setTimeout> | null = null;

  const durationSeconds = $derived(microsecondsToSeconds(session.duration_us));
  const controlsBlockingStatus = $derived(
    playbackErrored ||
      buffering ||
      conversionBusy ||
      ['awaiting_approval', 'failed', 'cancelled'].includes(session.playback_plan.state),
  );
  const controlsCanAutoHide = $derived(
    canAutoHidePlayerControls({
      active,
      playing,
      controlsHovered,
      controlsFocused,
      scrubbing,
      menuOpen: rateMenuOpen,
      blockingStatus: controlsBlockingStatus,
    }),
  );
  const controlsHidden = $derived(controlsIdle && controlsCanAutoHide);
  const playedPercent = $derived(
    durationSeconds > 0
      ? Math.min(100, Math.max(0, (microsecondsToSeconds(positionUs) / durationSeconds) * 100))
      : 0,
  );

  $effect(() => {
    if (!video) return;
    video.volume = volume;
    video.playbackRate = playbackRate;
  });

  $effect(() => {
    if (!video || active) return;
    // Keep the element and media resource alive across app navigation, but do
    // not let audio continue behind Settings or Diagnostics.
    video.pause();
    if (speedMenu) speedMenu.open = false;
  });

  $effect(() => {
    selectedAudio = session.selected_audio_stream_id ?? null;
  });

  $effect(() => {
    const plan = session.playback_plan;
    const attemptKey = `${session.session_id}:${plan.kind}:${session.selected_audio_stream_id ?? ''}`;
    if (
      plan.state === 'queued' &&
      (plan.kind === 'remux' || plan.kind === 'convert_audio') &&
      !conversionBusy &&
      automaticAttempt !== attemptKey
    ) {
      automaticAttempt = attemptKey;
      void preparePlayback(false);
    }
  });

  $effect(() => {
    if (controlsCanAutoHide) revealControls(true);
    else keepControlsVisible();
  });

  onMount(() => {
    void reportCapabilities();
  });

  async function togglePlayback(): Promise<void> {
    if (video.paused) {
      try {
        await video.play();
      } catch (error) {
        onError(asAppError(error));
      }
    } else {
      video.pause();
    }
  }

  function updateClock(mediaTime = video.currentTime): void {
    positionUs = secondsToMicroseconds(mediaTime);
    onTime(positionUs);
    const now = performance.now();
    if (playing && now - lastCoarseCheckpoint >= 1_000) {
      lastCoarseCheckpoint = now;
      void checkpoint('playing');
    }
  }

  function scheduleFrameClock(): void {
    cancelFrameClock();
    if ('requestVideoFrameCallback' in video) {
      frameRequest = video.requestVideoFrameCallback((_now, metadata) => {
        updateClock(metadata.mediaTime);
        if (!video.paused && !video.ended) scheduleFrameClock();
      });
    } else {
      const tick = () => {
        updateClock();
        if (!video.paused && !video.ended) animationRequest = requestAnimationFrame(tick);
      };
      animationRequest = requestAnimationFrame(tick);
    }
  }

  function cancelFrameClock(): void {
    if (frameRequest !== null && 'cancelVideoFrameCallback' in video) {
      video.cancelVideoFrameCallback(frameRequest);
      frameRequest = null;
    }
    if (animationRequest !== null) {
      cancelAnimationFrame(animationRequest);
      animationRequest = null;
    }
  }

  async function checkpoint(state: PlaybackStateV1): Promise<void> {
    const payload: PlaybackCheckpointV1 = {
      session_id: session.session_id,
      revision: ++revision,
      state,
      position_us: secondsToMicroseconds(video.currentTime),
      playback_rate_milli: Math.round(video.playbackRate * 1_000),
      selected_audio_stream_id: selectedAudio,
      observed_monotonic_us: Math.round(performance.now() * 1_000),
    };
    try {
      await api.checkpoint(payload);
    } catch (error) {
      onError(asAppError(error));
    }
  }

  function onPlay(): void {
    if (destroying) return;
    playing = true;
    buffering = false;
    scheduleFrameClock();
    void checkpoint('playing');
  }

  function onPause(): void {
    if (destroying) return;
    playing = false;
    cancelFrameClock();
    updateClock();
    void checkpoint('paused');
  }

  function onSeeked(): void {
    if (destroying) return;
    updateClock();
    void checkpoint('seeking');
  }

  function seek(event: Event): void {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(value)) video.currentTime = value;
  }

  function changeVolume(event: Event): void {
    volume = Number((event.currentTarget as HTMLInputElement).value);
    if (volume > 0) lastAudibleVolume = volume;
  }

  function toggleMute(): void {
    if (volume > 0) {
      lastAudibleVolume = volume;
      volume = 0;
    } else {
      volume = Math.max(0.05, lastAudibleVolume);
    }
  }

  function changeRate(rate: number): void {
    playbackRate = rate;
    video.playbackRate = rate;
    if (speedMenu) speedMenu.open = false;
    void checkpoint(playing ? 'playing' : 'paused');
  }

  function clearControlsHideTimer(): void {
    if (controlsHideTimer !== null) clearTimeout(controlsHideTimer);
    controlsHideTimer = null;
  }

  function keepControlsVisible(): void {
    clearControlsHideTimer();
    controlsIdle = false;
  }

  function revealControls(reschedule = true): void {
    controlsIdle = false;
    clearControlsHideTimer();
    if (!reschedule || !controlsCanAutoHide) return;
    controlsHideTimer = setTimeout(() => {
      controlsHideTimer = null;
      if (controlsCanAutoHide) controlsIdle = true;
    }, PLAYER_CONTROLS_HIDE_DELAY_MS);
  }

  function handleControlsPointerEnter(): void {
    controlsHovered = true;
    keepControlsVisible();
  }

  function handleControlsPointerLeave(): void {
    controlsHovered = false;
  }

  function handleControlsFocusIn(): void {
    controlsFocused = true;
    keepControlsVisible();
  }

  function handleControlsFocusOut(event: FocusEvent): void {
    if (event.relatedTarget instanceof Node && controlsRoot.contains(event.relatedTarget)) return;
    controlsFocused = false;
  }

  function beginScrubbing(): void {
    scrubbing = true;
    keepControlsVisible();
  }

  function endScrubbing(): void {
    scrubbing = false;
  }

  function handleRateToggle(): void {
    rateMenuOpen = speedMenu?.open ?? false;
  }

  function handleFullscreenChange(): void {
    fullscreenActive = Boolean(
      document.fullscreenElement &&
      (document.fullscreenElement === playerRoot ||
        document.fullscreenElement === fullscreenTarget ||
        document.fullscreenElement.contains(playerRoot)),
    );
    revealControls();
  }

  function updateBufferedProgress(): void {
    if (!video || durationSeconds <= 0 || video.buffered.length === 0) {
      bufferedPercent = 0;
      return;
    }
    let furthestEnd = 0;
    for (let index = 0; index < video.buffered.length; index += 1) {
      furthestEnd = Math.max(furthestEnd, video.buffered.end(index));
    }
    bufferedPercent = Math.min(100, Math.max(0, (furthestEnd / durationSeconds) * 100));
  }

  function handleLoadedData(): void {
    playbackErrored = false;
    buffering = false;
    updateBufferedProgress();
    void reportDecode('first_frame_presented');
  }

  function handleVideoClick(event: MouseEvent): void {
    revealControls();
    if (event.detail > 1) return;
    if (videoClickTimer !== null) clearTimeout(videoClickTimer);
    videoClickTimer = setTimeout(() => {
      videoClickTimer = null;
      void togglePlayback();
    }, 220);
  }

  function handleVideoDoubleClick(): void {
    if (videoClickTimer !== null) clearTimeout(videoClickTimer);
    videoClickTimer = null;
    void fullscreen();
  }

  function handleWindowPointerdown(event: PointerEvent): void {
    if (active) revealControls();
    if (speedMenu?.open && event.target instanceof Node && !speedMenu.contains(event.target)) {
      speedMenu.open = false;
    }
  }

  async function reportCapabilities(): Promise<void> {
    const videoCodec = session.video_streams[0]?.codec ?? null;
    const audioCodec =
      session.audio_streams.find((stream) => stream.stream_id === selectedAudio)?.codec ?? null;
    const candidates = await Promise.all([
      capabilityCandidate(session.container, videoCodec, audioCodec),
      capabilityCandidate('mp4', videoCodec, 'aac'),
    ]);
    try {
      const updated = await api.reportCapabilities(session.session_id, {
        revision: ++capabilityRevision,
        candidates,
        tested_webview: navigator.userAgent.slice(0, 256),
      });
      onSessionUpdate(updated);
    } catch (error) {
      onError(asAppError(error));
    }
  }

  async function capabilityCandidate(
    container: string,
    videoCodec: string | null,
    audioCodec: string | null,
  ): Promise<CapabilityCandidateV1> {
    const mime = mediaMime(container, videoCodec, audioCodec);
    const candidate: CapabilityCandidateV1 = {
      container,
      video_codec: videoCodec,
      audio_codec: audioCodec,
      can_play: mime ? video.canPlayType(mime) : '',
      media_capabilities_supported: null,
      media_capabilities_smooth: null,
    };
    if (!mime || !navigator.mediaCapabilities || !videoCodec) return candidate;
    const videoType = mediaMime(container, videoCodec, null);
    if (!videoType) return candidate;
    try {
      const result = await navigator.mediaCapabilities.decodingInfo({
        type: 'file',
        video: {
          contentType: videoType,
          width: session.dimensions?.width ?? 640,
          height: session.dimensions?.height ?? 360,
          bitrate: 2_000_000,
          framerate: session.video_streams[0]?.frame_rate ?? 24,
        },
      });
      candidate.media_capabilities_supported = result.supported;
      candidate.media_capabilities_smooth = result.smooth;
    } catch {
      // canPlayType remains the conservative signal on older WebView2 builds.
    }
    return candidate;
  }

  async function reportDecode(outcome: 'first_frame_presented' | 'failed'): Promise<void> {
    if (outcome === 'first_frame_presented' && reportedPlaybackUrl === session.playback_url) return;
    try {
      const updated = await api.reportDecodeOutcome(session.session_id, outcome);
      if (outcome === 'first_frame_presented') reportedPlaybackUrl = session.playback_url;
      onSessionUpdate(updated);
    } catch (error) {
      onError(asAppError(error));
    }
  }

  async function handleDecodeError(): Promise<void> {
    playbackErrored = true;
    buffering = false;
    keepControlsVisible();
    try {
      const updated = await api.reportDecodeOutcome(session.session_id, 'failed');
      onSessionUpdate(updated);
      if (updated.playback_plan.kind === 'unsupported') {
        onError({
          code: 'PLAYBACK_DECODE_FAILED',
          message: 'The webview could not decode this source and no safe fallback is available.',
          retryable: false,
          diagnostics: null,
        });
      }
    } catch (error) {
      onError(asAppError(error));
    }
  }

  async function preparePlayback(approvedVideoTranscode: boolean): Promise<void> {
    if (conversionBusy) return;
    const operationGeneration = ++conversionGeneration;
    const operationSessionId = session.session_id;
    conversionBusy = true;
    conversionProgress = 0;
    progressTimer = setInterval(() => {
      if (progressPollInFlight) return;
      progressPollInFlight = true;
      void api
        .conversionProgress(operationSessionId)
        .then((value) => {
          if (operationGeneration === conversionGeneration && value !== null) {
            conversionProgress = value;
          }
        })
        .catch(() => undefined)
        .finally(() => {
          progressPollInFlight = false;
        });
    }, 250);
    try {
      const updated = await api.preparePlayback(operationSessionId, approvedVideoTranscode);
      if (operationGeneration === conversionGeneration) onSessionUpdate(updated);
    } catch (error) {
      const appError = asAppError(error);
      if (
        operationGeneration === conversionGeneration &&
        appError.code !== 'CONVERSION_CANCELLED'
      ) {
        onError(appError);
      }
    } finally {
      if (progressTimer !== null) clearInterval(progressTimer);
      progressTimer = null;
      if (operationGeneration === conversionGeneration) {
        conversionGeneration += 1;
        conversionBusy = false;
        conversionProgress = null;
      }
    }
  }

  async function cancelConversion(): Promise<void> {
    try {
      await api.cancelConversion(session.session_id);
    } catch (error) {
      onError(asAppError(error));
    }
  }

  async function changeAudioStream(event: Event): Promise<void> {
    const streamId = (event.currentTarget as HTMLSelectElement).value;
    if (!streamId || streamId === session.selected_audio_stream_id) return;
    video.pause();
    try {
      const updated = await api.selectAudioStream(session.session_id, streamId);
      onSessionUpdate(updated);
    } catch (error) {
      selectedAudio = session.selected_audio_stream_id ?? null;
      onError(asAppError(error));
    }
  }

  function mediaMime(
    container: string,
    videoCodec: string | null,
    audioCodec: string | null,
  ): string | null {
    const type = container === 'webm' ? 'video/webm' : container === 'mp4' ? 'video/mp4' : null;
    if (!type) return null;
    const codecs = [codecLabel(videoCodec), codecLabel(audioCodec)].filter(Boolean);
    return codecs.length > 0 ? `${type}; codecs="${codecs.join(', ')}"` : type;
  }

  function codecLabel(codec: string | null): string {
    if (codec === 'h264') return 'avc1.640028';
    if (codec === 'aac') return 'mp4a.40.2';
    if (codec === 'vp9') return 'vp09.00.10.08';
    if (codec === 'opus') return 'opus';
    if (codec === 'av1') return 'av01.0.08M.08';
    return codec ?? '';
  }

  async function fullscreen(): Promise<void> {
    revealControls();
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await (fullscreenTarget ?? playerRoot).requestFullscreen();
    } catch (error) {
      onError(asAppError(error));
    }
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (!active) return;
    revealControls();
    if (event.key === 'Escape' && speedMenu?.open) {
      event.preventDefault();
      speedMenu.open = false;
      speedMenu.querySelector<HTMLElement>('summary')?.focus({ preventScroll: true });
      return;
    }
    const action = mediaShortcutAction({
      code: event.code,
      key: event.key,
      fullscreen: fullscreenActive,
      targetKind: shortcutTargetKind(event.target),
      ctrlKey: event.ctrlKey,
      metaKey: event.metaKey,
      altKey: event.altKey,
    });
    if (!action) return;
    event.preventDefault();
    if (event.repeat && ['toggle-playback', 'toggle-mute', 'toggle-fullscreen'].includes(action)) {
      return;
    }
    if (action === 'toggle-playback') void togglePlayback();
    else if (action === 'seek-backward') video.currentTime = Math.max(0, video.currentTime - 5);
    else if (action === 'seek-forward') {
      video.currentTime = Math.min(durationSeconds, video.currentTime + 5);
    } else if (action === 'toggle-mute') toggleMute();
    else void fullscreen();
  }

  onDestroy(() => {
    destroying = true;
    conversionGeneration += 1;
    playing = false;
    cancelFrameClock();
    clearControlsHideTimer();
    if (videoClickTimer !== null) clearTimeout(videoClickTimer);
    if (progressTimer !== null) clearInterval(progressTimer);
    if (video) {
      video.pause();
      video.removeAttribute('src');
      video.load();
    }
  });
</script>

<svelte:window
  onkeydown={handleKeydown}
  onpointerdown={handleWindowPointerdown}
  onfullscreenchange={handleFullscreenChange}
/>

<section
  class="player"
  class:controls-idle={controlsHidden}
  bind:this={playerRoot}
  aria-label="Video player"
  onpointermove={() => revealControls()}
  onpointerdown={() => revealControls()}
>
  <video
    bind:this={video}
    src={session.playback_url}
    preload="metadata"
    playsinline
    onplay={onPlay}
    onpause={onPause}
    onseeked={onSeeked}
    onloadeddata={handleLoadedData}
    onprogress={updateBufferedProgress}
    onwaiting={() => (buffering = true)}
    oncanplay={() => (buffering = false)}
    onerror={handleDecodeError}
    onclick={handleVideoClick}
    ondblclick={handleVideoDoubleClick}
  >
    <track kind="captions" />
  </video>

  {#if !playing && !controlsBlockingStatus}
    <button
      class="center-play"
      type="button"
      onclick={togglePlayback}
      data-player-control
      aria-label="Play video"
      title="Play (Space or K)"
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M8 5.4v13.2L18.8 12z" />
      </svg>
    </button>
  {/if}

  {#if buffering && !playbackErrored}
    <div class="buffering-indicator" role="status" aria-label="Buffering video">
      <span aria-hidden="true"></span>
    </div>
  {/if}

  <div
    class="controls"
    class:controls-hidden={controlsHidden}
    bind:this={controlsRoot}
    role="group"
    aria-label="Playback controls"
    onpointerenter={handleControlsPointerEnter}
    onpointerleave={handleControlsPointerLeave}
    onfocusin={handleControlsFocusIn}
    onfocusout={handleControlsFocusOut}
  >
    <label class="scrubber-label" title="Seek">
      <span class="sr-only">Playback position</span>
      <input
        class="scrubber"
        data-player-control
        type="range"
        min="0"
        max={durationSeconds}
        step="0.001"
        value={microsecondsToSeconds(positionUs)}
        style={`--played: ${playedPercent}%; --buffered: ${Math.max(playedPercent, bufferedPercent)}%`}
        oninput={seek}
        onpointerdown={beginScrubbing}
        onpointerup={endScrubbing}
        onpointercancel={endScrubbing}
        onblur={endScrubbing}
        aria-valuetext={`${formatTime(positionUs)} of ${formatTime(session.duration_us)}`}
      />
    </label>
    <div class="control-row">
      <button
        class="transport"
        type="button"
        onclick={togglePlayback}
        data-player-control
        aria-label={playing ? 'Pause video' : 'Play video'}
        title={playing ? 'Pause (Space or K)' : 'Play (Space or K)'}
      >
        {#if playing}
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6.5 5h4v14h-4zm7 0h4v14h-4z" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M8 5.4v13.2L18.8 12z" />
          </svg>
        {/if}
      </button>
      <span class="time">
        <span>{formatTime(positionUs)}</span>
        <span aria-hidden="true">/</span>
        <span>{formatTime(session.duration_us)}</span>
      </span>
      <span class="control-spacer"></span>
      <div class="volume-control">
        <button
          type="button"
          class="icon-control"
          onclick={toggleMute}
          data-player-control
          aria-label={volume > 0 ? 'Mute video' : 'Unmute video'}
          aria-pressed={volume === 0}
          title={volume > 0 ? 'Mute (M)' : 'Unmute (M)'}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M4 9v6h4l5 4V5L8 9zm11.5-.9v7.8a5 5 0 0 0 0-7.8z" />
            {#if volume === 0}<path class="mute-mark" d="m17 9 4 6m0-6-4 6" />{/if}
          </svg>
        </button>
        <label title="Volume">
          <span class="sr-only">Volume</span>
          <input
            data-player-control
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={volume}
            style={`--volume: ${volume * 100}%`}
            oninput={changeVolume}
            aria-label="Volume"
            aria-valuetext={`${Math.round(volume * 100)} percent`}
          />
        </label>
      </div>
      <details class="rate-picker" bind:this={speedMenu} ontoggle={handleRateToggle}>
        <summary
          data-player-control
          aria-label={`Playback speed, ${playbackRate} times`}
          aria-haspopup="menu"
          aria-expanded={rateMenuOpen}
          title="Playback speed">{playbackRate}×</summary
        >
        <div class="rate-options" role="menu" aria-label="Playback speed">
          {#each [0.5, 0.75, 1, 1.25, 1.5, 2] as rate}
            <button
              type="button"
              data-player-control
              role="menuitemradio"
              aria-checked={playbackRate === rate}
              class:active={playbackRate === rate}
              onclick={() => changeRate(rate)}>{rate}×</button
            >
          {/each}
        </div>
      </details>
      {#if session.audio_streams.length > 1}
        <label class="audio-control" title="Audio track">
          <span class="sr-only">Audio track</span>
          <select
            bind:value={selectedAudio}
            data-player-control
            aria-label="Audio track"
            onchange={changeAudioStream}
          >
            {#each session.audio_streams as stream}
              <option value={stream.stream_id}>{stream.language ?? 'und'} · {stream.codec}</option>
            {/each}
          </select>
        </label>
      {/if}
      <button
        type="button"
        class="icon-control fullscreen-control"
        onclick={fullscreen}
        data-player-control
        aria-label={fullscreenActive ? 'Exit fullscreen' : 'Enter fullscreen'}
        title={fullscreenActive ? 'Exit fullscreen (F)' : 'Enter fullscreen (F)'}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          {#if fullscreenActive}
            <path d="M9 4v5H4v2h7V4zm6 0v7h7V9h-5V4zM4 15v2h5v5h2v-7zm13 2h5v-2h-7v7h2z" />
          {:else}
            <path d="M4 4v7h2V6h5V4zm9 0v2h5v5h2V4zM4 13v7h7v-2H6v-5zm14 5h-5v2h7v-7h-2z" />
          {/if}
        </svg>
      </button>
    </div>
  </div>
  {#if conversionBusy || ['awaiting_approval', 'failed', 'cancelled'].includes(session.playback_plan.state)}
    <div class="conversion-panel" role="status">
      <strong>
        {conversionBusy
          ? conversionProgress !== null && conversionProgress < 0
            ? 'Queued behind another media operation…'
            : 'Preparing a compatible playback copy…'
          : 'This video needs a compatibility conversion'}
      </strong>
      <span>
        {session.playback_plan.kind === 'transcode_video'
          ? 'Video conversion can take time and additional disk space.'
          : 'The original file remains unchanged.'}
        {#if conversionBusy && conversionProgress !== null && conversionProgress >= 0}
          {Math.round(conversionProgress * 100)}%
        {/if}
      </span>
      {#if conversionBusy}
        <button type="button" class="secondary" onclick={cancelConversion}>Cancel</button>
      {:else}
        <button
          type="button"
          class="secondary"
          onclick={() => preparePlayback(session.playback_plan.kind === 'transcode_video')}
        >
          {session.playback_plan.kind === 'transcode_video' ? 'Approve conversion' : 'Retry'}
        </button>
      {/if}
    </div>
  {/if}
</section>

<style>
  .player {
    position: relative;
    overflow: hidden;
    border-radius: 0;
    background: #050603;
  }
  .player.controls-idle {
    cursor: none;
  }
  video {
    width: 100%;
    aspect-ratio: 16 / 9;
    display: block;
    background: #050608;
    object-fit: contain;
  }
  .controls {
    position: absolute;
    z-index: 7;
    right: 0;
    bottom: 0;
    left: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 0.3rem;
    padding: 3.6rem clamp(0.8rem, 2vw, 1.5rem) 0.75rem;
    background: linear-gradient(180deg, transparent, rgb(4 5 3 / 88%) 62%, rgb(4 5 3 / 97%));
    opacity: 1;
    transform: translateY(0);
    transition:
      opacity 220ms ease,
      transform 220ms ease;
  }
  .controls.controls-hidden:not(:focus-within) {
    pointer-events: none;
    opacity: 0;
    transform: translateY(0.65rem);
  }
  .control-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
  }
  .control-spacer {
    flex: 1;
  }
  .center-play {
    position: absolute;
    z-index: 5;
    top: 50%;
    left: 50%;
    display: grid;
    place-items: center;
    width: clamp(4rem, 7vw, 5.25rem);
    height: clamp(4rem, 7vw, 5.25rem);
    padding: 0;
    transform: translate(-50%, -50%);
    border: 1px solid rgb(255 255 255 / 38%);
    border-radius: 50%;
    background: rgb(10 11 8 / 68%);
    color: #fff;
    box-shadow: 0 12px 40px rgb(0 0 0 / 38%);
    backdrop-filter: blur(12px);
    transition:
      transform 160ms ease,
      background 160ms ease;
  }
  .center-play:hover {
    transform: translate(-50%, -50%) scale(1.06);
    background: rgb(217 72 63 / 88%);
  }
  .center-play svg {
    width: 45%;
    height: 45%;
    transform: translateX(0.08em);
    fill: currentColor;
  }
  .buffering-indicator {
    position: absolute;
    z-index: 6;
    top: 50%;
    left: 50%;
    display: grid;
    place-items: center;
    width: 4rem;
    height: 4rem;
    transform: translate(-50%, -50%);
  }
  .buffering-indicator span {
    width: 2.7rem;
    height: 2.7rem;
    border: 3px solid rgb(255 255 255 / 26%);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .conversion-panel {
    position: absolute;
    z-index: 9;
    inset: 1rem 1rem auto;
    display: flex;
    align-items: center;
    gap: 0.8rem;
    padding: 0.8rem 1rem;
    border: 1px solid rgb(255 255 255 / 14%);
    border-radius: 2px;
    background: rgb(17 18 15 / 94%);
    color: var(--text);
  }
  .conversion-panel span {
    flex: 1;
    color: var(--muted);
    font-size: 0.8rem;
  }
  .transport {
    display: grid;
    place-items: center;
    width: 2.75rem;
    height: 2.75rem;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: #fff;
    font-weight: 800;
  }
  .transport:hover,
  .icon-control:hover {
    background: rgb(255 255 255 / 14%);
  }
  .transport svg,
  .icon-control svg {
    width: 1.45rem;
    height: 1.45rem;
    fill: currentColor;
  }
  .time {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: rgb(255 255 255 / 86%);
    font-variant-numeric: tabular-nums;
    font-size: 0.72rem;
    white-space: nowrap;
  }
  .scrubber-label {
    display: flex;
    align-items: center;
    min-height: 1.4rem;
  }
  .scrubber {
    height: 1.25rem;
    width: 100%;
    margin: 0;
    padding: 0;
    appearance: none;
    border: 0;
    background: linear-gradient(
      to right,
      var(--accent) 0 var(--played),
      rgb(255 255 255 / 48%) var(--played) var(--buffered),
      rgb(255 255 255 / 24%) var(--buffered) 100%
    );
    background-position: center;
    background-size: 100% 0.22rem;
    background-repeat: no-repeat;
    cursor: pointer;
  }
  .scrubber::-webkit-slider-runnable-track {
    height: 0.22rem;
    background: transparent;
  }
  .scrubber::-webkit-slider-thumb {
    width: 0.85rem;
    height: 0.85rem;
    margin-top: -0.31rem;
    appearance: none;
    border: 0;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 1px 5px rgb(0 0 0 / 60%);
    transition: transform 120ms ease;
  }
  .scrubber:hover::-webkit-slider-thumb,
  .scrubber:focus-visible::-webkit-slider-thumb {
    transform: scale(1.28);
  }
  .volume-control {
    display: flex;
    align-items: center;
    gap: 0.12rem;
  }
  .volume-control label {
    display: flex;
    width: 4.25rem;
    overflow: hidden;
    transition: width 180ms ease;
  }
  .volume-control input {
    width: 4rem;
    height: 1.25rem;
    margin: 0;
    padding: 0;
    appearance: none;
    border: 0;
    background: linear-gradient(
      to right,
      #fff 0 var(--volume),
      rgb(255 255 255 / 28%) var(--volume) 100%
    );
    background-position: center;
    background-size: 100% 0.18rem;
    background-repeat: no-repeat;
  }
  .volume-control input::-webkit-slider-runnable-track {
    height: 0.18rem;
    background: transparent;
  }
  .volume-control input::-webkit-slider-thumb {
    width: 0.7rem;
    height: 0.7rem;
    margin-top: -0.26rem;
    appearance: none;
    border: 0;
    border-radius: 50%;
    background: #fff;
  }
  .icon-control {
    display: grid;
    place-items: center;
    width: 2.75rem;
    height: 2.75rem;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: #fff;
  }
  .mute-mark {
    fill: none;
    stroke: currentColor;
    stroke-linecap: round;
    stroke-width: 1.8;
  }
  select {
    max-width: 8rem;
    min-height: 2.5rem;
    color-scheme: dark;
    border-radius: 2px;
    background: #1e1f1a;
    color: var(--text);
  }
  .rate-picker {
    position: relative;
  }
  .rate-picker summary {
    display: grid;
    place-items: center;
    min-width: 2.75rem;
    min-height: 2.75rem;
    padding: 0.35rem;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: #fff;
    font-size: 0.78rem;
    font-weight: 750;
    text-align: center;
    list-style: none;
    cursor: pointer;
  }
  .rate-picker summary::-webkit-details-marker {
    display: none;
  }
  .rate-picker[open] summary,
  .rate-picker summary:hover {
    background: rgb(255 255 255 / 14%);
  }
  .rate-options {
    position: absolute;
    z-index: 12;
    right: 0;
    bottom: calc(100% + 0.65rem);
    display: grid;
    grid-template-columns: repeat(2, minmax(3.5rem, 1fr));
    gap: 0.25rem;
    width: 8.25rem;
    padding: 0.35rem;
    border: 1px solid var(--line);
    border-radius: 2px;
    background: rgb(20 21 18 / 98%);
    box-shadow: 0 16px 42px rgb(0 0 0 / 55%);
  }
  .rate-options button {
    min-height: 2.25rem;
    padding: 0.42rem 0.35rem;
    border: 0;
    border-radius: 1px;
    background: transparent;
    color: var(--muted);
    font-size: 0.75rem;
  }
  .rate-options button:hover,
  .rate-options button:focus-visible,
  .rate-options button.active {
    background: rgb(217 72 63 / 20%);
    color: #ff8b81;
  }
  :global(.playback-experience:fullscreen) .player,
  .player:fullscreen {
    width: 100%;
    height: 100%;
    border-radius: 0;
    display: block;
  }
  :global(.playback-experience:fullscreen) video,
  .player:fullscreen video {
    width: 100%;
    height: 100%;
    aspect-ratio: auto;
  }
  @media (max-width: 1100px) {
    .controls {
      grid-template-columns: minmax(0, 1fr);
    }
    .audio-control {
      display: none;
    }
  }
  @media (max-width: 640px) {
    .controls {
      padding-inline: 0.65rem;
      padding-bottom: 0.45rem;
    }
    .volume-control label {
      width: 0;
    }
    .control-row {
      gap: 0.15rem;
    }
    .transport,
    .icon-control,
    .rate-picker summary {
      width: 2.55rem;
      min-width: 2.55rem;
      height: 2.55rem;
      min-height: 2.55rem;
    }
  }
  @media (max-width: 420px) {
    .controls {
      grid-template-columns: minmax(0, 1fr);
      gap: 0.2rem;
      padding-inline: 0.55rem;
    }
    .time {
      display: none;
    }
    .scrubber-label {
      min-width: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .controls,
    .center-play,
    .scrubber::-webkit-slider-thumb,
    .volume-control label {
      transition: none;
    }
    .buffering-indicator span {
      animation: none;
    }
  }
</style>
