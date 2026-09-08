//! Local-media import, probing, compatibility planning, tool execution, and cache.

pub mod compatibility;
pub mod conversion;
pub mod import;
pub mod probe;
pub mod session;
pub mod toolchain;

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use contracts::{
    AppErrorV1, DecodeTestOutcomeV1, DependencyHealthV1, MediaSessionId, MediaSessionV1,
    PlaybackCapabilityReportV1, PlaybackCheckpointV1, StreamId, error_codes,
};
use parking_lot::{Condvar, Mutex};
use ports::{
    AssetExtractionPort, ExtractedAssets, ExtractionSpec, MediaSessionPort, ToolProgressCallback,
    ToolRequest,
};
use sha2::{Digest, Sha256};

use crate::{
    conversion::{CachePolicy, ConversionService},
    probe::ProbeService,
    session::SessionRegistry,
    toolchain::ProcessRunner,
};

#[derive(Debug)]
pub struct MediaEngine {
    sessions: SessionRegistry,
    ffmpeg_path: PathBuf,
    runner: Arc<ProcessRunner>,
    conversion: ConversionService,
    conversions: Mutex<HashMap<MediaSessionId, ConversionControl>>,
    mining_operations: Mutex<HashMap<MediaSessionId, Vec<ConversionControl>>>,
    heavy_operation: HeavyOperationQueue,
    cache_error: Mutex<Option<AppErrorV1>>,
}

#[derive(Clone, Debug)]
struct ConversionControl {
    cancelled: Arc<AtomicBool>,
    progress_milli: Arc<AtomicU32>,
    queued: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct HeavyOperationQueue {
    active: Mutex<Option<MediaSessionId>>,
    available: Condvar,
}

#[derive(Debug)]
struct HeavyOperationGuard<'a> {
    queue: &'a HeavyOperationQueue,
}

impl Drop for HeavyOperationGuard<'_> {
    fn drop(&mut self) {
        self.queue.active.lock().take();
        self.queue.available.notify_all();
    }
}

#[derive(Clone, Debug)]
pub struct ExtractedSubtitle {
    pub path: PathBuf,
    pub stream_id: StreamId,
    pub language: Option<String>,
}

impl MediaEngine {
    #[must_use]
    pub fn new(ffmpeg_path: PathBuf, ffprobe_path: PathBuf, cache_dir: PathBuf) -> Self {
        Self::new_with_cache_policy(ffmpeg_path, ffprobe_path, cache_dir, CachePolicy::default())
    }

    #[must_use]
    pub fn new_with_cache_policy(
        ffmpeg_path: PathBuf,
        ffprobe_path: PathBuf,
        cache_dir: PathBuf,
        cache_policy: CachePolicy,
    ) -> Self {
        let runner = Arc::new(ProcessRunner::default());
        let probe = ProbeService::new(ffprobe_path, runner.clone());
        let conversion = ConversionService::new_with_cache_policy(
            ffmpeg_path.clone(),
            cache_dir.clone(),
            runner.clone(),
            cache_policy,
        );
        let cache_error = conversion
            .prune_cache(&std::collections::HashSet::new())
            .err();
        Self {
            sessions: SessionRegistry::new(probe, cache_dir),
            ffmpeg_path,
            runner,
            conversion,
            conversions: Mutex::new(HashMap::new()),
            mining_operations: Mutex::new(HashMap::new()),
            heavy_operation: HeavyOperationQueue::default(),
            cache_error: Mutex::new(cache_error),
        }
    }

    pub fn set_cache_policy(&self, cache_policy: CachePolicy) {
        self.conversion.set_cache_policy(cache_policy);
    }

    pub fn set_audio_language_priority(&self, priority: Vec<String>) -> Result<(), AppErrorV1> {
        self.sessions.set_audio_language_priority(priority)
    }

