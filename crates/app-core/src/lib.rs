//! Cross-domain orchestration. UI commands call this layer, never domain internals.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use contracts::{
    AnalysisStateV1, AnalyzedCueV1, AnalyzedTokenV1, AppErrorV1, CardDraftV1, CueId,
    DecodeTestOutcomeV1, DictionaryEntryId, DraftId, MediaSessionId, MediaSessionV1,
    PlaybackCapabilityReportV1, PlaybackCheckpointV1, StreamId, SubtitleOriginV1, SubtitleSourceId,
    SubtitleSourceV1, SubtitleWindowV1, TimestampUs, TokenId, TranslationWindowV1, error_codes,
};
use parking_lot::RwLock;
use ports::{DictionaryPort, DraftRepositoryPort, MediaSessionPort, TokenizerPort};
use sha2::{Digest, Sha256};
use subtitle_core::{source, timeline::Timeline};

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DraftSelection {
    pub session_id: MediaSessionId,
    pub subtitle_source_id: SubtitleSourceId,
    pub cue_id: CueId,
    pub token_id: TokenId,
    pub dictionary_entry_id: Option<DictionaryEntryId>,
    pub observed_playback_time_us: TimestampUs,
}

#[derive(Debug)]
struct SubtitleRecord {
    source: SubtitleSourceV1,
    path: PathBuf,
    timeline: Timeline,
    role: SubtitleRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubtitleRole {
    Interactive,
    Translation,
}

pub struct AppCore {
    media: Arc<dyn MediaSessionPort>,
    tokenizer: Arc<dyn TokenizerPort>,
    dictionary: Arc<dyn DictionaryPort>,
    drafts: Arc<dyn DraftRepositoryPort>,
    sessions: RwLock<HashMap<MediaSessionId, MediaSessionV1>>,
    subtitles: RwLock<HashMap<(MediaSessionId, SubtitleSourceId), SubtitleRecord>>,
}

impl std::fmt::Debug for AppCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppCore")
            .field("session_count", &self.sessions.read().len())
            .field("subtitle_count", &self.subtitles.read().len())
            .finish_non_exhaustive()
    }
}

impl AppCore {
    #[must_use]
    pub fn new(
        media: Arc<dyn MediaSessionPort>,
        tokenizer: Arc<dyn TokenizerPort>,
        dictionary: Arc<dyn DictionaryPort>,
        drafts: Arc<dyn DraftRepositoryPort>,
    ) -> Self {
        Self {
            media,
            tokenizer,
            dictionary,
            drafts,
            sessions: RwLock::new(HashMap::new()),
            subtitles: RwLock::new(HashMap::new()),
        }
    }

