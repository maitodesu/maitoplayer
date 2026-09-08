use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use contracts::{
    AppErrorV1, DecodeTestOutcomeV1, EstimatedCostV1, JobStateV1, MediaSessionId, MediaSessionV1,
    MediaStreamKindV1, MediaStreamV1, PlaybackCapabilityReportV1, PlaybackCheckpointV1,
    PlaybackPlanKindV1, PlaybackPlanV1, PlaybackTimelineMapV1, SourceScopePersistenceV1, StreamId,
    error_codes,
};
use parking_lot::RwLock;

use crate::{
    compatibility::{plan_playback, select_default_stream},
    conversion::{
        ConversionArtifact, ConversionKind, ConversionRequest, remove_artifact_and_marker,
    },
    import::import_authorized_path,
    probe::{AssetProbe, ProbeInventory, ProbeService},
};

pub const MAX_PROTOCOL_CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
struct ActiveSession {
    source_path: PathBuf,
    source_size_bytes: u64,
    playback_path: PathBuf,
    playback_size_bytes: u64,
    inventory: ProbeInventory,
    public: MediaSessionV1,
    capability_revision: u64,
    checkpoint_revision: u64,
    capability_report: Option<PlaybackCapabilityReportV1>,
    decode_outcome: DecodeTestOutcomeV1,
}

#[derive(Debug)]
pub struct MediaSlice {
    pub bytes: Vec<u8>,
    pub start: u64,
    pub end_inclusive: u64,
    pub total_size: u64,
    pub content_type: &'static str,
}

#[derive(Debug)]
pub struct SessionRegistry {
    probe: ProbeService,
    cache_dir: PathBuf,
    audio_language_priority: RwLock<Vec<String>>,
    sessions: RwLock<HashMap<MediaSessionId, ActiveSession>>,
}

