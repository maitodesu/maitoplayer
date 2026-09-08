//! Versioned DTOs shared across the privileged backend and the webview.
//!
//! Paths and process arguments are deliberately absent from public requests.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

opaque_id!(MediaSessionId);
opaque_id!(StreamId);
opaque_id!(SubtitleSourceId);
opaque_id!(CueId);
opaque_id!(TokenId);
opaque_id!(DictionaryEntryId);
opaque_id!(DraftId);
opaque_id!(JobId);

pub type TimestampUs = i64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DimensionsV1 {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStreamKindV1 {
    Video,
    Audio,
    Subtitle,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleKindV1 {
    Text,
    Image,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MediaStreamV1 {
    pub stream_id: StreamId,
    pub index: u32,
    pub kind: MediaStreamKindV1,
    pub codec: String,
    pub codec_profile: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_format: Option<String>,
    pub frame_rate: Option<f64>,
    pub subtitle_kind: Option<SubtitleKindV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackPlanKindV1 {
    Direct,
    Remux,
    ConvertAudio,
    TranscodeVideo,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStateV1 {
    Probing,
    AwaitingApproval,
    Queued,
    Running,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimatedCostV1 {
    Negligible,
    Low,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaybackTimelineMapV1 {
    pub source_origin_us: TimestampUs,
    pub playback_origin_us: TimestampUs,
    pub source_offset_us: TimestampUs,
    pub source_duration_us: TimestampUs,
    pub playback_duration_us: TimestampUs,
}

impl PlaybackTimelineMapV1 {
    #[must_use]
    pub fn to_source_time(&self, playback_time_us: TimestampUs) -> Option<TimestampUs> {
        let playback_delta = playback_time_us.checked_sub(self.playback_origin_us)?;
        if !(0..=self.playback_duration_us).contains(&playback_delta) {
            return None;
        }
        let mapped = self.source_origin_us.checked_add(playback_delta)?;
        let offset_mapped = playback_time_us.checked_add(self.source_offset_us)?;
        if mapped != offset_mapped {
            return None;
        }
        let source_end = self.source_origin_us.checked_add(self.source_duration_us)?;
        (self.source_origin_us..=source_end)
            .contains(&mapped)
            .then_some(mapped)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlaybackPlanV1 {
    pub kind: PlaybackPlanKindV1,
    pub state: JobStateV1,
    pub source_stream_ids: Vec<StreamId>,
    pub output_container: Option<String>,
    pub output_codecs: Vec<String>,
    pub timeline_map: PlaybackTimelineMapV1,
    pub estimated_cost: EstimatedCostV1,
    pub cache_key: Option<String>,
    pub progress: Option<f32>,
    pub reason_codes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScopePersistenceV1 {
    Session,
    UserApprovedPersistent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MediaSessionV1 {
    pub session_id: MediaSessionId,
    pub display_name: String,
    pub source_fingerprint: String,
    pub duration_us: TimestampUs,
    pub dimensions: Option<DimensionsV1>,
    pub container: String,
    pub video_streams: Vec<MediaStreamV1>,
    pub audio_streams: Vec<MediaStreamV1>,
    pub subtitle_streams: Vec<MediaStreamV1>,
    pub selected_video_stream_id: Option<StreamId>,
    pub selected_audio_stream_id: Option<StreamId>,
    pub playback_plan: PlaybackPlanV1,
    pub playback_url: String,
    pub source_scope_persistence: SourceScopePersistenceV1,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStateV1 {
    Loading,
    Playing,
    Paused,
    Seeking,
    Ended,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaybackCheckpointV1 {
    pub session_id: MediaSessionId,
    pub revision: u64,
    pub state: PlaybackStateV1,
    pub position_us: TimestampUs,
    pub playback_rate_milli: u32,
    pub selected_audio_stream_id: Option<StreamId>,
    pub observed_monotonic_us: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityCandidateV1 {
    pub container: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub can_play: String,
    pub media_capabilities_supported: Option<bool>,
    pub media_capabilities_smooth: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaybackCapabilityReportV1 {
    pub revision: u64,
    pub candidates: Vec<CapabilityCandidateV1>,
    pub tested_webview: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeTestOutcomeV1 {
    NotTested,
    FirstFramePresented,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleOriginV1 {
    AdjacentFile,
    SelectedFile,
    EmbeddedTextTrack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleFormatV1 {
    Srt,
    Ass,
    Ssa,
    Vtt,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleSourceV1 {
    pub subtitle_source_id: SubtitleSourceId,
    pub origin: SubtitleOriginV1,
    pub display_name: String,
    pub format: SubtitleFormatV1,
    pub language: Option<String>,
    pub source_version: String,
    pub embedded_stream_id: Option<StreamId>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleStyleHintV1 {
    pub alignment: Option<u8>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleCueV1 {
    pub cue_id: CueId,
    pub start_us: TimestampUs,
    pub end_us: TimestampUs,
    pub plain_text: String,
    pub source_text: String,
    pub track_order: u32,
    pub style_hint: SubtitleStyleHintV1,
}

impl SubtitleCueV1 {
    #[must_use]
    pub fn is_active_at(&self, position_us: TimestampUs) -> bool {
        self.start_us <= position_us && position_us < self.end_us
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenV1 {
    pub token_id: TokenId,
    pub surface: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub lemma: String,
    pub reading: String,
    pub pronunciation: Option<String>,
    pub part_of_speech: Vec<String>,
    pub lookup_candidate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DictionarySenseV1 {
    pub glosses: Vec<String>,
    pub parts_of_speech: Vec<String>,
    pub restrictions: Vec<String>,
    pub fields: Vec<String>,
    pub dialects: Vec<String>,
    pub misc: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PitchLevelV1 {
    Low,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PitchAccentV1 {
    pub reading: String,
    pub morae: Vec<String>,
    pub levels: Vec<PitchLevelV1>,
    /// One-based mora after which the pitch drops. `None` is a heiban pattern.
    pub drop_after_mora: Option<u32>,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DictionaryEntrySummaryV1 {
    pub entry_id: DictionaryEntryId,
    pub headwords: Vec<String>,
    pub readings: Vec<String>,
    pub senses: Vec<DictionarySenseV1>,
    #[serde(default)]
    pub pitch_accents: Vec<PitchAccentV1>,
    /// An unofficial N1-N5 community estimate. The JLPT does not publish a
    /// current vocabulary list.
    #[serde(default)]
    pub jlpt_level: Option<u8>,
    #[serde(default)]
    pub jlpt_source: Option<String>,
    pub match_reason: String,
    pub priority_score: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnalyzedTokenV1 {
    pub token: TokenV1,
    pub dictionary: Vec<DictionaryEntrySummaryV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnalyzedCueV1 {
    pub cue: SubtitleCueV1,
    pub tokens: Vec<AnalyzedTokenV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStateV1 {
    Pending,
    Partial,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleWindowV1 {
    pub window_id: String,
    pub revision: u64,
    pub session_id: MediaSessionId,
    pub subtitle_source_id: SubtitleSourceId,
    pub window_start_us: TimestampUs,
    pub window_end_us: TimestampUs,
    pub cues: Vec<AnalyzedCueV1>,
    pub recommended_refresh_at_us: TimestampUs,
    pub analysis_state: AnalysisStateV1,
    pub warnings: Vec<String>,
}

/// A bounded, raw subtitle window for a non-interactive translation track.
///
/// Unlike `SubtitleWindowV1`, these cues deliberately skip Japanese tokenization
/// and dictionary lookup. The shared media clock remains the only source of
/// truth for deciding which translation cues are visible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranslationWindowV1 {
    pub window_id: String,
    pub revision: u64,
    pub session_id: MediaSessionId,
    pub subtitle_source_id: SubtitleSourceId,
    pub window_start_us: TimestampUs,
    pub window_end_us: TimestampUs,
    pub cues: Vec<SubtitleCueV1>,
    pub recommended_refresh_at_us: TimestampUs,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardDraftV1 {
    pub draft_id: DraftId,
    pub revision: u64,
    pub session_id: MediaSessionId,
    pub subtitle_source_id: SubtitleSourceId,
    pub subtitle_source_version: String,
    pub cue: SubtitleCueV1,
    pub token: TokenV1,
    pub dictionary_entry: Option<DictionaryEntrySummaryV1>,
    pub observed_source_time_us: TimestampUs,
    pub source_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateCardRequestV1 {
    pub draft_id: DraftId,
    pub expected_draft_revision: u64,
    pub profile_id: String,
    pub editable_fields: std::collections::BTreeMap<String, String>,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateCardOutcomeV1 {
    Created,
    AlreadyExists,
    FailedRetryable,
    FailedTerminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateCardResultV1 {
    pub job_id: JobId,
    pub note_id: Option<i64>,
    pub media_names: Vec<String>,
    pub outcome: CreateCardOutcomeV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppErrorV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub diagnostics: Option<String>,
}

impl AppErrorV1 {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            diagnostics: None,
        }
    }

    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: impl Into<String>) -> Self {
        self.diagnostics = Some(diagnostics.into());
        self
    }
}

impl fmt::Display for AppErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppErrorV1 {}

pub mod error_codes {
    pub const MEDIA_SCOPE_DENIED: &str = "MEDIA_SCOPE_DENIED";
    pub const MEDIA_PROBE_FAILED: &str = "MEDIA_PROBE_FAILED";
    pub const MEDIA_UNSUPPORTED: &str = "MEDIA_UNSUPPORTED";
    pub const PLAYBACK_DECODE_FAILED: &str = "PLAYBACK_DECODE_FAILED";
    pub const CONVERSION_REQUIRED: &str = "CONVERSION_REQUIRED";
    pub const CONVERSION_FAILED: &str = "CONVERSION_FAILED";
    pub const CONVERSION_CANCELLED: &str = "CONVERSION_CANCELLED";
    pub const SUBTITLE_AMBIGUOUS: &str = "SUBTITLE_AMBIGUOUS";
    pub const SUBTITLE_IMAGE_UNSUPPORTED: &str = "SUBTITLE_IMAGE_UNSUPPORTED";
    pub const DICTIONARY_UNAVAILABLE: &str = "DICTIONARY_UNAVAILABLE";
    pub const FFMPEG_NOT_FOUND: &str = "FFMPEG_NOT_FOUND";
    pub const ANKI_OFFLINE: &str = "ANKI_OFFLINE";
    pub const ANKI_SCHEMA_MISMATCH: &str = "ANKI_SCHEMA_MISMATCH";
    pub const PUBLISH_OUTCOME_UNCERTAIN: &str = "PUBLISH_OUTCOME_UNCERTAIN";
    pub const INVALID_REQUEST: &str = "INVALID_REQUEST";
    pub const STALE_REVISION: &str = "STALE_REVISION";
    pub const STORAGE_FAILED: &str = "STORAGE_FAILED";
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DependencyHealthV1 {
    pub component: String,
    pub available: bool,
    pub version: Option<String>,
    pub action: Option<String>,
    pub error: Option<AppErrorV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppHealthV1 {
    pub checks: Vec<DependencyHealthV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardProfileSettingsV1 {
    pub profile_id: String,
    pub deck_name: String,
    pub model_name: String,
    pub field_mapping: std::collections::BTreeMap<String, String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserSettingsV1 {
    pub preferred_audio_languages: Vec<String>,
    pub playback_cache_max_bytes: u64,
    pub playback_cache_max_age_days: u32,
    pub clip_leading_padding_us: TimestampUs,
    pub clip_trailing_padding_us: TimestampUs,
    pub anki_port: u16,
    pub anki_timeout_ms: u32,
    pub card_profile: CardProfileSettingsV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSelectionResultV1 {
    pub component: String,
    pub configured: bool,
    pub restart_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleCandidateV1 {
    pub candidate_id: String,
    pub display_name: String,
    pub format: SubtitleFormatV1,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtitleDiscoveryV1 {
    pub candidates: Vec<SubtitleCandidateV1>,
    pub recommended_candidate_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecentMediaV1 {
    pub source_fingerprint: String,
    pub display_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_map_checks_overflow_and_bounds() {
        let map = PlaybackTimelineMapV1 {
            source_origin_us: 2_000,
            playback_origin_us: 0,
            source_offset_us: 2_000,
            source_duration_us: 10_000,
            playback_duration_us: 8_000,
        };
        assert_eq!(map.to_source_time(1_500), Some(3_500));
        assert_eq!(map.to_source_time(9_000), None);
        assert_eq!(map.to_source_time(i64::MAX), None);
        let mut inconsistent = map;
        inconsistent.source_offset_us = 0;
        assert_eq!(inconsistent.to_source_time(1_500), None);
    }

    #[test]
    fn cue_interval_is_half_open() {
        let cue = SubtitleCueV1 {
            cue_id: CueId::new("cue"),
            start_us: 10,
            end_us: 20,
            plain_text: "日本語".into(),
            source_text: "日本語".into(),
            track_order: 0,
            style_hint: SubtitleStyleHintV1::default(),
        };
        assert!(cue.is_active_at(10));
        assert!(!cue.is_active_at(20));
    }

    #[test]
    fn error_serialization_is_stable() -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(AppErrorV1::new(
            error_codes::ANKI_OFFLINE,
            "Start Anki and retry.",
            true,
        ))?;
        assert_eq!(value["code"], "ANKI_OFFLINE");
        assert_eq!(value["retryable"], true);
        assert!(value["diagnostics"].is_null());
        Ok(())
    }
}
