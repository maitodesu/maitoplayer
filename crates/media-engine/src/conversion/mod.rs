use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, SystemTime},
};

use contracts::{AppErrorV1, StreamId, TimestampUs, error_codes};
use parking_lot::RwLock;
use ports::{ExtractedAssets, ExtractionSpec, MediaToolPort, ToolProgressCallback, ToolRequest};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionKind {
    Remux,
    ConvertAudio,
    TranscodeVideo,
}

#[derive(Clone, Debug)]
pub struct ConversionRequest {
    pub source_path: PathBuf,
    pub source_fingerprint: String,
    pub selected_streams: Vec<StreamId>,
    pub video_stream_index: u32,
    pub audio_stream_index: Option<u32>,
    pub kind: ConversionKind,
    pub duration_us: TimestampUs,
    pub approved_video_transcode: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversionArtifact {
    pub path: PathBuf,
    pub cache_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachePolicy {
    pub max_bytes: u64,
    pub max_age: Duration,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            max_bytes: 20 * 1024 * 1024 * 1024,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheCleanupReport {
    pub scanned: usize,
    pub removed: usize,
    pub bytes_reclaimed: u64,
    pub bytes_remaining: u64,
}

pub struct ConversionService {
    ffmpeg_path: PathBuf,
    cache_dir: PathBuf,
    runner: Arc<dyn MediaToolPort>,
    tool_profile: String,
    cache_policy: RwLock<CachePolicy>,
}

impl std::fmt::Debug for ConversionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConversionService")
            .field("ffmpeg_path", &self.ffmpeg_path)
            .field("cache_dir", &self.cache_dir)
            .field("tool_profile", &self.tool_profile)
            .field("cache_policy", &*self.cache_policy.read())
            .finish_non_exhaustive()
    }
}

impl ConversionService {
    #[must_use]
    pub fn new(ffmpeg_path: PathBuf, cache_dir: PathBuf, runner: Arc<dyn MediaToolPort>) -> Self {
        Self::new_with_cache_policy(ffmpeg_path, cache_dir, runner, CachePolicy::default())
    }

    #[must_use]
    pub fn new_with_cache_policy(
        ffmpeg_path: PathBuf,
        cache_dir: PathBuf,
        runner: Arc<dyn MediaToolPort>,
        cache_policy: CachePolicy,
    ) -> Self {
        let tool_profile = tool_profile(&ffmpeg_path, runner.as_ref());
        Self {
            ffmpeg_path,
            cache_dir,
            runner,
            tool_profile,
            cache_policy: RwLock::new(cache_policy),
        }
    }

    pub fn set_cache_policy(&self, cache_policy: CachePolicy) {
        *self.cache_policy.write() = cache_policy;
    }

