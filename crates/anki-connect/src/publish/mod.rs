use std::{collections::BTreeMap, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use contracts::{
    AppErrorV1, CreateCardOutcomeV1, CreateCardRequestV1, CreateCardResultV1, JobId, error_codes,
};
use ports::{CardPublisherPort, DraftRepositoryPort, PublishHistoryPort};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    note::{AnkiProfile, NoteMedia, build_note_with_media, marker_tag},
    transport::AnkiApi,
};

const MINIMUM_API_VERSION: u64 = 6;
const MAX_SCHEMA_ITEMS: usize = 10_000;
const MAX_MEDIA_FILES: usize = 8;
const MAX_TOTAL_MEDIA_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Eq, PartialEq)]
pub struct AnkiMedia {
    pub media_name: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishBoundary {
    BeforeLookup,
    BeforeMediaUpload,
    MediaUploaded,
    BeforeNoteCreation,
    NoteCreated(i64),
    Confirmed(i64),
}

pub trait PublishObserver: Send + Sync {
    fn observe(&self, boundary: PublishBoundary) -> Result<(), AppErrorV1>;
}

struct NoopPublishObserver;

impl PublishObserver for NoopPublishObserver {
    fn observe(&self, _boundary: PublishBoundary) -> Result<(), AppErrorV1> {
        Ok(())
    }
}

impl std::fmt::Debug for AnkiMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AnkiMedia")
            .field("media_name", &self.media_name)
            .field("size_bytes", &self.data.len())
            .finish()
    }
}

pub struct CardPublisher {
    api: Arc<dyn AnkiApi>,
    drafts: Arc<dyn DraftRepositoryPort>,
    history: Arc<dyn PublishHistoryPort>,
    profiles: BTreeMap<String, AnkiProfile>,
    publish_lock: parking_lot::Mutex<()>,
}

impl std::fmt::Debug for CardPublisher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CardPublisher")
            .field("profile_count", &self.profiles.len())
            .finish_non_exhaustive()
    }
}

impl CardPublisher {
    #[must_use]
    pub fn new(
        api: Arc<dyn AnkiApi>,
        drafts: Arc<dyn DraftRepositoryPort>,
        history: Arc<dyn PublishHistoryPort>,
        profiles: impl IntoIterator<Item = AnkiProfile>,
    ) -> Self {
        Self {
            api,
            drafts,
            history,
            profiles: profiles
                .into_iter()
                .map(|profile| (profile.profile_id.clone(), profile))
                .collect(),
            publish_lock: parking_lot::Mutex::new(()),
        }
    }

    pub fn validate_profile(&self, profile_id: &str) -> Result<(), AppErrorV1> {
        let profile = self.profiles.get(profile_id).ok_or_else(|| {
            AppErrorV1::new(
                error_codes::ANKI_SCHEMA_MISMATCH,
                "Select a valid Anki card profile.",
                true,
            )
        })?;
        validate_remote_profile(self.api.as_ref(), profile)
    }

    pub fn validate_profile_snapshot(&self, profile: &AnkiProfile) -> Result<(), AppErrorV1> {
        validate_remote_profile(self.api.as_ref(), profile)
    }

    #[must_use]
    pub fn profile_snapshot(&self, profile_id: &str) -> Option<AnkiProfile> {
        self.profiles.get(profile_id).cloned()
    }

    pub fn reconcile(
        &self,
        request: CreateCardRequestV1,
    ) -> Result<CreateCardResultV1, AppErrorV1> {
        self.publish_inner(request, &[], None, &NoopPublishObserver)
    }

    pub fn publish_with_media(
        &self,
        request: CreateCardRequestV1,
        media: &[AnkiMedia],
    ) -> Result<CreateCardResultV1, AppErrorV1> {
        self.publish_inner(request, media, None, &NoopPublishObserver)
    }

    pub fn publish_snapshot_with_media(
        &self,
        request: CreateCardRequestV1,
        profile: &AnkiProfile,
        media: &[AnkiMedia],
        observer: &dyn PublishObserver,
    ) -> Result<CreateCardResultV1, AppErrorV1> {
        self.publish_inner(request, media, Some(profile), observer)
    }

