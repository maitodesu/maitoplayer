//! Deterministic providers for contract and desktop tests.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use anki_connect::transport::AnkiApi;
use contracts::{
    AppErrorV1, CardDraftV1, CreateCardRequestV1, CreateCardResultV1, DecodeTestOutcomeV1,
    DictionaryEntryId, DictionaryEntrySummaryV1, DictionarySenseV1, DraftId, MediaSessionId,
    MediaSessionV1, PlaybackCapabilityReportV1, PlaybackCheckpointV1, StreamId, TokenId, TokenV1,
    error_codes,
};
use parking_lot::Mutex;
use ports::{
    CardPublisherPort, DictionaryPort, DraftRepositoryPort, MediaSessionPort, PublishHistoryPort,
    TokenizerPort,
};
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct MemoryDrafts {
    drafts: Mutex<HashMap<DraftId, CardDraftV1>>,
    history: Mutex<HashMap<String, i64>>,
}

impl DraftRepositoryPort for MemoryDrafts {
    fn insert_draft(&self, draft: &CardDraftV1) -> Result<(), AppErrorV1> {
        self.drafts
            .lock()
            .insert(draft.draft_id.clone(), draft.clone());
        Ok(())
    }

    fn get_draft(&self, draft_id: &DraftId) -> Result<Option<CardDraftV1>, AppErrorV1> {
        Ok(self.drafts.lock().get(draft_id).cloned())
    }
}

impl PublishHistoryPort for MemoryDrafts {
    fn note_for_mining_id(&self, mining_id: &str) -> Result<Option<i64>, AppErrorV1> {
        Ok(self.history.lock().get(mining_id).copied())
    }