    pub fn import_media(&self, authorized_path: PathBuf) -> Result<MediaSessionV1, AppErrorV1> {
        let session = self.media.import_path(authorized_path)?;
        self.sessions
            .write()
            .insert(session.session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn apply_capabilities(
        &self,
        session_id: &MediaSessionId,
        report: PlaybackCapabilityReportV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let session = self.media.apply_capabilities(session_id, report)?;
        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn checkpoint(&self, checkpoint: PlaybackCheckpointV1) -> Result<bool, AppErrorV1> {
        self.media.checkpoint(checkpoint)
    }

    pub fn report_decode_outcome(
        &self,
        session_id: &MediaSessionId,
        outcome: DecodeTestOutcomeV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let session = self.media.report_decode_outcome(session_id, outcome)?;
        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn prepare_playback(
        &self,
        session_id: &MediaSessionId,
        approved_video_transcode: bool,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let session = self
            .media
            .prepare_playback(session_id, approved_video_transcode)?;
        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn cancel_conversion(&self, session_id: &MediaSessionId) -> Result<bool, AppErrorV1> {
        self.media.cancel_conversion(session_id)
    }

    pub fn select_audio_stream(
        &self,
        session_id: &MediaSessionId,
        stream_id: &StreamId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        let session = self.media.select_audio_stream(session_id, stream_id)?;
        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        Ok(session)
    }

    pub fn register_subtitle(
        &self,
        session_id: &MediaSessionId,
        authorized_path: &Path,
        origin: SubtitleOriginV1,
    ) -> Result<SubtitleSourceV1, AppErrorV1> {
        self.register_subtitle_record(
            session_id,
            authorized_path,
            origin,
            None,
            None,
            SubtitleRole::Interactive,
        )
    }

    pub fn register_translation_subtitle(
        &self,
        session_id: &MediaSessionId,
        authorized_path: &Path,
        origin: SubtitleOriginV1,
    ) -> Result<SubtitleSourceV1, AppErrorV1> {
        self.register_subtitle_record(
            session_id,
            authorized_path,
            origin,
            None,
            None,
            SubtitleRole::Translation,
        )
    }

    pub fn register_embedded_subtitle(
        &self,
        session_id: &MediaSessionId,
        extracted_path: &Path,
        stream_id: StreamId,
        language: Option<String>,
    ) -> Result<SubtitleSourceV1, AppErrorV1> {
        self.register_subtitle_record(
            session_id,
            extracted_path,
            SubtitleOriginV1::EmbeddedTextTrack,
            Some(stream_id),
            language,
            SubtitleRole::Interactive,
        )
    }

    pub fn register_embedded_translation_subtitle(
        &self,
        session_id: &MediaSessionId,
        extracted_path: &Path,
        stream_id: StreamId,
        language: Option<String>,
    ) -> Result<SubtitleSourceV1, AppErrorV1> {
        self.register_subtitle_record(
            session_id,
            extracted_path,
            SubtitleOriginV1::EmbeddedTextTrack,
            Some(stream_id),
            language,
            SubtitleRole::Translation,
        )
    }

    fn register_subtitle_record(
        &self,
        session_id: &MediaSessionId,
        authorized_path: &Path,
        origin: SubtitleOriginV1,
        embedded_stream_id: Option<StreamId>,
        language: Option<String>,
        role: SubtitleRole,
    ) -> Result<SubtitleSourceV1, AppErrorV1> {
        if !self.sessions.read().contains_key(session_id) {
            return Err(missing_session());
        }
        let loaded = source::load(authorized_path)?;
        let parsed = subtitle_core::parse(loaded.format, &loaded.source_version, &loaded.text)?;
        let display_name = authorized_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Selected subtitles".into());
        let source_id = subtitle_source_id(
            session_id,
            &loaded.source_version,
            embedded_stream_id.as_ref(),
            role,
        );
        let source = SubtitleSourceV1 {
            subtitle_source_id: source_id.clone(),
            origin,
            display_name,
            format: loaded.format,
            language,
            source_version: loaded.source_version,
            embedded_stream_id,
        };
        let mut subtitles = self.subtitles.write();
        subtitles.retain(|(owner, _), record| {
            should_retain_subtitle(owner, record.role, session_id, role)
        });
        subtitles.insert(
            (session_id.clone(), source_id),
            SubtitleRecord {
                source: source.clone(),
                path: authorized_path.to_path_buf(),
                timeline: Timeline::new(parsed.cues),
                role,
            },
        );
        Ok(source)
    }

    pub fn subtitle_window(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
        position_us: TimestampUs,
        revision: u64,
    ) -> Result<SubtitleWindowV1, AppErrorV1> {
        let mut window = {
            let subtitles = self.subtitles.read();
            let record = subtitles
                .get(&(session_id.clone(), source_id.clone()))
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::INVALID_REQUEST,
                        "The selected subtitle source is unavailable.",
                        true,
                    )
                })?;
            if record.role != SubtitleRole::Interactive {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "Only the Japanese study track can use interactive subtitle analysis.",
                    false,
                ));
            }
            record
                .timeline
                .window(session_id.clone(), source_id.clone(), position_us, revision)
        };
        let mut analyzed = Vec::with_capacity(window.cues.len());
        let mut dictionary_failed = false;
        for item in window.cues {
            let tokens = self.tokenizer.tokenize(&item.cue.plain_text)?;
            let mut analyzed_tokens = Vec::with_capacity(tokens.len());
            for token in tokens {
                let dictionary = match self.dictionary.lookup(&token) {
                    Ok(entries) => entries,
                    Err(_) => {
                        dictionary_failed = true;
                        Vec::new()
                    }
                };
                analyzed_tokens.push(AnalyzedTokenV1 { token, dictionary });
            }
            analyzed.push(AnalyzedCueV1 {
                cue: item.cue,
                tokens: analyzed_tokens,
            });
        }
        window.cues = analyzed;
        window.analysis_state = if dictionary_failed {
            window
                .warnings
                .push("Local dictionary unavailable; tokenization remains active.".into());
            AnalysisStateV1::Partial
        } else {
            AnalysisStateV1::Complete
        };
        Ok(window)
    }

