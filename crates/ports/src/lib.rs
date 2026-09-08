//! Domain ports. Implementations remain behind the application composition root.

use std::sync::{Arc, atomic::AtomicBool};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use contracts::{
    AppErrorV1, AppHealthV1, CardDraftV1, CreateCardRequestV1, CreateCardResultV1,
    DecodeTestOutcomeV1, DictionaryEntrySummaryV1, MediaSessionId, MediaSessionV1,
    PlaybackCapabilityReportV1, PlaybackCheckpointV1, StreamId, SubtitleSourceId, SubtitleWindowV1,
    TimestampUs, TokenV1,
};

#[derive(Clone, Debug)]
pub struct ToolRequest {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutput {
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub type ToolProgressCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

pub trait MediaToolPort: Send + Sync {
    fn run(&self, request: ToolRequest) -> Result<ToolOutput, AppErrorV1>;

    fn run_cancellable(
        &self,
        request: ToolRequest,
        _cancelled: &Arc<AtomicBool>,
    ) -> Result<ToolOutput, AppErrorV1> {
        self.run(request)
    }

    fn run_cancellable_with_progress(
        &self,
        request: ToolRequest,
        cancelled: &Arc<AtomicBool>,
        _progress: ToolProgressCallback,
    ) -> Result<ToolOutput, AppErrorV1> {
        self.run_cancellable(request, cancelled)
    }
}

pub trait MediaSessionPort: Send + Sync {
    fn import_path(&self, authorized_path: PathBuf) -> Result<MediaSessionV1, AppErrorV1>;
    fn apply_capabilities(
        &self,
        session_id: &MediaSessionId,
        report: PlaybackCapabilityReportV1,
    ) -> Result<MediaSessionV1, AppErrorV1>;
    fn report_decode_outcome(
        &self,
        session_id: &MediaSessionId,
        outcome: DecodeTestOutcomeV1,
    ) -> Result<MediaSessionV1, AppErrorV1>;
    fn prepare_playback(
        &self,
        session_id: &MediaSessionId,
        approved_video_transcode: bool,
    ) -> Result<MediaSessionV1, AppErrorV1>;
    fn cancel_conversion(&self, session_id: &MediaSessionId) -> Result<bool, AppErrorV1>;
    fn select_audio_stream(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<MediaSessionV1, AppErrorV1>;
    fn checkpoint(&self, checkpoint: PlaybackCheckpointV1) -> Result<bool, AppErrorV1>;
    fn close(&self, session_id: &MediaSessionId) -> Result<(), AppErrorV1>;
}

pub trait SubtitlePort: Send + Sync {
    fn window(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
        position_us: TimestampUs,
        revision: u64,
    ) -> Result<SubtitleWindowV1, AppErrorV1>;
}

pub trait TokenizerPort: Send + Sync {
    fn tokenize(&self, sentence: &str) -> Result<Vec<TokenV1>, AppErrorV1>;
    fn version(&self) -> &str;
}

pub trait DictionaryPort: Send + Sync {
    fn lookup(&self, token: &TokenV1) -> Result<Vec<DictionaryEntrySummaryV1>, AppErrorV1>;
    fn version(&self) -> &str;
}

pub trait DraftRepositoryPort: Send + Sync {
    fn insert_draft(&self, draft: &CardDraftV1) -> Result<(), AppErrorV1>;
    fn get_draft(&self, draft_id: &contracts::DraftId) -> Result<Option<CardDraftV1>, AppErrorV1>;
}

pub trait PublishHistoryPort: Send + Sync {
    fn note_for_mining_id(&self, mining_id: &str) -> Result<Option<i64>, AppErrorV1>;
    fn record_note(&self, mining_id: &str, note_id: i64) -> Result<(), AppErrorV1>;
}

pub trait CardPublisherPort: Send + Sync {
    fn publish(&self, request: CreateCardRequestV1) -> Result<CreateCardResultV1, AppErrorV1>;
}

pub trait HealthPort: Send + Sync {
    fn health(&self) -> AppHealthV1;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionSpec {
    pub session_id: MediaSessionId,
    pub start_us: TimestampUs,
    pub end_us: TimestampUs,
    pub frame_us: TimestampUs,
    pub profile: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedAssets {
    pub audio_path: PathBuf,
    pub image_path: PathBuf,
    pub metadata: BTreeMap<String, String>,
}

pub trait AssetExtractionPort: Send + Sync {
    fn extract(&self, spec: &ExtractionSpec) -> Result<ExtractedAssets, AppErrorV1>;
}