    pub fn prune_cache(
        &self,
        protected: &HashSet<PathBuf>,
    ) -> Result<CacheCleanupReport, AppErrorV1> {
        fs::create_dir_all(&self.cache_dir).map_err(cache_error)?;
        let policy = *self.cache_policy.read();
        let now = SystemTime::now();
        let mut entries = Vec::new();
        let mut report = CacheCleanupReport::default();
        for item in fs::read_dir(&self.cache_dir)
            .map_err(cache_error)?
            .take(10_000)
        {
            let item = item.map_err(cache_error)?;
            let path = item.path();
            let metadata = item.metadata().map_err(cache_error)?;
            if !metadata.is_file() {
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str());
            if extension == Some("part") {
                report.scanned += 1;
                fs::remove_file(&path).map_err(cache_error)?;
                report.removed += 1;
                report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(metadata.len());
                continue;
            }
            if extension == Some("access") {
                report.scanned += 1;
                if !path.with_extension("mp4").is_file() {
                    fs::remove_file(&path).map_err(cache_error)?;
                    report.removed += 1;
                    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(metadata.len());
                }
                continue;
            }
            if extension != Some("mp4") {
                continue;
            }
            report.scanned += 1;
            let accessed = access_marker(&path)
                .metadata()
                .and_then(|marker| marker.modified())
                .or_else(|_| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push((path, metadata.len(), accessed));
        }
        entries.sort_by_key(|(_, _, accessed)| *accessed);
        report.bytes_remaining = entries
            .iter()
            .map(|(_, size, _)| *size)
            .fold(0_u64, u64::saturating_add);
        for (path, size, accessed) in entries {
            let expired = now
                .duration_since(accessed)
                .is_ok_and(|age| age > policy.max_age);
            let over_budget = report.bytes_remaining > policy.max_bytes;
            if protected.contains(&path) || (!expired && !over_budget) {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    let _ = fs::remove_file(access_marker(&path));
                    report.removed += 1;
                    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(size);
                    report.bytes_remaining = report.bytes_remaining.saturating_sub(size);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(cache_error(error)),
            }
        }
        Ok(report)
    }

    pub fn convert(
        &self,
        request: &ConversionRequest,
        cancelled: &Arc<AtomicBool>,
        progress: ToolProgressCallback,
    ) -> Result<ConversionArtifact, AppErrorV1> {
        if request.kind == ConversionKind::TranscodeVideo && !request.approved_video_transcode {
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_REQUIRED,
                "Video conversion requires confirmation because it can take time and disk space.",
                true,
            ));
        }
        fs::create_dir_all(&self.cache_dir).map_err(cache_error)?;
        let cache_key = conversion_cache_key(request, &self.tool_profile);
        let final_path = self.cache_dir.join(format!("{cache_key}.mp4"));
        if verified_artifact(&final_path) {
            touch_access(&final_path)?;
            return Ok(ConversionArtifact {
                path: final_path,
                cache_key,
            });
        }
        let partial_path = self.cache_dir.join(format!("{cache_key}.part"));
        if partial_path.exists() {
            fs::remove_file(&partial_path).map_err(cache_error)?;
        }
        let args = conversion_args(request, &partial_path);
        let timeout_seconds = match request.kind {
            ConversionKind::Remux => 300,
            ConversionKind::ConvertAudio => 900,
            ConversionKind::TranscodeVideo => 7_200,
        };
        let output = self.runner.run_cancellable_with_progress(
            ToolRequest {
                executable: self.ffmpeg_path.clone(),
                args,
                timeout: Duration::from_secs(timeout_seconds),
                max_stdout_bytes: 4 * 1024 * 1024,
                max_stderr_bytes: 2 * 1024 * 1024,
            },
            cancelled,
            progress,
        );
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                let _ = fs::remove_file(&partial_path);
                return Err(error);
            }
        };
        if output.status_code != Some(0) || !verified_artifact(&partial_path) {
            let _ = fs::remove_file(&partial_path);
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_FAILED,
                "Playback conversion failed. The incomplete file was removed.",
                true,
            ));
        }
        fs::rename(&partial_path, &final_path).map_err(cache_error)?;
        touch_access(&final_path)?;
        Ok(ConversionArtifact {
            path: final_path,
            cache_key,
        })
    }
}

fn access_marker(path: &Path) -> PathBuf {
    path.with_extension("access")
}

fn touch_access(path: &Path) -> Result<(), AppErrorV1> {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| cache_error(std::io::Error::other(error)))?
        .as_secs();
    fs::write(access_marker(path), timestamp.to_string()).map_err(cache_error)
}

pub(crate) fn remove_artifact_and_marker(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(access_marker(path));
}

fn tool_profile(path: &Path, runner: &dyn MediaToolPort) -> String {
    let version = crate::toolchain::inspect_tool_with_runner(path, "ffmpeg", runner)
        .unwrap_or_else(|_| "unavailable".into());
    let Ok(metadata) = path.metadata() else {
        return format!(
            "ffmpeg-profile-v3-{}",
            hex::encode(&Sha256::digest(version.as_bytes())[..16])
        );
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_secs());
    let version_hash = hex::encode(&Sha256::digest(version.as_bytes())[..16]);
    format!(
        "ffmpeg-profile-v3-{}-{modified}-{version_hash}",
        metadata.len()
    )
}