    fn publish_inner(
        &self,
        request: CreateCardRequestV1,
        media: &[AnkiMedia],
        profile_snapshot: Option<&AnkiProfile>,
        observer: &dyn PublishObserver,
    ) -> Result<CreateCardResultV1, AppErrorV1> {
        let _publish_guard = self.publish_lock.lock();
        if !request.confirmed {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Confirm the card before publishing.",
                false,
            ));
        }
        let draft = self.drafts.get_draft(&request.draft_id)?.ok_or_else(|| {
            AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "The card draft no longer exists.",
                false,
            )
        })?;
        if draft.revision != request.expected_draft_revision {
            return Err(AppErrorV1::new(
                error_codes::STALE_REVISION,
                "The card draft changed. Review it again before publishing.",
                true,
            ));
        }
        let profile = match profile_snapshot {
            Some(profile) if profile.profile_id == request.profile_id => profile,
            Some(_) => {
                return Err(schema_error(
                    "The immutable profile snapshot did not match the publish request.",
                ));
            }
            None => self.profiles.get(&request.profile_id).ok_or_else(|| {
                AppErrorV1::new(
                    error_codes::ANKI_SCHEMA_MISMATCH,
                    "Select a valid Anki card profile.",
                    true,
                )
            })?,
        };
        let mining_id = mining_id(&draft, &profile.profile_id);
        let job_id = JobId::new(format!("publish_{}", &mining_id[..24]));
        observer.observe(PublishBoundary::BeforeLookup)?;
        if let Some(note_id) = self.history.note_for_mining_id(&mining_id)? {
            observe_after_note(observer, PublishBoundary::NoteCreated(note_id))?;
            observe_after_note(observer, PublishBoundary::Confirmed(note_id))?;
            return Ok(existing_result(job_id, note_id));
        }

        let marker = marker_tag(&mining_id);
        let remote = self
            .api
            .invoke_retryable("findNotes", json!({ "query": format!("tag:{marker}") }))?;
        let remote_note_ids = note_ids(&remote)?;
        if remote_note_ids.len() > 1 {
            return Err(AppErrorV1::new(
                error_codes::ANKI_SCHEMA_MISMATCH,
                "Multiple Anki notes have the same mining marker. Resolve the duplicate before publishing.",
                false,
            )
            .with_diagnostics(format!(
                "Marker matched {} note IDs.",
                remote_note_ids.len()
            )));
        }
        if let Some(note_id) = remote_note_ids.first().copied() {
            observe_after_note(observer, PublishBoundary::NoteCreated(note_id))?;
            self.history.record_note(&mining_id, note_id)?;
            observe_after_note(observer, PublishBoundary::Confirmed(note_id))?;
            return Ok(existing_result(job_id, note_id));
        }

        validate_remote_profile(self.api.as_ref(), profile)?;
        let note_media = validate_media(media)?;
        let note = build_note_with_media(
            profile,
            &draft,
            &request.editable_fields,
            &mining_id,
            &note_media,
        )?;
        observer.observe(PublishBoundary::BeforeMediaUpload)?;
        let media_names = upload_media(self.api.as_ref(), media)?;
        observer.observe(PublishBoundary::MediaUploaded)?;
        observer.observe(PublishBoundary::BeforeNoteCreation)?;
        let added = self
            .api
            .invoke("addNote", json!({ "note": note }))
            .map_err(|error| {
                if error.code == error_codes::ANKI_OFFLINE {
                    uncertain_error(
                        "The connection ended while Anki was adding the note. Reconciliation is required.",
                    )
                } else {
                    error
                }
            })?;
        let note_id = added.as_i64().filter(|id| *id > 0).ok_or_else(|| {
            uncertain_error(
                "Anki did not return a valid positive note ID. Reconciliation is required.",
            )
        })?;
        observe_after_note(observer, PublishBoundary::NoteCreated(note_id))?;
        self.history
            .record_note(&mining_id, note_id)
            .map_err(|error| {
                observer_uncertain("Mining history could not confirm the new note.", error)
            })?;
        observe_after_note(observer, PublishBoundary::Confirmed(note_id))?;
        Ok(CreateCardResultV1 {
            job_id,
            note_id: Some(note_id),
            media_names,
            outcome: CreateCardOutcomeV1::Created,
        })
    }
}

impl CardPublisherPort for CardPublisher {
    fn publish(&self, request: CreateCardRequestV1) -> Result<CreateCardResultV1, AppErrorV1> {
        self.publish_inner(request, &[], None, &NoopPublishObserver)
    }
}

fn observe_after_note(
    observer: &dyn PublishObserver,
    boundary: PublishBoundary,
) -> Result<(), AppErrorV1> {
    observer
        .observe(boundary)
        .map_err(|error| observer_uncertain("Publish recovery state could not be saved.", error))
}