    fn record_note(&self, mining_id: &str, note_id: i64) -> Result<(), AppErrorV1> {
        self.history.lock().insert(mining_id.into(), note_id);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SimpleTokenizer;

impl TokenizerPort for SimpleTokenizer {
    fn tokenize(&self, sentence: &str) -> Result<Vec<TokenV1>, AppErrorV1> {
        let mut tokens = Vec::new();
        let mut start = 0_usize;
        for (index, character) in sentence.char_indices() {
            if index > start {
                tokens.push(simple_token(sentence, start, index));
            }
            let end = index + character.len_utf8();
            tokens.push(simple_token(sentence, index, end));
            start = end;
        }
        if start < sentence.len() {
            tokens.push(simple_token(sentence, start, sentence.len()));
        }
        Ok(tokens)
    }

    fn version(&self) -> &str {
        "simple-test-v1"
    }
}

fn simple_token(sentence: &str, start: usize, end: usize) -> TokenV1 {
    let surface = sentence[start..end].to_owned();
    let lookup_candidate = surface.chars().any(char::is_alphanumeric);
    TokenV1 {
        token_id: TokenId::new(format!("t-{start}-{end}")),
        surface: surface.clone(),
        byte_start: start as u32,
        byte_end: end as u32,
        lemma: surface.clone(),
        reading: surface,
        pronunciation: None,
        part_of_speech: vec!["test".into()],
        lookup_candidate,
    }
}

#[derive(Debug, Default)]
pub struct SimpleDictionary;

impl DictionaryPort for SimpleDictionary {
    fn lookup(&self, token: &TokenV1) -> Result<Vec<DictionaryEntrySummaryV1>, AppErrorV1> {
        if !token.lookup_candidate {
            return Ok(Vec::new());
        }
        Ok(vec![DictionaryEntrySummaryV1 {
            entry_id: DictionaryEntryId::new(format!("entry-{}", token.token_id)),
            headwords: vec![token.lemma.clone()],
            readings: vec![token.reading.clone()],
            senses: vec![DictionarySenseV1 {
                glosses: vec![format!("Definition for {}", token.surface)],
                parts_of_speech: token.part_of_speech.clone(),
                restrictions: Vec::new(),
                fields: Vec::new(),
                dialects: Vec::new(),
                misc: Vec::new(),
            }],
            pitch_accents: Vec::new(),
            jlpt_level: None,
            jlpt_source: None,
            match_reason: "test_exact".into(),
            priority_score: 100,
        }])
    }

    fn version(&self) -> &str {
        "simple-dictionary-v1"
    }
}

#[derive(Debug, Default)]
pub struct AnkiEmulator {
    state: Mutex<AnkiEmulatorState>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AnkiEmulatorFault {
    #[default]
    None,
    ApiError,
    MalformedResult,
    DisconnectBeforeCommit,
    DisconnectAfterCommit,
    DuplicateMarker,
    MissingModelField,
    IncompatibleVersion,
}

#[derive(Debug, Default)]
struct AnkiEmulatorState {
    notes: HashMap<String, Vec<i64>>,
    next_note_id: i64,
    fault: AnkiEmulatorFault,
    delay: Duration,
    calls: HashMap<String, usize>,
}

impl AnkiEmulator {
    pub fn disconnect_after_commit(&self, enabled: bool) {
        self.state.lock().fault = if enabled {
            AnkiEmulatorFault::DisconnectAfterCommit
        } else {
            AnkiEmulatorFault::None
        };
    }

    pub fn set_fault(&self, fault: AnkiEmulatorFault) {
        self.state.lock().fault = fault;
    }

    pub fn set_delay(&self, delay: Duration) {
        self.state.lock().delay = delay;
    }

    pub fn note_count(&self) -> usize {
        self.state.lock().notes.values().map(Vec::len).sum()
    }

    pub fn action_calls(&self, action: &str) -> usize {
        self.state.lock().calls.get(action).copied().unwrap_or(0)
    }
}

impl AnkiApi for AnkiEmulator {
    fn invoke(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
        let mut state = self.state.lock();
        let delay = state.delay;
        *state.calls.entry(action.to_owned()).or_default() += 1;
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if state.fault == AnkiEmulatorFault::ApiError {
            return Err(AppErrorV1::new(
                error_codes::ANKI_SCHEMA_MISMATCH,
                "Injected Anki API error.",
                true,
            ));
        }
        if state.fault == AnkiEmulatorFault::MalformedResult {
            return Ok(json!({ "malformed": true }));
        }
        match action {
            "version" => Ok(json!(
                if state.fault == AnkiEmulatorFault::IncompatibleVersion {
                    5
                } else {
                    6
                }
            )),
            "deckNames" => Ok(json!(["Default"])),
            "modelNames" => Ok(json!(["Basic"])),
            "modelFieldNames" => {
                if state.fault == AnkiEmulatorFault::MissingModelField {
                    Ok(json!(["Front"]))
                } else {
                    Ok(json!(["Front", "Back", "Audio", "Image", "Mining ID"]))
                }
            }
            "findNotes" => {
                let marker = params
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_start_matches("tag:");
                let mut ids = state.notes.get(marker).cloned().unwrap_or_default();
                if state.fault == AnkiEmulatorFault::DuplicateMarker && !ids.is_empty() {
                    state.next_note_id += 1;
                    ids.push(state.next_note_id);
                }
                Ok(json!(ids))
            }
            "addNote" => {
                if state.fault == AnkiEmulatorFault::DisconnectBeforeCommit {
                    return Err(AppErrorV1::new(
                        error_codes::ANKI_OFFLINE,
                        "Injected pre-commit disconnect.",
                        true,
                    ));
                }
                let marker = params
                    .pointer("/note/tags")
                    .and_then(Value::as_array)
                    .and_then(|tags| {
                        tags.iter()
                            .filter_map(Value::as_str)
                            .find(|tag| tag.starts_with("migaku_id_"))
                    })
                    .ok_or_else(|| {
                        AppErrorV1::new(
                            error_codes::ANKI_SCHEMA_MISMATCH,
                            "Test note omitted mining marker.",
                            false,
                        )
                    })?
                    .to_owned();
                state.next_note_id += 1;
                let note_id = state.next_note_id;
                state.notes.entry(marker).or_default().push(note_id);
                if state.fault == AnkiEmulatorFault::DisconnectAfterCommit {
                    Err(AppErrorV1::new(
                        error_codes::ANKI_OFFLINE,
                        "Injected post-commit disconnect.",
                        true,
                    ))
                } else {
                    Ok(json!(note_id))
                }
            }
            "storeMediaFile" => params
                .get("filename")
                .and_then(Value::as_str)
                .map(|name| json!(name))
                .ok_or_else(|| {
                    AppErrorV1::new(
                        error_codes::ANKI_SCHEMA_MISMATCH,
                        "Test media upload omitted its filename.",
                        false,
                    )
                }),
            _ => Err(AppErrorV1::new(
                error_codes::ANKI_SCHEMA_MISMATCH,
                "Unsupported emulator action.",
                false,
            )),
        }
    }
}

#[derive(Debug, Default)]
pub struct UnimplementedMedia;

impl MediaSessionPort for UnimplementedMedia {
    fn import_path(&self, _authorized_path: PathBuf) -> Result<MediaSessionV1, AppErrorV1> {
        Err(AppErrorV1::new(
            error_codes::MEDIA_PROBE_FAILED,
            "Test media provider needs a fixture session.",
            false,
        ))
    }

    fn apply_capabilities(
        &self,
        _session_id: &MediaSessionId,
        _report: PlaybackCapabilityReportV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        Err(AppErrorV1::new(
            error_codes::MEDIA_SCOPE_DENIED,
            "No test session.",
            false,
        ))
    }

    fn report_decode_outcome(
        &self,
        _session_id: &MediaSessionId,
        _outcome: DecodeTestOutcomeV1,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        Err(no_test_session())
    }

    fn prepare_playback(
        &self,
        _session_id: &MediaSessionId,
        _approved_video_transcode: bool,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        Err(no_test_session())
    }

    fn cancel_conversion(&self, _session_id: &MediaSessionId) -> Result<bool, AppErrorV1> {
        Ok(false)
    }

    fn select_audio_stream(
        &self,
        _session_id: &MediaSessionId,
        _stream_id: &StreamId,
    ) -> Result<MediaSessionV1, AppErrorV1> {
        Err(no_test_session())
    }

    fn checkpoint(&self, _checkpoint: PlaybackCheckpointV1) -> Result<bool, AppErrorV1> {
        Ok(true)
    }

    fn close(&self, _session_id: &MediaSessionId) -> Result<(), AppErrorV1> {
        Ok(())
    }
}

fn no_test_session() -> AppErrorV1 {
    AppErrorV1::new(error_codes::MEDIA_SCOPE_DENIED, "No test session.", false)
}

#[derive(Debug, Default)]
pub struct UnimplementedPublisher;

impl CardPublisherPort for UnimplementedPublisher {
    fn publish(&self, _request: CreateCardRequestV1) -> Result<CreateCardResultV1, AppErrorV1> {
        Err(AppErrorV1::new(
            error_codes::ANKI_OFFLINE,
            "Test publisher was not configured.",
            true,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_params() -> Value {
        json!({
            "note": {
                "tags": [format!("migaku_id_{}", "a".repeat(64))]
            }
        })
    }

    #[test]
    fn emulator_distinguishes_pre_and_post_commit_disconnects() -> Result<(), AppErrorV1> {
        let pre_commit = AnkiEmulator::default();
        pre_commit.set_fault(AnkiEmulatorFault::DisconnectBeforeCommit);
        assert!(pre_commit.invoke("addNote", note_params()).is_err());
        assert_eq!(pre_commit.note_count(), 0);

        let post_commit = AnkiEmulator::default();
        post_commit.set_fault(AnkiEmulatorFault::DisconnectAfterCommit);
        assert!(post_commit.invoke("addNote", note_params()).is_err());
        assert_eq!(post_commit.note_count(), 1);
        let found = post_commit.invoke(
            "findNotes",
            json!({ "query": format!("tag:migaku_id_{}", "a".repeat(64)) }),
        )?;
        assert_eq!(found, json!([1]));
        Ok(())
    }

    #[test]
    fn emulator_exposes_schema_and_malformed_faults() -> Result<(), AppErrorV1> {
        let emulator = AnkiEmulator::default();
        assert_eq!(emulator.invoke("version", json!({}))?, json!(6));
        assert_eq!(emulator.invoke("deckNames", json!({}))?, json!(["Default"]));
        emulator.set_fault(AnkiEmulatorFault::MalformedResult);
        assert!(emulator.invoke("findNotes", json!({}))?.is_object());
        assert_eq!(emulator.action_calls("findNotes"), 1);
        Ok(())
    }
}