#[must_use]
pub fn conversion_cache_key(request: &ConversionRequest, tool_profile: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-conversion-v1\0");
    hasher.update(request.source_fingerprint.as_bytes());
    for stream in &request.selected_streams {
        hasher.update([0]);
        hasher.update(stream.as_str().as_bytes());
    }
    hasher.update([request.kind as u8]);
    hasher.update(tool_profile.as_bytes());
    hex::encode(hasher.finalize())
}

fn conversion_args(request: &ConversionRequest, output: &Path) -> Vec<String> {
    let mut args = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-y".into(),
        "-fflags".into(),
        "+genpts".into(),
        "-i".into(),
        request
            .source_path
            .as_os_str()
            .to_string_lossy()
            .into_owned(),
        "-map".into(),
        format!("0:{}", request.video_stream_index),
    ];
    if let Some(audio_index) = request.audio_stream_index {
        args.extend(["-map".into(), format!("0:{audio_index}")]);
    }
    match request.kind {
        ConversionKind::Remux => args.extend(["-c".into(), "copy".into()]),
        ConversionKind::ConvertAudio => args.extend([
            "-c:v".into(),
            "copy".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
        ]),
        ConversionKind::TranscodeVideo => args.extend([
            "-c:v".into(),
            "libopenh264".into(),
            "-b:v".into(),
            "5M".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
        ]),
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-f".into(),
        "mp4".into(),
        output.as_os_str().to_string_lossy().into_owned(),
    ]);
    args
}

fn verified_artifact(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn cache_error(error: std::io::Error) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "The playback cache is unavailable. Check free space and cache settings.",
        true,
    )
    .with_diagnostics(error.kind().to_string())
}

pub(crate) struct ExtractionSignals<'a> {
    pub cancelled: &'a Arc<AtomicBool>,
    pub progress_milli: &'a std::sync::atomic::AtomicU32,
}

pub(crate) fn extract_assets(
    ffmpeg_path: &Path,
    runner: &dyn MediaToolPort,
    source_path: &Path,
    cache_dir: &Path,
    duration_us: i64,
    spec: &ExtractionSpec,
    signals: ExtractionSignals<'_>,
) -> Result<ExtractedAssets, AppErrorV1> {
    if spec.start_us < 0
        || spec.end_us <= spec.start_us
        || spec.end_us > duration_us
        || spec.frame_us < 0
        || spec.frame_us >= duration_us
    {
        return Err(AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "Asset extraction timing is outside the media session.",
            false,
        ));
    }
    let key = extraction_key(spec);
    let staging = cache_dir.join("extraction-staging");
    fs::create_dir_all(&staging).map_err(cache_error)?;
    let audio_path = staging.join(format!("{key}.mp3"));
    let image_path = staging.join(format!("{key}.jpg"));
    let audio_partial = staging.join(format!("{key}.audio.part"));
    let image_partial = staging.join(format!("{key}.image.part"));
    let result = (|| {
        if !verified_artifact(&audio_path) {
            let args = vec![
                "-nostdin".into(),
                "-hide_banner".into(),
                "-y".into(),
                "-ss".into(),
                seconds(spec.start_us),
                "-to".into(),
                seconds(spec.end_us),
                "-i".into(),
                source_path.as_os_str().to_string_lossy().into_owned(),
                "-vn".into(),
                "-ac".into(),
                "1".into(),
                "-c:a".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                "128k".into(),
                "-f".into(),
                "mp3".into(),
                audio_partial.as_os_str().to_string_lossy().into_owned(),
            ];
            run_extraction(ffmpeg_path, runner, args, &audio_partial, signals.cancelled)?;
            fs::rename(&audio_partial, &audio_path).map_err(cache_error)?;
        }
        signals
            .progress_milli
            .store(500, std::sync::atomic::Ordering::Release);
        if !verified_artifact(&image_path) {
            let args = vec![
                "-nostdin".into(),
                "-hide_banner".into(),
                "-y".into(),
                "-ss".into(),
                seconds(spec.frame_us),
                "-i".into(),
                source_path.as_os_str().to_string_lossy().into_owned(),
                "-frames:v".into(),
                "1".into(),
                "-q:v".into(),
                "2".into(),
                "-f".into(),
                "image2".into(),
                image_partial.as_os_str().to_string_lossy().into_owned(),
            ];
            run_extraction(ffmpeg_path, runner, args, &image_partial, signals.cancelled)?;
            fs::rename(&image_partial, &image_path).map_err(cache_error)?;
        }
        signals
            .progress_milli
            .store(1_000, std::sync::atomic::Ordering::Release);
        Ok(ExtractedAssets {
            audio_path: audio_path.clone(),
            image_path: image_path.clone(),
            metadata: [
                ("profile".to_owned(), spec.profile.clone()),
                ("source_timeline".to_owned(), "canonical".to_owned()),
            ]
            .into_iter()
            .collect(),
        })
    })();
    if result.is_err() {
        for path in [&audio_path, &image_path, &audio_partial, &image_partial] {
            let _ = fs::remove_file(path);
        }
    }
    result
}