    /// Returns a raw translation cue window without invoking the tokenizer or
    /// dictionary. English subtitles are display-only and must never enter the
    /// interactive Japanese/card pipeline.
    pub fn translation_window(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
        position_us: TimestampUs,
        revision: u64,
    ) -> Result<TranslationWindowV1, AppErrorV1> {
        let window = {
            let subtitles = self.subtitles.read();
            let record = subtitles
                .get(&(session_id.clone(), source_id.clone()))
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::INVALID_REQUEST,
                        "The selected translation subtitle source is unavailable.",
                        true,
                    )
                })?;
            if record.role != SubtitleRole::Translation {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "Only the translation subtitle track can use the raw cue window.",
                    false,
                ));
            }
            record.timeline.translation_window(
                session_id.clone(),
                source_id.clone(),
                position_us,
                revision,
            )
        };
        Ok(window)
    }

    pub fn create_draft(&self, selection: &DraftSelection) -> Result<CardDraftV1, AppErrorV1> {
        let (source_time, source_fingerprint) = {
            let sessions = self.sessions.read();
            let session = sessions
                .get(&selection.session_id)
                .ok_or_else(missing_session)?;
            let source_time = session
                .playback_plan
                .timeline_map
                .to_source_time(selection.observed_playback_time_us)
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::INVALID_REQUEST,
                        "Observed playback time cannot be mapped to the source timeline.",
                        false,
                    )
                })?;
            (source_time, session.source_fingerprint.clone())
        };
        let (cue, subtitle_source_version) = {
            let subtitles = self.subtitles.read();
            let record = subtitles
                .get(&(
                    selection.session_id.clone(),
                    selection.subtitle_source_id.clone(),
                ))
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::INVALID_REQUEST,
                        "The selected subtitle source is unavailable.",
                        true,
                    )
                })?;
            if record.role != SubtitleRole::Interactive {
                return Err(AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "Translation subtitles cannot be used to create word cards.",
                    false,
                ));
            }
            let cue = record
                .timeline
                .active_at(source_time)
                .into_iter()
                .find(|cue| cue.cue_id == selection.cue_id)
                .cloned()
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::STALE_REVISION,
                        "The selected subtitle is no longer active. Review the draft again.",
                        true,
                    )
                })?;
            (cue, record.source.source_version.clone())
        };
        let token = self
            .tokenizer
            .tokenize(&cue.plain_text)?
            .into_iter()
            .find(|token| token.token_id == selection.token_id)
            .ok_or_else(|| {
                AppErrorV1::new(
                    error_codes::STALE_REVISION,
                    "The selected token changed. Select it again.",
                    true,
                )
            })?;
        let entries = self.dictionary.lookup(&token)?;
        let dictionary_entry = match &selection.dictionary_entry_id {
            Some(id) => Some(
                entries
                    .into_iter()
                    .find(|entry| &entry.entry_id == id)
                    .ok_or_else(|| {
                        AppErrorV1::new(
                            error_codes::STALE_REVISION,
                            "The selected dictionary entry changed. Select it again.",
                            true,
                        )
                    })?,
            ),
            None => entries.into_iter().next(),
        };
        let draft = CardDraftV1 {
            draft_id: draft_id(selection, &subtitle_source_version),
            revision: 1,
            session_id: selection.session_id.clone(),
            subtitle_source_id: selection.subtitle_source_id.clone(),
            subtitle_source_version,
            cue,
            token,
            dictionary_entry,
            observed_source_time_us: source_time,
            source_fingerprint,
        };
        self.drafts.insert_draft(&draft)?;
        Ok(draft)
    }

    pub fn session(&self, session_id: &MediaSessionId) -> Result<MediaSessionV1, AppErrorV1> {
        self.sessions
            .read()
            .get(session_id)
            .cloned()
            .ok_or_else(missing_session)
    }

    pub fn sync_session(&self, session: MediaSessionV1) {
        self.sessions
            .write()
            .insert(session.session_id.clone(), session);
    }

    pub fn interactive_subtitle_binding(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
    ) -> Result<(PathBuf, SubtitleSourceV1), AppErrorV1> {
        self.subtitle_binding_for_role(session_id, source_id, SubtitleRole::Interactive)
    }

    pub fn translation_subtitle_binding(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
    ) -> Result<(PathBuf, SubtitleSourceV1), AppErrorV1> {
        self.subtitle_binding_for_role(session_id, source_id, SubtitleRole::Translation)
    }

    fn subtitle_binding_for_role(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
        expected_role: SubtitleRole,
    ) -> Result<(PathBuf, SubtitleSourceV1), AppErrorV1> {
        let subtitles = self.subtitles.read();
        let record = subtitles
            .get(&(session_id.clone(), source_id.clone()))
            .ok_or_else(|| {
                AppErrorV1::new(
                    error_codes::INVALID_REQUEST,
                    "The selected subtitle binding is no longer active.",
                    true,
                )
            })?;
        if record.role != expected_role {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The selected subtitle source does not match the requested track role.",
                false,
            ));
        }
        Ok((record.path.clone(), record.source.clone()))
    }

    pub fn unregister_translation_subtitle(
        &self,
        session_id: &MediaSessionId,
        source_id: &SubtitleSourceId,
    ) -> Result<(), AppErrorV1> {
        let mut subtitles = self.subtitles.write();
        let key = (session_id.clone(), source_id.clone());
        let record = subtitles.get(&key).ok_or_else(|| {
            AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The selected translation subtitle source is unavailable.",
                true,
            )
        })?;
        if record.role != SubtitleRole::Translation {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The interactive Japanese subtitle track cannot be removed as a translation.",
                false,
            ));
        }
        subtitles.remove(&key);
        Ok(())
    }

    pub fn close_session(&self, session_id: &MediaSessionId) -> Result<(), AppErrorV1> {
        self.media.close(session_id)?;
        self.sessions.write().remove(session_id);
        self.subtitles
            .write()
            .retain(|(owner, _), _| owner != session_id);
        Ok(())
    }
}