fn observer_uncertain(message: &str, error: AppErrorV1) -> AppErrorV1 {
    uncertain_error(message).with_diagnostics(format!("{}: {}", error.code, error.message))
}

fn validate_media(media: &[AnkiMedia]) -> Result<NoteMedia, AppErrorV1> {
    if media.len() > MAX_MEDIA_FILES {
        return Err(schema_error(
            "Too many media files were attached to one note.",
        ));
    }
    let total_bytes = media.iter().try_fold(0_usize, |total, item| {
        total
            .checked_add(item.data.len())
            .ok_or_else(|| schema_error("Attached media size overflowed."))
    })?;
    if total_bytes > MAX_TOTAL_MEDIA_BYTES {
        return Err(schema_error(
            "Attached media exceeded the thirty-two MiB safety limit.",
        ));
    }
    let mut note_media = NoteMedia::default();
    let mut names = std::collections::HashSet::new();
    for item in media {
        let extension = validate_content_addressed_media(item)?;
        if !names.insert(&item.media_name) {
            return Err(schema_error("Attached media names must be unique."));
        }
        match extension {
            "mp3" | "ogg" if note_media.audio_media_name.is_none() => {
                note_media.audio_media_name = Some(item.media_name.clone());
            }
            "jpg" | "png" if note_media.image_media_name.is_none() => {
                note_media.image_media_name = Some(item.media_name.clone());
            }
            "mp3" | "ogg" | "jpg" | "png" => {
                return Err(schema_error(
                    "Only one audio file and one image file may be attached.",
                ));
            }
            _ => return Err(schema_error("Attached media type was unsupported.")),
        }
    }
    Ok(note_media)
}

fn validate_content_addressed_media(media: &AnkiMedia) -> Result<&str, AppErrorV1> {
    if media.data.is_empty() {
        return Err(schema_error("Attached media was empty."));
    }
    let (hash, extension) = media
        .media_name
        .strip_prefix("kiku-")
        .and_then(|value| value.rsplit_once('.'))
        .ok_or_else(|| schema_error("Attached media name was not content-addressed."))?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !matches!(extension, "mp3" | "ogg" | "jpg" | "png")
    {
        return Err(schema_error("Attached media name was invalid."));
    }
    if hex::encode(Sha256::digest(&media.data)) != hash {
        return Err(schema_error(
            "Attached media contents did not match their content-addressed name.",
        ));
    }
    Ok(extension)
}

fn upload_media(api: &dyn AnkiApi, media: &[AnkiMedia]) -> Result<Vec<String>, AppErrorV1> {
    let mut uploaded = Vec::with_capacity(media.len());
    for item in media {
        let result = api.invoke(
            "storeMediaFile",
            json!({
                "filename": item.media_name,
                "data": STANDARD.encode(&item.data),
            }),
        )?;
        if result.as_str() != Some(item.media_name.as_str()) {
            return Err(schema_error(
                "AnkiConnect did not confirm the uploaded media name.",
            ));
        }
        uploaded.push(item.media_name.clone());
    }
    Ok(uploaded)
}

#[must_use]
pub fn mining_id(draft: &contracts::CardDraftV1, profile_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"kiku-mining-v2\0");
    update_component(&mut hasher, draft.source_fingerprint.as_bytes());
    update_component(&mut hasher, draft.subtitle_source_version.as_bytes());
    hasher.update(draft.cue.start_us.to_le_bytes());
    hasher.update(draft.cue.end_us.to_le_bytes());
    let cue_text_hash = Sha256::digest(draft.cue.plain_text.as_bytes());
    hasher.update(cue_text_hash);
    update_component(&mut hasher, draft.token.lemma.as_bytes());
    update_component(&mut hasher, profile_id.as_bytes());
    hex::encode(hasher.finalize())
}

fn update_component(hasher: &mut Sha256, component: &[u8]) {
    hasher.update((component.len() as u64).to_le_bytes());
    hasher.update(component);
}