fn run_extraction(
    ffmpeg_path: &Path,
    runner: &dyn MediaToolPort,
    args: Vec<String>,
    expected: &Path,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), AppErrorV1> {
    let output = runner.run_cancellable(
        ToolRequest {
            executable: ffmpeg_path.to_path_buf(),
            args,
            timeout: Duration::from_secs(60),
            max_stdout_bytes: 64 * 1024,
            max_stderr_bytes: 1024 * 1024,
        },
        cancelled,
    );
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(expected);
            return Err(error);
        }
    };
    if output.status_code != Some(0) || !verified_artifact(expected) {
        let _ = fs::remove_file(expected);
        return Err(AppErrorV1::new(
            error_codes::CONVERSION_FAILED,
            "Audio or image extraction failed. No incomplete asset was retained.",
            true,
        ));
    }
    Ok(())
}

fn extraction_key(spec: &ExtractionSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-extraction-v1\0");
    hasher.update(spec.session_id.as_str().as_bytes());
    hasher.update(spec.start_us.to_le_bytes());
    hasher.update(spec.end_us.to_le_bytes());
    hasher.update(spec.frame_us.to_le_bytes());
    hasher.update(spec.profile.as_bytes());
    hex::encode(hasher.finalize())
}

fn seconds(microseconds: i64) -> String {
    format!(
        "{}.{:06}",
        microseconds / 1_000_000,
        microseconds % 1_000_000
    )
}

#[cfg(test)]
mod tests {
    use ports::ToolOutput;
    use tempfile::tempdir;

    use super::*;