    pub fn cancel_all(&self) {
        for control in self.conversions.lock().values() {
            control.cancelled.store(true, Ordering::Release);
        }
        for controls in self.mining_operations.lock().values() {
            for control in controls {
                control.cancelled.store(true, Ordering::Release);
            }
        }
        self.heavy_operation.available.notify_all();
    }

    #[must_use]
    pub fn cache_health(&self) -> DependencyHealthV1 {
        let error = self.cache_error.lock().clone();
        DependencyHealthV1 {
            component: "Playback conversion cache".into(),
            available: error.is_none(),
            version: Some("bounded LRU v1".into()),
            action: error
                .as_ref()
                .map(|_| "Check cache permissions and free disk space.".into()),
            error,
        }
    }

    pub fn source_identity_context(
        &self,
        session_id: &MediaSessionId,
    ) -> Result<(PathBuf, String, String), AppErrorV1> {
        self.sessions.source_identity_context(session_id)
    }

    pub fn mark_persistent(
        &self,
        session_id: &MediaSessionId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions.mark_persistent(session_id)
    }

    #[must_use]
    pub fn sessions(&self) -> &SessionRegistry {
        &self.sessions
    }

    pub fn extract_embedded_subtitle(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<ExtractedSubtitle, AppErrorV1> {
        let (source_path, source_fingerprint, stream) = self
            .sessions
            .embedded_subtitle_context(session_id, stream_id)?;
        let (extension, output_format) = match stream.codec.as_str() {
            "ass" | "ssa" => ("ass", "ass"),
            "webvtt" => ("vtt", "webvtt"),
            _ => ("srt", "srt"),
        };
        let mut hasher = Sha256::new();
        hasher.update(b"migaku-embedded-subtitle-v1\0");
        hasher.update(source_fingerprint.as_bytes());
        hasher.update(stream_id.as_str().as_bytes());
        let key = hex::encode(hasher.finalize());
        let directory = self.sessions.cache_dir().join("embedded-subtitles");
        fs::create_dir_all(&directory).map_err(subtitle_extraction_error)?;
        let output_path = directory.join(format!("{key}.{extension}"));
        if !output_path.metadata().is_ok_and(|value| value.len() > 0) {
            let partial_path = directory.join(format!("{key}.{extension}.part"));
            let output = self.runner.run_cancellable(
                ToolRequest {
                    executable: self.ffmpeg_path.clone(),
                    args: vec![
                        "-nostdin".into(),
                        "-hide_banner".into(),
                        "-y".into(),
                        "-i".into(),
                        source_path.as_os_str().to_string_lossy().into_owned(),
                        "-map".into(),
                        format!("0:{}", stream.index),
                        "-f".into(),
                        output_format.into(),
                        partial_path.as_os_str().to_string_lossy().into_owned(),
                    ],
                    timeout: Duration::from_secs(60),
                    max_stdout_bytes: 64 * 1024,
                    max_stderr_bytes: 1024 * 1024,
                },
                &Arc::new(AtomicBool::new(false)),
            );
            match output {
                Ok(output)
                    if output.status_code == Some(0)
                        && partial_path.metadata().is_ok_and(|value| value.len() > 0) =>
                {
                    fs::rename(&partial_path, &output_path).map_err(subtitle_extraction_error)?;
                }
                Ok(_) => {
                    let _ = fs::remove_file(&partial_path);
                    return Err(subtitle_extraction_error(
                        "FFmpeg returned a failure status",
                    ));
                }
                Err(error) => {
                    let _ = fs::remove_file(&partial_path);
                    return Err(error);
                }
            }
        }
        Ok(ExtractedSubtitle {
            path: output_path,
            stream_id: stream.stream_id,
            language: stream.language,
        })
    }

    #[must_use]
    pub fn conversion_progress(&self, session_id: &MediaSessionId) -> Option<f32> {
        self.conversions
            .lock()
            .get(session_id)
            .map(operation_progress)
    }

    #[must_use]
    pub fn mining_progress(&self, session_id: &MediaSessionId) -> Option<f32> {
        self.mining_operations
            .lock()
            .get(session_id)
            .and_then(|controls| {
                controls
                    .iter()
                    .filter(|control| !control.queued.load(Ordering::Acquire))
                    .map(operation_progress)
                    .max_by(f32::total_cmp)
                    .or_else(|| (!controls.is_empty()).then_some(-1.0))
            })
    }

    #[must_use]
    pub fn cancel_mining(&self, session_id: &MediaSessionId) -> bool {
        let operations = self.mining_operations.lock();
        let Some(controls) = operations.get(session_id) else {
            return false;
        };
        for control in controls {
            control.cancelled.store(true, Ordering::Release);
        }
        self.heavy_operation.available.notify_all();
        true
    }

    fn reserve_heavy_operation(
        &self,
        session_id: &MediaSessionId,
        control: &ConversionControl,
    ) -> Result<HeavyOperationGuard<'_>, AppErrorV1> {
        let mut active = self.heavy_operation.active.lock();
        while active.is_some() {
            control.queued.store(true, Ordering::Release);
            if control.cancelled.load(Ordering::Acquire) {
                control.queued.store(false, Ordering::Release);
                return Err(operation_cancelled());
            }
            self.heavy_operation
                .available
                .wait_for(&mut active, Duration::from_millis(100));
        }
        if control.cancelled.load(Ordering::Acquire) {
            control.queued.store(false, Ordering::Release);
            return Err(operation_cancelled());
        }
        *active = Some(session_id.clone());
        control.queued.store(false, Ordering::Release);
        drop(active);
        Ok(HeavyOperationGuard {
            queue: &self.heavy_operation,
        })
    }