impl SessionRegistry {
    #[must_use]
    pub fn new(probe: ProbeService, cache_dir: PathBuf) -> Self {
        Self {
            probe,
            cache_dir,
            audio_language_priority: RwLock::new(vec!["jpn".into(), "ja".into(), "und".into()]),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_audio_language_priority(&self, priority: Vec<String>) -> Result<(), AppErrorV1> {
        if priority.is_empty()
            || priority.len() > 16
            || priority.iter().any(|language| {
                language.is_empty()
                    || language.len() > 16
                    || !language
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Preferred audio languages were invalid.",
                false,
            ));
        }
        *self.audio_language_priority.write() = priority;
        Ok(())
    }

    pub fn import(&self, authorized_path: PathBuf) -> Result<MediaSessionV1, AppErrorV1> {
        let imported = import_authorized_path(&authorized_path)?;
        let inventory = self
            .probe
            .probe(&imported.canonical_path, &imported.source_fingerprint)?;
        let video_streams = inventory
            .streams
            .iter()
            .filter(|stream| stream.kind == MediaStreamKindV1::Video)
            .cloned()
            .collect();
        let audio_streams = inventory
            .streams
            .iter()
            .filter(|stream| stream.kind == MediaStreamKindV1::Audio)
            .cloned()
            .collect();
        let subtitle_streams = inventory
            .streams
            .iter()
            .filter(|stream| stream.kind == MediaStreamKindV1::Subtitle)
            .cloned()
            .collect();
        let selected_video_stream_id =
            select_default_stream(&inventory.streams, MediaStreamKindV1::Video, &[]);
        let selected_audio_stream_id = select_default_stream(
            &inventory.streams,
            MediaStreamKindV1::Audio,
            &self.audio_language_priority.read(),
        );
        let initial_plan = PlaybackPlanV1 {
            kind: PlaybackPlanKindV1::Direct,
            state: JobStateV1::Probing,
            source_stream_ids: selected_video_stream_id
                .iter()
                .chain(selected_audio_stream_id.iter())
                .cloned()
                .collect(),
            output_container: Some(inventory.container.clone()),
            output_codecs: Vec::new(),
            timeline_map: PlaybackTimelineMapV1 {
                source_origin_us: inventory.source_origin_us,
                playback_origin_us: 0,
                source_offset_us: inventory.source_origin_us,
                source_duration_us: inventory.duration_us,
                playback_duration_us: inventory.duration_us,
            },
            estimated_cost: EstimatedCostV1::Negligible,
            cache_key: None,
            progress: None,
            reason_codes: vec!["CAPABILITY_REPORT_REQUIRED".into()],
        };
        let playback_url = playback_url(&imported.session_id, None);
        let public = MediaSessionV1 {
            session_id: imported.session_id.clone(),
            display_name: imported.display_name,
            source_fingerprint: imported.source_fingerprint,
            duration_us: inventory.duration_us,
            dimensions: inventory.dimensions.clone(),
            container: inventory.container.clone(),
            video_streams,
            audio_streams,
            subtitle_streams,
            selected_video_stream_id,
            selected_audio_stream_id,
            playback_plan: initial_plan,
            playback_url,
            source_scope_persistence: SourceScopePersistenceV1::Session,
            warnings: inventory.warnings.clone(),
        };
        self.sessions.write().insert(
            imported.session_id,
            ActiveSession {
                source_path: imported.canonical_path.clone(),
                source_size_bytes: imported.size_bytes,
                playback_path: imported.canonical_path,
                playback_size_bytes: imported.size_bytes,
                inventory,
                public: public.clone(),
                capability_revision: 0,
                checkpoint_revision: 0,
                capability_report: None,
                decode_outcome: DecodeTestOutcomeV1::NotTested,
            },
        );
        Ok(public)
    }

    pub fn apply_capabilities(
        &self,
        session_id: &MediaSessionId,
        report: &PlaybackCapabilityReportV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        if report.revision <= session.capability_revision {
            return Ok(session.public.clone());
        }
        session.capability_revision = report.revision;
        session.capability_report = Some(report.clone());
        session.public.playback_plan = plan_playback(
            &session.inventory,
            report,
            session.decode_outcome,
            session.public.selected_video_stream_id.as_ref(),
            session.public.selected_audio_stream_id.as_ref(),
        );
        Ok(session.public.clone())
    }

    pub fn report_decode_outcome(
        &self,
        session_id: &MediaSessionId,
        outcome: DecodeTestOutcomeV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        session.decode_outcome = outcome;
        if session.public.playback_plan.cache_key.is_some() {
            if outcome == DecodeTestOutcomeV1::FirstFramePresented {
                session.public.playback_plan.state = JobStateV1::Ready;
                return Ok(session.public.clone());
            }
            if outcome == DecodeTestOutcomeV1::Failed {
                let failed_artifact = session.playback_path.clone();
                session.public.playback_plan.kind = PlaybackPlanKindV1::TranscodeVideo;
                session.public.playback_plan.state = JobStateV1::AwaitingApproval;
                session.public.playback_plan.estimated_cost = EstimatedCostV1::High;
                session.public.playback_plan.cache_key = None;
                session.public.playback_plan.progress = None;
                session.public.playback_plan.output_container = Some("mp4".into());
                session.public.playback_plan.output_codecs = vec!["h264".into(), "aac".into()];
                session.public.playback_plan.reason_codes =
                    vec!["CONVERTED_ARTIFACT_DECODE_FAILED".into()];
                session.playback_path = session.source_path.clone();
                session.playback_size_bytes = session.source_size_bytes;
                session.public.playback_url = playback_url(session_id, None);
                if failed_artifact != session.source_path {
                    remove_artifact_and_marker(&failed_artifact);
                }
                return Ok(session.public.clone());
            }
        }
        if let Some(report) = &session.capability_report {
            session.public.playback_plan = plan_playback(
                &session.inventory,
                report,
                outcome,
                session.public.selected_video_stream_id.as_ref(),
                session.public.selected_audio_stream_id.as_ref(),
            );
        }
        Ok(session.public.clone())
    }

    pub(crate) fn begin_conversion(
        &self,
        session_id: &MediaSessionId,
        approved_video_transcode: bool,
    ) -> Result<ConversionRequest, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        let kind = match session.public.playback_plan.kind {
            PlaybackPlanKindV1::Remux => ConversionKind::Remux,
            PlaybackPlanKindV1::ConvertAudio => ConversionKind::ConvertAudio,
            PlaybackPlanKindV1::TranscodeVideo if approved_video_transcode => {
                ConversionKind::TranscodeVideo
            }
            PlaybackPlanKindV1::TranscodeVideo => {
                return Err(AppErrorV1::new(
                    error_codes::CONVERSION_REQUIRED,
                    "Video conversion requires explicit approval.",
                    true,
                ));
            }
            PlaybackPlanKindV1::Direct
                if session.public.playback_plan.state == JobStateV1::Ready =>
            {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "This media session is already ready for direct playback.",
                    false,
                ));
            }
            PlaybackPlanKindV1::Unsupported => {
                return Err(AppErrorV1::new(
                    error_codes::MEDIA_UNSUPPORTED,
                    "This media has no supported playback conversion path.",
                    false,
                ));
            }
            _ => {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "Playback planning must finish before conversion starts.",
                    true,
                ));
            }
        };
        if session.public.playback_plan.state == JobStateV1::Running {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "A playback conversion is already running.",
                true,
            ));
        }
        let video_stream = selected_stream(
            session,
            session.public.selected_video_stream_id.as_ref(),
            MediaStreamKindV1::Video,
        )?;
        let audio_stream = selected_stream(
            session,
            session.public.selected_audio_stream_id.as_ref(),
            MediaStreamKindV1::Audio,
        )
        .ok();
        let mut selected_streams = vec![video_stream.stream_id.clone()];
        if let Some(audio) = audio_stream.as_ref() {
            selected_streams.push(audio.stream_id.clone());
        }
        session.public.playback_plan.state = JobStateV1::Running;
        session.public.playback_plan.progress = Some(0.0);
        Ok(ConversionRequest {
            source_path: session.source_path.clone(),
            source_fingerprint: session.public.source_fingerprint.clone(),
            selected_streams,
            video_stream_index: video_stream.index,
            audio_stream_index: audio_stream.map(|stream| stream.index),
            kind,
            duration_us: session.public.duration_us,
            approved_video_transcode,
        })
    }

    pub(crate) fn complete_conversion(
        &self,
        session_id: &MediaSessionId,
        artifact: &ConversionArtifact,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let verified = match self.probe.probe(&artifact.path, &artifact.cache_key) {
            Ok(verified) => verified,
            Err(error) => {
                remove_artifact_and_marker(&artifact.path);
                return Err(error);
            }
        };
        if verified.container != "mp4"
            || verified.container_start_us.abs() > 1_000
            || !verified
                .streams
                .iter()
                .any(|stream| stream.kind == MediaStreamKindV1::Video)
        {
            remove_artifact_and_marker(&artifact.path);
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_FAILED,
                "The converted playback artifact failed validation.",
                true,
            ));
        }
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        if verified.duration_us.abs_diff(session.public.duration_us) > 1_000_000 {
            remove_artifact_and_marker(&artifact.path);
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_FAILED,
                "The converted playback timeline did not match the source.",
                true,
            ));
        }
        let metadata = artifact.path.metadata().map_err(media_read_error)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(AppErrorV1::new(
                error_codes::CONVERSION_FAILED,
                "The converted playback artifact was empty or unavailable.",
                true,
            ));
        }
        session.playback_path = artifact.path.clone();
        session.playback_size_bytes = metadata.len();
        session.public.playback_plan.state = JobStateV1::Ready;
        session.public.playback_plan.cache_key = Some(artifact.cache_key.clone());
        session.public.playback_plan.progress = Some(1.0);
        session.public.playback_url = playback_url(session_id, Some(&artifact.cache_key));
        Ok(session.public.clone())
    }

    pub(crate) fn fail_conversion(
        &self,
        session_id: &MediaSessionId,
        cancelled: bool,
    ) -> Result<(), AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        session.public.playback_plan.state = if cancelled {
            JobStateV1::Cancelled
        } else {
            JobStateV1::Failed
        };
        session.public.playback_plan.progress = None;
        Ok(())
    }

    pub fn select_audio_stream(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        selected_stream(session, Some(stream_id), MediaStreamKindV1::Audio)?;
        if session.public.selected_audio_stream_id.as_ref() == Some(stream_id) {
            return Ok(session.public.clone());
        }
        session.public.selected_audio_stream_id = Some(stream_id.clone());
        session.playback_path = session.source_path.clone();
        session.playback_size_bytes = session.source_size_bytes;
        session.decode_outcome = DecodeTestOutcomeV1::NotTested;
        if let Some(report) = &session.capability_report {
            let mut plan = plan_playback(
                &session.inventory,
                report,
                DecodeTestOutcomeV1::NotTested,
                session.public.selected_video_stream_id.as_ref(),
                Some(stream_id),
            );
            if plan.kind == PlaybackPlanKindV1::Direct {
                plan.kind = PlaybackPlanKindV1::Remux;
                plan.state = JobStateV1::Queued;
                plan.estimated_cost = EstimatedCostV1::Low;
                plan.output_container = Some("mp4".into());
                plan.reason_codes = vec!["AUDIO_STREAM_SWITCH_REQUIRES_REMUX".into()];
            }
            session.public.playback_plan = plan;
        }
        session.public.playback_url = playback_url(session_id, None);
        Ok(session.public.clone())
    }

    pub fn checkpoint(&self, checkpoint: PlaybackCheckpointV1) -> Result<bool, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(&checkpoint.session_id)
            .ok_or_else(session_missing)?;
        if checkpoint.position_us < 0
            || checkpoint.position_us > session.public.duration_us + 1_000_000
        {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Playback position is outside this media session.",
                false,
            ));
        }
        if !(250..=4_000).contains(&checkpoint.playback_rate_milli) {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Playback rate is outside supported bounds.",
                false,
            ));
        }
        if checkpoint.revision <= session.checkpoint_revision {
            return Ok(false);
        }
        session.checkpoint_revision = checkpoint.revision;
        Ok(true)
    }

    pub fn read_range(
        &self,
        session_id: &MediaSessionId,
        requested_start: u64,
        requested_end_inclusive: Option<u64>,
    ) -> Result<MediaSlice, AppErrorV1> {
        let (playback_path, playback_size_bytes) = {
            let sessions = self.sessions.read();
            let session = sessions.get(session_id).ok_or_else(session_missing)?;
            (session.playback_path.clone(), session.playback_size_bytes)
        };
        if requested_start >= playback_size_bytes {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Requested media range is outside the session.",
                false,
            ));
        }
        let max_end = requested_start
            .saturating_add(MAX_PROTOCOL_CHUNK_BYTES as u64 - 1)
            .min(playback_size_bytes - 1);
        let end = requested_end_inclusive.unwrap_or(max_end).min(max_end);
        if end < requested_start {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Requested media range is invalid.",
                false,
            ));
        }
        let length = usize::try_from(end - requested_start + 1).map_err(|_| session_missing())?;
        let mut file = File::open(&playback_path).map_err(media_read_error)?;
        file.seek(SeekFrom::Start(requested_start))
            .map_err(media_read_error)?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes).map_err(media_read_error)?;
        Ok(MediaSlice {
            bytes,
            start: requested_start,
            end_inclusive: end,
            total_size: playback_size_bytes,
            content_type: mime_for_path(&playback_path),
        })
    }

    pub fn playback_size(&self, session_id: &MediaSessionId) -> Result<u64, AppErrorV1> {
        self.sessions
            .read()
            .get(session_id)
            .map(|session| session.playback_size_bytes)
            .ok_or_else(session_missing)
    }

    pub fn close(&self, session_id: &MediaSessionId) -> Result<(), AppErrorV1> {
        self.sessions
            .write()
            .remove(session_id)
            .map(|_| ())
            .ok_or_else(session_missing)
    }

    pub(crate) fn source_context(
        &self,
        session_id: &MediaSessionId,
    ) -> Result<(PathBuf, i64), AppErrorV1> {
        let sessions = self.sessions.read();
        let session = sessions.get(session_id).ok_or_else(session_missing)?;
        Ok((session.source_path.clone(), session.public.duration_us))
    }

    pub(crate) fn source_identity_context(
        &self,
        session_id: &MediaSessionId,
    ) -> Result<(PathBuf, String, String), AppErrorV1> {
        let sessions = self.sessions.read();
        let session = sessions.get(session_id).ok_or_else(session_missing)?;
        Ok((
            session.source_path.clone(),
            session.public.source_fingerprint.clone(),
            session.public.display_name.clone(),
        ))
    }

    pub(crate) fn probe_asset(&self, path: &Path) -> Result<AssetProbe, AppErrorV1> {
        self.probe.probe_asset(path)
    }

    pub(crate) fn mark_persistent(
        &self,
        session_id: &MediaSessionId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or_else(session_missing)?;
        session.public.source_scope_persistence = SourceScopePersistenceV1::UserApprovedPersistent;
        Ok(session.public.clone())
    }

    pub(crate) fn embedded_subtitle_context(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<(PathBuf, String, MediaStreamV1), AppErrorV1> {
        let sessions = self.sessions.read();
        let session = sessions.get(session_id).ok_or_else(session_missing)?;
        let stream = selected_stream(session, Some(stream_id), MediaStreamKindV1::Subtitle)?;
        if stream.subtitle_kind != Some(contracts::SubtitleKindV1::Text) {
            return Err(AppErrorV1::new(
                error_codes::SUBTITLE_IMAGE_UNSUPPORTED,
                "Image subtitles are not interactive in this release. Choose a text subtitle track.",
                false,
            ));
        }
        Ok((
            session.source_path.clone(),
            session.public.source_fingerprint.clone(),
            stream,
        ))
    }

    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    #[must_use]
    pub fn protected_cache_paths(&self) -> HashSet<PathBuf> {
        self.sessions
            .read()
            .values()
            .filter(|session| session.playback_path.starts_with(&self.cache_dir))
            .map(|session| session.playback_path.clone())
            .collect()
    }
}