fn validate_remote_profile(api: &dyn AnkiApi, profile: &AnkiProfile) -> Result<(), AppErrorV1> {
    let version = api.invoke_retryable("version", json!({}))?;
    let version = version
        .as_u64()
        .ok_or_else(|| schema_error("AnkiConnect version was not an integer."))?;
    if version < MINIMUM_API_VERSION {
        return Err(schema_error(format!(
            "AnkiConnect API version {version} is older than required version {MINIMUM_API_VERSION}."
        )));
    }
    let decks = string_list(api.invoke_retryable("deckNames", json!({}))?, "deck list")?;
    if !decks.iter().any(|name| name == &profile.deck_name) {
        return Err(schema_error(format!(
            "Configured deck '{}' does not exist.",
            profile.deck_name
        )));
    }
    let models = string_list(api.invoke_retryable("modelNames", json!({}))?, "model list")?;
    if !models.iter().any(|name| name == &profile.model_name) {
        return Err(schema_error(format!(
            "Configured note type '{}' does not exist.",
            profile.model_name
        )));
    }
    let fields = string_list(
        api.invoke_retryable(
            "modelFieldNames",
            json!({ "modelName": profile.model_name }),
        )?,
        "model field list",
    )?;
    for field in profile.field_mapping.values() {
        if !fields.iter().any(|candidate| candidate == field) {
            return Err(schema_error(format!(
                "Configured model field '{field}' does not exist."
            )));
        }
    }
    Ok(())
}

fn string_list(value: Value, label: &str) -> Result<Vec<String>, AppErrorV1> {
    let array = value
        .as_array()
        .ok_or_else(|| schema_error(format!("AnkiConnect {label} was not an array.")))?;
    if array.len() > MAX_SCHEMA_ITEMS {
        return Err(schema_error(format!(
            "AnkiConnect {label} exceeded its item limit."
        )));
    }
    array
        .iter()
        .map(|item| {
            let value = item
                .as_str()
                .ok_or_else(|| schema_error(format!("AnkiConnect {label} contained non-text.")))?;
            if value.len() > 1_024 {
                return Err(schema_error(format!(
                    "AnkiConnect {label} contained an oversized value."
                )));
            }
            Ok(value.to_owned())
        })
        .collect()
}