    #[derive(Debug)]
    struct VersionRunner(&'static str);

    #[derive(Debug, Default)]
    struct FailSecondExtractionRunner(std::sync::atomic::AtomicUsize);

    impl MediaToolPort for VersionRunner {
        fn run(&self, _request: ToolRequest) -> Result<ToolOutput, AppErrorV1> {
            Ok(ToolOutput {
                status_code: Some(0),
                stdout: format!("ffmpeg version {}\n", self.0).into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    impl MediaToolPort for FailSecondExtractionRunner {
        fn run(&self, request: ToolRequest) -> Result<ToolOutput, AppErrorV1> {
            let call = self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if call == 0 {
                let Some(output) = request.args.last() else {
                    return Err(AppErrorV1::new(
                        error_codes::INVALID_REQUEST,
                        "Test extraction command omitted its output.",
                        false,
                    ));
                };
                fs::write(output, b"valid-looking-audio").map_err(cache_error)?;
                return Ok(ToolOutput {
                    status_code: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
            Ok(ToolOutput {
                status_code: Some(1),
                stdout: Vec::new(),
                stderr: b"simulated frame failure".to_vec(),
            })
        }
    }

    #[test]
    fn cache_key_changes_with_profile_and_stream() {
        let mut request = ConversionRequest {
            source_path: "source.mkv".into(),
            source_fingerprint: "fingerprint".into(),
            selected_streams: vec![StreamId::new("v0")],
            video_stream_index: 0,
            audio_stream_index: Some(1),
            kind: ConversionKind::Remux,
            duration_us: 10_000_000,
            approved_video_transcode: false,
        };
        let first = conversion_cache_key(&request, "ffmpeg-v1");
        request.selected_streams.push(StreamId::new("a1"));
        assert_ne!(first, conversion_cache_key(&request, "ffmpeg-v1"));
        assert_ne!(first, conversion_cache_key(&request, "ffmpeg-v2"));
    }

    #[test]
    fn conversion_maps_the_exact_selected_stream_indices() {
        let request = ConversionRequest {
            source_path: "source.mkv".into(),
            source_fingerprint: "fingerprint".into(),
            selected_streams: vec![StreamId::new("video"), StreamId::new("audio")],
            video_stream_index: 3,
            audio_stream_index: Some(7),
            kind: ConversionKind::Remux,
            duration_us: 10_000_000,
            approved_video_transcode: false,
        };
        let args = conversion_args(&request, Path::new("out.mp4"));
        assert!(args.windows(2).any(|pair| pair == ["-map", "0:3"]));
        assert!(args.windows(2).any(|pair| pair == ["-map", "0:7"]));
        assert!(!args.iter().any(|arg| arg.contains("shell")));
    }

    #[test]
    fn tool_profile_includes_the_reported_ffmpeg_version() -> Result<(), AppErrorV1> {
        let temporary = tempdir().map_err(cache_error)?;
        let executable = temporary.path().join("ffmpeg.exe");
        fs::write(&executable, b"placeholder").map_err(cache_error)?;
        assert_ne!(
            tool_profile(&executable, &VersionRunner("one")),
            tool_profile(&executable, &VersionRunner("two"))
        );
        Ok(())
    }

    #[test]
    fn cache_pruning_enforces_budget_and_protects_active_artifacts() -> Result<(), AppErrorV1> {
        let temporary = tempdir().map_err(cache_error)?;
        let executable = temporary.path().join("ffmpeg.exe");
        fs::write(&executable, b"placeholder").map_err(cache_error)?;
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).map_err(cache_error)?;
        let removable = cache.join("removable.mp4");
        let protected = cache.join("protected.mp4");
        let partial = cache.join("abandoned.part");
        let orphan_marker = cache.join("orphan.access");
        for path in [&removable, &protected, &partial, &orphan_marker] {
            fs::write(path, b"1234").map_err(cache_error)?;
        }
        let service = ConversionService::new_with_cache_policy(
            executable,
            cache,
            Arc::new(VersionRunner("test")),
            CachePolicy {
                max_bytes: 4,
                max_age: Duration::from_secs(86_400),
            },
        );
        let report = service.prune_cache(&HashSet::from([protected.clone()]))?;
        assert!(!removable.exists());
        assert!(protected.exists());
        assert!(!partial.exists());
        assert!(!orphan_marker.exists());
        assert_eq!(report.bytes_remaining, 4);
        Ok(())
    }

    #[test]
    fn failed_second_extraction_removes_every_staging_artifact() -> Result<(), AppErrorV1> {
        let temporary = tempdir().map_err(cache_error)?;
        let cache = temporary.path().join("cache");
        let spec = ExtractionSpec {
            session_id: contracts::MediaSessionId::new("session"),
            start_us: 0,
            end_us: 1_000_000,
            frame_us: 500_000,
            profile: "test".into(),
        };
        let result = extract_assets(
            Path::new("ffmpeg"),
            &FailSecondExtractionRunner::default(),
            Path::new("source.mp4"),
            &cache,
            2_000_000,
            &spec,
            ExtractionSignals {
                cancelled: &Arc::new(AtomicBool::new(false)),
                progress_milli: &std::sync::atomic::AtomicU32::new(0),
            },
        );
        let Err(error) = result else {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The simulated second extraction unexpectedly succeeded.",
                false,
            ));
        };
        assert_eq!(error.code, error_codes::CONVERSION_FAILED);
        let staging = cache.join("extraction-staging");
        assert_eq!(
            fs::read_dir(staging).map_err(cache_error)?.count(),
            0,
            "no successful first output or partial second output may remain"
        );
        Ok(())
    }
}