    fn finish_mining_operation(
        &self,
        session_id: &MediaSessionId,
        expected_control: &ConversionControl,
        result: Result<ExtractedAssets, AppErrorV1>,
    ) -> Result<ExtractedAssets, AppErrorV1> {
        let accepted_cancellation = self.remove_mining_operation(session_id, expected_control);
        if accepted_cancellation {
            if let Ok(extracted) = &result {
                remove_extracted_assets(extracted);
            }
            return Err(mining_cancelled());
        }
        result
    }

    fn remove_mining_operation(
        &self,
        session_id: &MediaSessionId,
        expected_control: &ConversionControl,
    ) -> bool {
        let mut operations = self.mining_operations.lock();
        let Some(controls) = operations.get_mut(session_id) else {
            return false;
        };
        let Some(index) = controls
            .iter()
            .position(|control| Arc::ptr_eq(&control.cancelled, &expected_control.cancelled))
        else {
            return false;
        };
        let control = controls.swap_remove(index);
        let accepted_cancellation = control.cancelled.load(Ordering::Acquire);
        if controls.is_empty() {
            operations.remove(session_id);
        }
        accepted_cancellation
    }
}

fn operation_progress(control: &ConversionControl) -> f32 {
    if control.queued.load(Ordering::Acquire) {
        -1.0
    } else {
        control.progress_milli.load(Ordering::Acquire) as f32 / 1_000.0
    }
}

fn operation_cancelled() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_CANCELLED,
        "The queued media operation was cancelled before it started.",
        true,
    )
}

fn mining_cancelled() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_CANCELLED,
        "Mining media extraction was cancelled and its staging files were removed.",
        true,
    )
}

impl Drop for MediaEngine {
    fn drop(&mut self) {
        for control in self.conversions.get_mut().values() {
            control.cancelled.store(true, Ordering::Release);
        }
        for controls in self.mining_operations.get_mut().values() {
            for control in controls {
                control.cancelled.store(true, Ordering::Release);
            }
        }
    }
}

fn subtitle_extraction_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "The embedded text subtitle track could not be extracted.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn asset_extraction_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "The audio or image asset could not be extracted and verified.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn remove_extracted_assets(extracted: &ExtractedAssets) {
    let _ = fs::remove_file(&extracted.audio_path);
    let _ = fs::remove_file(&extracted.image_path);
}