fn note_ids(value: &Value) -> Result<Vec<i64>, AppErrorV1> {
    let array = value
        .as_array()
        .ok_or_else(|| schema_error("AnkiConnect note search result was not an array."))?;
    if array.len() > MAX_SCHEMA_ITEMS {
        return Err(schema_error(
            "AnkiConnect note search exceeded its item limit.",
        ));
    }
    let mut ids = array
        .iter()
        .map(|item| {
            item.as_i64()
                .filter(|id| *id > 0)
                .ok_or_else(|| schema_error("AnkiConnect returned an invalid note ID."))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn existing_result(job_id: JobId, note_id: i64) -> CreateCardResultV1 {
    CreateCardResultV1 {
        job_id,
        note_id: Some(note_id),
        media_names: Vec::new(),
        outcome: CreateCardOutcomeV1::AlreadyExists,
    }
}

fn uncertain_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::PUBLISH_OUTCOME_UNCERTAIN,
        "Anki may have created the note before the connection failed. Reconcile before retrying.",
        true,
    )
    .with_diagnostics(detail)
}

fn schema_error(detail: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::ANKI_SCHEMA_MISMATCH,
        "The selected Anki deck, note type, or field mapping is unavailable.",
        true,
    )
    .with_diagnostics(detail.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Mutex, MutexGuard},
    };

    use contracts::{
        CardDraftV1, CueId, DraftId, MediaSessionId, SubtitleCueV1, SubtitleSourceId,
        SubtitleStyleHintV1, TokenId, TokenV1,
    };

    use super::*;

    #[derive(Debug, Default)]
    struct FakeState {
        drafts: HashMap<DraftId, CardDraftV1>,
        history: HashMap<String, i64>,
    }

    #[derive(Debug, Default)]
    struct Repository {
        state: Mutex<FakeState>,
    }

    impl Repository {
        fn lock(&self) -> MutexGuard<'_, FakeState> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl DraftRepositoryPort for Repository {
        fn insert_draft(&self, draft: &CardDraftV1) -> Result<(), AppErrorV1> {
            self.lock()
                .drafts
                .insert(draft.draft_id.clone(), draft.clone());
            Ok(())
        }

        fn get_draft(&self, draft_id: &DraftId) -> Result<Option<CardDraftV1>, AppErrorV1> {
            Ok(self.lock().drafts.get(draft_id).cloned())
        }
    }

    impl PublishHistoryPort for Repository {
        fn note_for_mining_id(&self, mining_id: &str) -> Result<Option<i64>, AppErrorV1> {
            Ok(self.lock().history.get(mining_id).copied())
        }

        fn record_note(&self, mining_id: &str, note_id: i64) -> Result<(), AppErrorV1> {
            self.lock().history.insert(mining_id.to_owned(), note_id);
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct PostCommitDisconnectApi {
        state: Mutex<ApiState>,
    }

    #[derive(Debug, Default)]
    struct ApiState {
        note_by_marker: HashMap<String, i64>,
        add_calls: usize,
        media_calls: usize,
        disconnect_after_commit: bool,
    }

    impl PostCommitDisconnectApi {
        fn add_calls(&self) -> usize {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .add_calls
        }

        fn media_calls(&self) -> usize {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .media_calls
        }
    }

    impl AnkiApi for PostCommitDisconnectApi {
        fn invoke(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match action {
                "version" => Ok(json!(6)),
                "deckNames" => Ok(json!(["Mining"])),
                "modelNames" => Ok(json!(["Basic"])),
                "modelFieldNames" => Ok(json!(["Front", "Back", "Audio", "Image"])),
                "findNotes" => {
                    let marker = params
                        .get("query")
                        .and_then(Value::as_str)
                        .and_then(|query| query.strip_prefix("tag:"))
                        .unwrap_or_default();
                    Ok(json!(
                        state
                            .note_by_marker
                            .get(marker)
                            .copied()
                            .into_iter()
                            .collect::<Vec<_>>()
                    ))
                }
                "addNote" => {
                    let marker = params
                        .pointer("/note/tags")
                        .and_then(Value::as_array)
                        .and_then(|tags| {
                            tags.iter()
                                .filter_map(Value::as_str)
                                .find(|tag| tag.starts_with("kiku_id_"))
                        })
                        .ok_or_else(|| schema_error("Note omitted its marker."))?
                        .to_owned();
                    state.add_calls += 1;
                    state.note_by_marker.insert(marker, 42);
                    if state.disconnect_after_commit {
                        Err(AppErrorV1::new(
                            error_codes::ANKI_OFFLINE,
                            "Injected disconnect after commit.",
                            true,
                        ))
                    } else {
                        Ok(json!(42))
                    }
                }
                "storeMediaFile" => {
                    state.media_calls += 1;
                    Ok(params.get("filename").cloned().unwrap_or(Value::Null))
                }
                _ => Err(schema_error("Unexpected test action.")),
            }
        }
    }

    fn draft(source_fingerprint: &str, subtitle_version: &str) -> CardDraftV1 {
        CardDraftV1 {
            draft_id: DraftId::new("draft"),
            revision: 1,
            session_id: MediaSessionId::new("session"),
            subtitle_source_id: SubtitleSourceId::new("subtitle"),
            subtitle_source_version: subtitle_version.into(),
            cue: SubtitleCueV1 {
                cue_id: CueId::new("cue"),
                start_us: 1_000_000,
                end_us: 2_000_000,
                plain_text: "見る".into(),
                source_text: "見る".into(),
                track_order: 0,
                style_hint: SubtitleStyleHintV1::default(),
            },
            token: TokenV1 {
                token_id: TokenId::new("token"),
                surface: "見る".into(),
                byte_start: 0,
                byte_end: 6,
                lemma: "見る".into(),
                reading: "みる".into(),
                pronunciation: None,
                part_of_speech: vec!["verb".into()],
                lookup_candidate: true,
            },
            dictionary_entry: None,
            observed_source_time_us: 1_500_000,
            source_fingerprint: source_fingerprint.into(),
        }
    }

    fn profile() -> AnkiProfile {
        AnkiProfile {
            profile_id: "default".into(),
            deck_name: "Mining".into(),
            model_name: "Basic".into(),
            field_mapping: BTreeMap::from([
                ("expression".into(), "Front".into()),
                ("sentence".into(), "Back".into()),
                ("audio".into(), "Audio".into()),
                ("image".into(), "Image".into()),
            ]),
            tags: Vec::new(),
        }
    }

    fn request() -> CreateCardRequestV1 {
        CreateCardRequestV1 {
            draft_id: DraftId::new("draft"),
            expected_draft_revision: 1,
            profile_id: "default".into(),
            editable_fields: BTreeMap::new(),
            confirmed: true,
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        boundaries: Mutex<Vec<PublishBoundary>>,
    }

    impl PublishObserver for RecordingObserver {
        fn observe(&self, boundary: PublishBoundary) -> Result<(), AppErrorV1> {
            self.boundaries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(boundary);
            Ok(())
        }
    }

    fn media() -> Vec<AnkiMedia> {
        let audio_data = b"ID3 audio".to_vec();
        let image_data = [137, 80, 78, 71, 13, 10, 26, 10, 1].to_vec();
        vec![
            AnkiMedia {
                media_name: format!("kiku-{}.mp3", hex::encode(Sha256::digest(&audio_data))),
                data: audio_data,
            },
            AnkiMedia {
                media_name: format!("kiku-{}.png", hex::encode(Sha256::digest(&image_data))),
                data: image_data,
            },
        ]
    }

    #[test]
    fn post_commit_disconnect_reconciles_to_exactly_one_note() -> Result<(), AppErrorV1> {
        let repository = Arc::new(Repository::default());
        repository.insert_draft(&draft("source", "subtitle"))?;
        let api = Arc::new(PostCommitDisconnectApi::default());
        api.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .disconnect_after_commit = true;
        let publisher =
            CardPublisher::new(api.clone(), repository.clone(), repository, [profile()]);

        let first = publisher.publish(request());
        assert!(matches!(
            first,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::PUBLISH_OUTCOME_UNCERTAIN
        ));
        let reconciled = publisher.reconcile(request())?;
        assert_eq!(reconciled.outcome, CreateCardOutcomeV1::AlreadyExists);
        assert_eq!(reconciled.note_id, Some(42));
        assert_eq!(api.add_calls(), 1);
        Ok(())
    }

    #[test]
    fn length_prefixing_distinguishes_ambiguous_component_boundaries() {
        let first = mining_id(&draft("ab", "c"), "default");
        let second = mining_id(&draft("a", "bc"), "default");
        assert_ne!(first, second);
    }

    #[test]
    fn uploads_only_hash_named_verified_media() -> Result<(), AppErrorV1> {
        let repository = Arc::new(Repository::default());
        repository.insert_draft(&draft("source", "subtitle"))?;
        let api = Arc::new(PostCommitDisconnectApi::default());
        let publisher =
            CardPublisher::new(api.clone(), repository.clone(), repository, [profile()]);
        let media = media();
        let created = publisher.publish_with_media(request(), &media)?;
        assert_eq!(created.outcome, CreateCardOutcomeV1::Created);
        assert_eq!(created.media_names.len(), 2);
        assert_eq!(api.media_calls(), 2);

        let invalid = AnkiMedia {
            media_name: format!("kiku-{}.mp3", "a".repeat(64)),
            data: b"different".to_vec(),
        };
        assert!(validate_media(&[invalid]).is_err());
        Ok(())
    }

    #[test]
    fn observer_reports_side_effect_boundaries_in_order() -> Result<(), AppErrorV1> {
        let repository = Arc::new(Repository::default());
        repository.insert_draft(&draft("source", "subtitle"))?;
        let api = Arc::new(PostCommitDisconnectApi::default());
        let publisher = CardPublisher::new(api, repository.clone(), repository, [profile()]);
        let observer = RecordingObserver::default();

        publisher.publish_snapshot_with_media(request(), &profile(), &media(), &observer)?;

        assert_eq!(
            *observer
                .boundaries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                PublishBoundary::BeforeLookup,
                PublishBoundary::BeforeMediaUpload,
                PublishBoundary::MediaUploaded,
                PublishBoundary::BeforeNoteCreation,
                PublishBoundary::NoteCreated(42),
                PublishBoundary::Confirmed(42),
            ]
        );
        Ok(())
    }

    #[test]
    fn concurrent_identical_requests_add_exactly_one_note() -> Result<(), AppErrorV1> {
        let repository = Arc::new(Repository::default());
        repository.insert_draft(&draft("source", "subtitle"))?;
        let api = Arc::new(PostCommitDisconnectApi::default());
        let publisher = Arc::new(CardPublisher::new(
            api.clone(),
            repository.clone(),
            repository,
            [profile()],
        ));
        let handles = (0..8)
            .map(|_| {
                let publisher = publisher.clone();
                std::thread::spawn(move || publisher.publish(request()))
            })
            .collect::<Vec<_>>();
        let mut created = 0;
        for handle in handles {
            let result = handle
                .join()
                .map_err(|_| schema_error("Concurrent publish worker panicked."))??;
            created += usize::from(result.outcome == CreateCardOutcomeV1::Created);
        }
        assert_eq!(created, 1);
        assert_eq!(api.add_calls(), 1);
        Ok(())
    }
}