fn subtitle_source_id(
    session_id: &MediaSessionId,
    version: &str,
    embedded_stream_id: Option<&StreamId>,
    role: SubtitleRole,
) -> SubtitleSourceId {
    let mut hasher = Sha256::new();
    hasher.update(match role {
        SubtitleRole::Interactive => b"migaku-subtitle-source-v1\0".as_slice(),
        SubtitleRole::Translation => b"migaku-translation-subtitle-source-v1\0".as_slice(),
    });
    hasher.update(session_id.as_str().as_bytes());
    hasher.update(version.as_bytes());
    if let Some(stream_id) = embedded_stream_id {
        hasher.update(stream_id.as_str().as_bytes());
    }
    let digest = hex::encode(hasher.finalize());
    SubtitleSourceId::new(format!("sub_{}", &digest[..24]))
}

fn should_retain_subtitle(
    existing_session: &MediaSessionId,
    existing_role: SubtitleRole,
    replacement_session: &MediaSessionId,
    replacement_role: SubtitleRole,
) -> bool {
    existing_session != replacement_session || existing_role != replacement_role
}

fn draft_id(selection: &DraftSelection, source_version: &str) -> DraftId {
    let mut hasher = Sha256::new();
    hasher.update(b"migaku-draft-v1\0");
    hasher.update(selection.session_id.as_str().as_bytes());
    hasher.update(selection.cue_id.as_str().as_bytes());
    hasher.update(selection.token_id.as_str().as_bytes());
    hasher.update(selection.observed_playback_time_us.to_le_bytes());
    hasher.update(source_version.as_bytes());
    let digest = hex::encode(hasher.finalize());
    DraftId::new(format!("draft_{}", &digest[..32]))
}

fn missing_session() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "This media session is no longer available. Import the file again.",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_source_ids_are_scoped_by_role() {
        let session_id = MediaSessionId::new("session");
        let interactive = subtitle_source_id(
            &session_id,
            "same-file-version",
            None,
            SubtitleRole::Interactive,
        );
        let translation = subtitle_source_id(
            &session_id,
            "same-file-version",
            None,
            SubtitleRole::Translation,
        );
        assert_ne!(interactive, translation);
    }

    #[test]
    fn subtitle_role_replacement_keeps_other_sessions_and_roles() {
        let session = MediaSessionId::new("session-a");
        let other_session = MediaSessionId::new("session-b");

        assert!(!should_retain_subtitle(
            &session,
            SubtitleRole::Interactive,
            &session,
            SubtitleRole::Interactive,
        ));
        assert!(should_retain_subtitle(
            &session,
            SubtitleRole::Translation,
            &session,
            SubtitleRole::Interactive,
        ));
        assert!(should_retain_subtitle(
            &other_session,
            SubtitleRole::Interactive,
            &session,
            SubtitleRole::Interactive,
        ));
    }
}