fn ffmpeg_progress_callback(
    progress_milli: Arc<AtomicU32>,
    duration_us: i64,
) -> ToolProgressCallback {
    let pending = Arc::new(Mutex::new(Vec::<u8>::new()));
    Arc::new(move |chunk| {
        let mut pending = pending.lock();
        pending.extend_from_slice(chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = pending.drain(..=newline).collect();
            let text = String::from_utf8_lossy(&line);
            let Some(value) = text.trim().strip_prefix("out_time_us=") else {
                continue;
            };
            let Ok(position_us) = value.parse::<i64>() else {
                continue;
            };
            let denominator = duration_us.max(1) as f64;
            let progress = ((position_us.max(0) as f64 / denominator) * 1_000.0)
                .round()
                .clamp(0.0, 999.0) as u32;
            progress_milli.store(progress, Ordering::Release);
        }
        if pending.len() > 64 * 1024 {
            pending.clear();
        }
    })
}

impl AssetExtractionPort for MediaEngine {
    fn extract(&self, spec: &ExtractionSpec) -> Result<ExtractedAssets, AppErrorV1> {
        let control = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(0)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        {
            let mut operations = self.mining_operations.lock();
            operations
                .entry(spec.session_id.clone())
                .or_default()
                .push(control.clone());
        }
        let _heavy_operation = match self.reserve_heavy_operation(&spec.session_id, &control) {
            Ok(guard) => guard,
            Err(error) => {
                self.remove_mining_operation(&spec.session_id, &control);
                return Err(error);
            }
        };
        let result = (|| {
            let (source_path, duration_us) = self.sessions.source_context(&spec.session_id)?;
            let mut extracted = conversion::extract_assets(
                &self.ffmpeg_path,
                self.runner.as_ref(),
                &source_path,
                self.sessions.cache_dir(),
                duration_us,
                spec,
                conversion::ExtractionSignals {
                    cancelled: &control.cancelled,
                    progress_milli: control.progress_milli.as_ref(),
                },
            )?;
            let validation = (|| {
                ensure_mining_active(&control)?;
                let audio = self.sessions.probe_asset(&extracted.audio_path)?;
                ensure_mining_active(&control)?;
                let image = self.sessions.probe_asset(&extracted.image_path)?;
                ensure_mining_active(&control)?;
                let audio_duration_us = audio.duration_us.ok_or_else(|| {
                    asset_extraction_error("Extracted audio probe omitted its duration.")
                })?;
                if audio.kind != Some(contracts::MediaStreamKindV1::Audio) || audio.codec != "mp3" {
                    return Err(asset_extraction_error(
                        "Extracted audio did not probe as MP3 audio.",
                    ));
                }
                if image.kind != Some(contracts::MediaStreamKindV1::Video)
                    || !matches!(image.codec.as_str(), "mjpeg" | "jpeg")
                {
                    return Err(asset_extraction_error(
                        "Extracted frame did not probe as a JPEG image.",
                    ));
                }
                let width = image.width.ok_or_else(|| {
                    asset_extraction_error("Extracted frame probe omitted its width.")
                })?;
                let height = image.height.ok_or_else(|| {
                    asset_extraction_error("Extracted frame probe omitted its height.")
                })?;
                ensure_mining_active(&control)?;
                Ok((audio_duration_us, width, height))
            })();
            let (audio_duration_us, width, height) = match validation {
                Ok(metadata) => metadata,
                Err(error) => {
                    remove_extracted_assets(&extracted);
                    return Err(error);
                }
            };
            extracted.metadata.extend([
                ("audio_duration_us".into(), audio_duration_us.to_string()),
                ("audio_mime".into(), "audio/mpeg".into()),
                ("image_mime".into(), "image/jpeg".into()),
                ("image_width".into(), width.to_string()),
                ("image_height".into(), height.to_string()),
            ]);
            Ok(extracted)
        })();
        self.finish_mining_operation(&spec.session_id, &control, result)
    }
}