fn selected_stream(
    session: &ActiveSession,
    selected_id: Option<&StreamId>,
    kind: MediaStreamKindV1,
) -> Result<MediaStreamV1, AppErrorV1> {
    let selected_id = selected_id.ok_or_else(|| {
        AppErrorV1::new(
            error_codes::MEDIA_UNSUPPORTED,
            "The requested media stream is unavailable.",
            false,
        )
    })?;
    session
        .inventory
        .streams
        .iter()
        .find(|stream| stream.kind == kind && &stream.stream_id == selected_id)
        .cloned()
        .ok_or_else(|| {
            AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The requested media stream does not belong to this session.",
                false,
            )
        })
}

fn playback_url(session_id: &MediaSessionId, cache_key: Option<&str>) -> String {
    let base = if cfg!(windows) {
        format!("http://migaku-media.localhost/session/{session_id}")
    } else {
        format!("migaku-media://localhost/session/{session_id}")
    };
    cache_key.map_or(base.clone(), |key| format!("{base}?v={key}"))
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn session_missing() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "This media session is closed or unavailable. Import the file again.",
        true,
    )
}

fn media_read_error(error: std::io::Error) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "The media source can no longer be read. Locate it again.",
        true,
    )
    .with_diagnostics(error.kind().to_string())
}