fn ensure_mining_active(control: &ConversionControl) -> Result<(), AppErrorV1> {
    if control.cancelled.load(Ordering::Acquire) {
        return Err(mining_cancelled());
    }
    Ok(())
}

impl MediaSessionPort for MediaEngine {
    fn import_path(&self, authorized_path: PathBuf) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions.import(authorized_path)
    }

    fn apply_capabilities(
        &self,
        session_id: &MediaSessionId,
        report: PlaybackCapabilityReportV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions.apply_capabilities(session_id, &report)
    }

    fn report_decode_outcome(
        &self,
        session_id: &MediaSessionId,
        outcome: DecodeTestOutcomeV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions.report_decode_outcome(session_id, outcome)
    }

    fn prepare_playback(
        &self,
        session_id: &MediaSessionId,
        approved_video_transcode: bool,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let control = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(0)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        {
            let mut conversions = self.conversions.lock();
            if conversions.contains_key(session_id) {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "This playback conversion is already queued or running.",
                    true,
                ));
            }
            conversions.insert(session_id.clone(), control.clone());
        }
        let _heavy_operation = match self.reserve_heavy_operation(session_id, &control) {
            Ok(guard) => guard,
            Err(error) => {
                self.conversions.lock().remove(session_id);
                return Err(error);
            }
        };
        if let Err(error) = self
            .conversion
            .prune_cache(&self.sessions.protected_cache_paths())
        {
            *self.cache_error.lock() = Some(error.clone());
            self.conversions.lock().remove(session_id);
            return Err(error);
        }
        *self.cache_error.lock() = None;
        let request = match self
            .sessions
            .begin_conversion(session_id, approved_video_transcode)
        {
            Ok(request) => request,
            Err(error) => {
                self.conversions.lock().remove(session_id);
                return Err(error);
            }
        };
        let progress =
            ffmpeg_progress_callback(control.progress_milli.clone(), request.duration_us);
        let result = self
            .conversion
            .convert(&request, &control.cancelled, progress);
        let final_result = match result {
            Ok(artifact) => match self.sessions.complete_conversion(session_id, &artifact) {
                Ok(session) => {
                    match self
                        .conversion
                        .prune_cache(&self.sessions.protected_cache_paths())
                    {
                        Ok(_) => *self.cache_error.lock() = None,
                        Err(error) => *self.cache_error.lock() = Some(error),
                    }
                    Ok(session)
                }
                Err(error) => match self.sessions.fail_conversion(session_id, false) {
                    Ok(()) => Err(error),
                    Err(state_error) => Err(state_error),
                },
            },
            Err(error) => {
                let was_cancelled = control.cancelled.load(Ordering::Acquire)
                    || error.code == error_codes::CONVERSION_CANCELLED;
                match self.sessions.fail_conversion(session_id, was_cancelled) {
                    Ok(()) => Err(error),
                    Err(state_error) => Err(state_error),
                }
            }
        };
        self.conversions.lock().remove(session_id);
        final_result
    }

    fn cancel_conversion(&self, session_id: &MediaSessionId) -> Result<bool, AppErrorV1> {
        let conversions = self.conversions.lock();
        let Some(control) = conversions.get(session_id) else {
            return Ok(false);
        };
        control.cancelled.store(true, Ordering::Release);
        self.heavy_operation.available.notify_all();
        Ok(true)
    }

    fn select_audio_stream(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions.select_audio_stream(session_id, stream_id)
    }

    fn checkpoint(&self, checkpoint: PlaybackCheckpointV1) -> Result<bool, AppErrorV1> {
        self.sessions.checkpoint(checkpoint)
    }

    fn close(&self, session_id: &MediaSessionId) -> Result<(), AppErrorV1> {
        if let Some(control) = self.conversions.lock().get(session_id) {
            control.cancelled.store(true, Ordering::Release);
        }
        if let Some(controls) = self.mining_operations.lock().get(session_id) {
            for control in controls {
                control.cancelled.store(true, Ordering::Release);
            }
        }
        self.heavy_operation.available.notify_all();
        self.sessions.close(session_id)
    }
}

#[cfg(test)]
mod real_tool_tests {
    use std::path::PathBuf;

    use contracts::{
        CapabilityCandidateV1, JobStateV1, PlaybackCapabilityReportV1, PlaybackPlanKindV1,
        SubtitleKindV1,
    };
    use ports::MediaSessionPort;
    use tempfile::tempdir;

    use super::*;

    fn configured_engine() -> Option<(MediaEngine, tempfile::TempDir)> {
        let ffmpeg = std::env::var_os("MIGAKU_TEST_FFMPEG").map(PathBuf::from)?;
        let ffprobe = std::env::var_os("MIGAKU_TEST_FFPROBE").map(PathBuf::from)?;
        let cache = tempdir().ok()?;
        Some((
            MediaEngine::new(ffmpeg, ffprobe, cache.path().join("playback")),
            cache,
        ))
    }

    #[test]
    fn heavy_operations_queue_and_start_after_the_active_job() -> Result<(), AppErrorV1> {
        let cache = tempdir().map_err(asset_extraction_error)?;
        let engine = Arc::new(MediaEngine::new(
            PathBuf::from("missing-ffmpeg"),
            PathBuf::from("missing-ffprobe"),
            cache.path().join("cache"),
        ));
        let first_id = MediaSessionId::new("first");
        let second_id = MediaSessionId::new("second");
        let first_control = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(0)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        let second_control = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(0)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        let guard = engine.reserve_heavy_operation(&first_id, &first_control)?;
        let queued_flag = second_control.queued.clone();
        let queued_engine = engine.clone();
        let handle = std::thread::spawn(move || {
            let _guard = queued_engine.reserve_heavy_operation(&second_id, &second_control)?;
            Ok::<_, AppErrorV1>(())
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !queued_flag.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(queued_flag.load(Ordering::Acquire));
        drop(guard);
        handle
            .join()
            .map_err(|_| asset_extraction_error("Queued operation worker panicked."))??;
        Ok(())
    }

    #[test]
    fn accepted_mining_cancellation_cannot_return_assets() -> Result<(), AppErrorV1> {
        let cache = tempdir().map_err(asset_extraction_error)?;
        let engine = MediaEngine::new(
            PathBuf::from("missing-ffmpeg"),
            PathBuf::from("missing-ffprobe"),
            cache.path().join("cache"),
        );
        let session_id = MediaSessionId::new("cancelled-mining");
        let control = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(1_000)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        engine
            .mining_operations
            .lock()
            .insert(session_id.clone(), vec![control.clone()]);
        let audio_path = cache.path().join("staging.mp3");
        let image_path = cache.path().join("staging.jpg");
        fs::write(&audio_path, b"audio").map_err(asset_extraction_error)?;
        fs::write(&image_path, b"image").map_err(asset_extraction_error)?;

        assert!(engine.cancel_mining(&session_id));
        let result = engine.finish_mining_operation(
            &session_id,
            &control,
            Ok(ExtractedAssets {
                audio_path: audio_path.clone(),
                image_path: image_path.clone(),
                metadata: std::collections::BTreeMap::new(),
            }),
        );
        assert!(matches!(
            result,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::CONVERSION_CANCELLED
        ));
        assert!(!audio_path.exists());
        assert!(!image_path.exists());
        assert!(!engine.cancel_mining(&session_id));
        Ok(())
    }

    #[test]
    fn same_session_mining_operations_are_queued_and_cancelled_together() -> Result<(), AppErrorV1>
    {
        let cache = tempdir().map_err(asset_extraction_error)?;
        let engine = MediaEngine::new(
            PathBuf::from("missing-ffmpeg"),
            PathBuf::from("missing-ffprobe"),
            cache.path().join("cache"),
        );
        let session_id = MediaSessionId::new("shared-session");
        let active = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(250)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        let queued = ConversionControl {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress_milli: Arc::new(AtomicU32::new(0)),
            queued: Arc::new(AtomicBool::new(true)),
        };
        engine
            .mining_operations
            .lock()
            .insert(session_id.clone(), vec![active.clone(), queued.clone()]);

        assert_eq!(engine.mining_progress(&session_id), Some(0.25));
        assert!(engine.cancel_mining(&session_id));
        assert!(active.cancelled.load(Ordering::Acquire));
        assert!(queued.cancelled.load(Ordering::Acquire));
        let active_result = engine.finish_mining_operation(
            &session_id,
            &active,
            Err(asset_extraction_error("test cancellation")),
        );
        assert!(matches!(
            active_result,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::CONVERSION_CANCELLED
        ));
        assert_eq!(engine.mining_progress(&session_id), Some(-1.0));
        let queued_result = engine.finish_mining_operation(
            &session_id,
            &queued,
            Err(asset_extraction_error("test cancellation")),
        );
        assert!(matches!(
            queued_result,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::CONVERSION_CANCELLED
        ));
        assert_eq!(engine.mining_progress(&session_id), None);
        Ok(())
    }

    #[test]
    #[ignore = "requires MIGAKU_TEST_* paths and generated FFmpeg fixtures"]
    fn real_tool_remux_promotes_only_a_scoped_cache_artifact() -> Result<(), AppErrorV1> {
        let Some((engine, _cache)) = configured_engine() else {
            return Ok(());
        };
        let Some(fixture) = std::env::var_os("MIGAKU_TEST_REMUX_FIXTURE").map(PathBuf::from) else {
            return Ok(());
        };
        let imported = engine.import_path(fixture)?;
        let planned = engine.apply_capabilities(
            &imported.session_id,
            PlaybackCapabilityReportV1 {
                revision: 1,
                candidates: vec![CapabilityCandidateV1 {
                    container: "mp4".into(),
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    can_play: "probably".into(),
                    media_capabilities_supported: Some(true),
                    media_capabilities_smooth: Some(true),
                }],
                tested_webview: "real-tool-test".into(),
            },
        )?;
        assert_eq!(planned.playback_plan.kind, PlaybackPlanKindV1::Remux);
        let ready = engine.prepare_playback(&imported.session_id, false)?;
        assert_eq!(ready.playback_plan.state, JobStateV1::Ready);
        assert!(ready.playback_plan.cache_key.is_some());
        let slice =
            engine
                .sessions()
                .read_range(&imported.session_id, 0, Some(16 * 1024 * 1024))?;
        assert!(slice.bytes.len() <= 8 * 1024 * 1024);
        assert_eq!(slice.content_type, "video/mp4");
        Ok(())
    }

    #[test]
    #[ignore = "requires MIGAKU_TEST_* paths and generated FFmpeg fixtures"]
    fn real_tool_extracts_only_selected_embedded_text_track() -> Result<(), AppErrorV1> {
        let Some((engine, _cache)) = configured_engine() else {
            return Ok(());
        };
        let Some(fixture) = std::env::var_os("MIGAKU_TEST_EMBEDDED_FIXTURE").map(PathBuf::from)
        else {
            return Ok(());
        };
        let imported = engine.import_path(fixture)?;
        let stream = imported
            .subtitle_streams
            .iter()
            .find(|stream| stream.subtitle_kind == Some(SubtitleKindV1::Text))
            .ok_or_else(|| {
                AppErrorV1::new(
                    error_codes::MEDIA_UNSUPPORTED,
                    "The real-tool fixture has no embedded text subtitle.",
                    false,
                )
            })?;
        let extracted =
            engine.extract_embedded_subtitle(&imported.session_id, &stream.stream_id)?;
        let text = std::fs::read_to_string(extracted.path).map_err(subtitle_extraction_error)?;
        assert!(text.contains("映画"));
        Ok(())
    }

    #[test]
    #[ignore = "requires MIGAKU_TEST_* paths and generated FFmpeg fixtures"]
    fn real_tool_converts_audio_without_transcoding_video() -> Result<(), AppErrorV1> {
        let Some((engine, _cache)) = configured_engine() else {
            return Ok(());
        };
        let Some(fixture) = std::env::var_os("MIGAKU_TEST_AUDIO_FIXTURE").map(PathBuf::from) else {
            return Ok(());
        };
        let imported = engine.import_path(fixture)?;
        let planned = engine.apply_capabilities(
            &imported.session_id,
            PlaybackCapabilityReportV1 {
                revision: 1,
                candidates: vec![CapabilityCandidateV1 {
                    container: "mp4".into(),
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    can_play: "probably".into(),
                    media_capabilities_supported: Some(true),
                    media_capabilities_smooth: Some(true),
                }],
                tested_webview: "real-tool-test".into(),
            },
        )?;
        assert_eq!(planned.playback_plan.kind, PlaybackPlanKindV1::ConvertAudio);
        let ready = engine.prepare_playback(&imported.session_id, false)?;
        assert_eq!(ready.playback_plan.state, JobStateV1::Ready);
        assert_eq!(ready.playback_plan.output_codecs, ["h264", "aac"]);
        Ok(())
    }

    #[test]
    #[ignore = "requires MIGAKU_TEST_* paths and generated FFmpeg fixtures"]
    fn real_tool_requires_approval_then_transcodes_video() -> Result<(), AppErrorV1> {
        let Some((engine, _cache)) = configured_engine() else {
            return Ok(());
        };
        let Some(fixture) = std::env::var_os("MIGAKU_TEST_VIDEO_FIXTURE").map(PathBuf::from) else {
            return Ok(());
        };
        let imported = engine.import_path(fixture)?;
        let planned = engine.apply_capabilities(
            &imported.session_id,
            PlaybackCapabilityReportV1 {
                revision: 1,
                candidates: vec![CapabilityCandidateV1 {
                    container: "mp4".into(),
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    can_play: "probably".into(),
                    media_capabilities_supported: Some(true),
                    media_capabilities_smooth: Some(true),
                }],
                tested_webview: "real-tool-test".into(),
            },
        )?;
        assert_eq!(
            planned.playback_plan.kind,
            PlaybackPlanKindV1::TranscodeVideo
        );
        let denied = engine.prepare_playback(&imported.session_id, false);
        assert!(matches!(
            denied,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::CONVERSION_REQUIRED
        ));
        let ready = engine.prepare_playback(&imported.session_id, true)?;
        assert_eq!(ready.playback_plan.state, JobStateV1::Ready);
        assert_eq!(ready.playback_plan.output_codecs, ["h264", "aac"]);
        Ok(())
    }

    #[test]
    #[ignore = "requires MIGAKU_TEST_* paths and generated FFmpeg fixtures"]
    fn real_tool_extracts_bounded_audio_and_frame_assets() -> Result<(), AppErrorV1> {
        let Some((engine, _cache)) = configured_engine() else {
            return Ok(());
        };
        let Some(fixture) = std::env::var_os("MIGAKU_TEST_DIRECT_FIXTURE").map(PathBuf::from)
        else {
            return Ok(());
        };
        let imported = engine.import_path(fixture)?;
        let assets = engine.extract(&ExtractionSpec {
            session_id: imported.session_id,
            start_us: 500_000,
            end_us: 2_000_000,
            frame_us: 1_000_000,
            profile: "real-tool-test".into(),
        })?;
        assert!(
            assets
                .audio_path
                .metadata()
                .is_ok_and(|value| value.len() > 0)
        );
        assert!(
            assets
                .image_path
                .metadata()
                .is_ok_and(|value| value.len() > 0)
        );
        Ok(())
    }
}
