use contracts::{AppErrorV1, CardDraftV1, DraftId, JobId};
use ports::{DraftRepositoryPort, PublishHistoryPort};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::db::{Storage, storage_error, unix_timestamp};

const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ASSET_HASHES: usize = 32;
const MAX_PENDING_ATTEMPTS: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishStage {
    Validated,
    AssetsReady,
    MediaUploaded,
    NoteCreated,
    Confirmed,
    Failed,
}

impl PublishStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validated => "validated",
            Self::AssetsReady => "assets_ready",
            Self::MediaUploaded => "media_uploaded",
            Self::NoteCreated => "note_created",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, AppErrorV1> {
        match value {
            "validated" => Ok(Self::Validated),
            "assets_ready" => Ok(Self::AssetsReady),
            "media_uploaded" => Ok(Self::MediaUploaded),
            "note_created" => Ok(Self::NoteCreated),
            "confirmed" => Ok(Self::Confirmed),
            "failed" => Ok(Self::Failed),
            _ => Err(storage_error("Publish stage was invalid.")),
        }
    }

    fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Validated, Self::AssetsReady | Self::Failed)
                    | (
                        Self::AssetsReady,
                        Self::MediaUploaded | Self::NoteCreated | Self::Failed
                    )
                    | (Self::MediaUploaded, Self::NoteCreated | Self::Failed)
                    | (Self::NoteCreated, Self::Confirmed)
            )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishAttempt {
    pub mining_id: String,
    pub job_id: JobId,
    pub stage: PublishStage,
    pub note_id: Option<i64>,
    pub asset_hashes: Vec<String>,
    pub terminal_error_json: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublishStageTimes {
    pub media_uploaded_at: Option<i64>,
    pub note_created_at: Option<i64>,
    pub confirmed_at: Option<i64>,
}

impl DraftRepositoryPort for Storage {
    fn insert_draft(&self, draft: &CardDraftV1) -> Result<(), AppErrorV1> {
        let snapshot = serde_json::to_string(draft).map_err(storage_error)?;
        if snapshot.len() > MAX_DRAFT_BYTES || draft.revision == 0 {
            return Err(storage_error(
                "Immutable card draft exceeded its safety limits.",
            ));
        }
        let revision = i64::try_from(draft.revision)
            .map_err(|_| storage_error("Draft revision exceeded SQLite integer bounds."))?;
        self.connection().execute(
            "INSERT INTO drafts(draft_id, revision, snapshot_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![draft.draft_id.as_str(), revision, snapshot, unix_timestamp()],
        ).map_err(storage_error)?;
        Ok(())
    }

    fn get_draft(&self, draft_id: &DraftId) -> Result<Option<CardDraftV1>, AppErrorV1> {
        let json: Option<String> = self
            .connection()
            .query_row(
                "SELECT snapshot_json FROM drafts WHERE draft_id = ?1",
                [draft_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        json.map(|value| serde_json::from_str(&value).map_err(storage_error))
            .transpose()
    }
}

impl PublishHistoryPort for Storage {
    fn note_for_mining_id(&self, mining_id: &str) -> Result<Option<i64>, AppErrorV1> {
        self.connection()
            .query_row(
                "SELECT note_id FROM mining_history WHERE mining_id = ?1",
                [mining_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)
    }

    fn record_note(&self, mining_id: &str, note_id: i64) -> Result<(), AppErrorV1> {
        validate_mining_id(mining_id)?;
        if note_id <= 0 {
            return Err(storage_error("Anki note ID must be positive."));
        }
        let connection = self.connection();
        let existing: Option<i64> = connection
            .query_row(
                "SELECT note_id FROM mining_history WHERE mining_id=?1",
                [mining_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            if existing != note_id {
                return Err(storage_error(
                    "Mining history already points to a different Anki note.",
                ));
            }
            return Ok(());
        }
        connection
            .execute(
                "INSERT INTO mining_history(mining_id, note_id, asset_hashes_json, confirmed_at) VALUES (?1, ?2, '[]', ?3)",
                params![mining_id, note_id, unix_timestamp()],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}

impl Storage {
    pub fn clear_failed_publish_attempt(&self, mining_id: &str) -> Result<bool, AppErrorV1> {
        validate_mining_id(mining_id)?;
        let connection = self.connection();
        let stage: Option<String> = connection
            .query_row(
                "SELECT stage FROM publish_attempts WHERE mining_id=?1",
                [mining_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        match stage.as_deref() {
            None => Ok(false),
            Some("failed") => connection
                .execute(
                    "DELETE FROM publish_attempts WHERE mining_id=?1 AND stage='failed'",
                    [mining_id],
                )
                .map(|changed| changed > 0)
                .map_err(storage_error),
            Some(_) => Err(storage_error(
                "Only a terminally failed publish attempt can be cleared.",
            )),
        }
    }

    pub fn record_publish_stage(&self, attempt: &PublishAttempt) -> Result<(), AppErrorV1> {
        validate_publish_attempt(attempt)?;
        let asset_hashes_json =
            serde_json::to_string(&attempt.asset_hashes).map_err(storage_error)?;
        let now = unix_timestamp();
        let mut connection = self.connection();
        // Reserve the WAL writer slot before reading the current stage. A
        // deferred transaction could otherwise lose its snapshot-upgrade race
        // to another process polling the publish claim and fail immediately.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current_stage: Option<(String, String)> = transaction
            .query_row(
                "SELECT stage, job_id FROM publish_attempts WHERE mining_id = ?1",
                [&attempt.mining_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some((current_stage, current_job_id)) = current_stage {
            if current_job_id != attempt.job_id.as_str() {
                return Err(storage_error(
                    "Publish attempt job identity cannot change after creation.",
                ));
            }
            let current_stage = PublishStage::parse(&current_stage)?;
            if !current_stage.can_transition_to(attempt.stage) {
                return Err(storage_error(format!(
                    "Publish attempt cannot move from {} to {}.",
                    current_stage.as_str(),
                    attempt.stage.as_str()
                )));
            }
            transaction
                .execute(
                    "UPDATE publish_attempts
                     SET stage=?2, note_id=?3, asset_hashes_json=?4, terminal_error_json=?5,
                         updated_at=?6,
                         media_uploaded_at=CASE WHEN ?2='media_uploaded' THEN COALESCE(media_uploaded_at, ?6) ELSE media_uploaded_at END,
                         note_created_at=CASE WHEN ?2='note_created' THEN COALESCE(note_created_at, ?6) ELSE note_created_at END,
                         confirmed_at=CASE WHEN ?2='confirmed' THEN COALESCE(confirmed_at, ?6) ELSE confirmed_at END
                     WHERE mining_id=?1",
                    params![attempt.mining_id, attempt.stage.as_str(), attempt.note_id, asset_hashes_json, attempt.terminal_error_json, now],
                )
                .map_err(storage_error)?;
        } else {
            if attempt.stage != PublishStage::Validated {
                return Err(storage_error(
                    "A durable publish attempt must begin in the validated stage.",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO publish_attempts(mining_id, job_id, stage, note_id, asset_hashes_json, terminal_error_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![attempt.mining_id, attempt.job_id.as_str(), attempt.stage.as_str(), attempt.note_id, asset_hashes_json, attempt.terminal_error_json, now],
                )
                .map_err(storage_error)?;
        }
        if attempt.stage == PublishStage::Confirmed {
            let note_id = attempt
                .note_id
                .ok_or_else(|| storage_error("Confirmed publish attempt omitted its note ID."))?;
            let existing_note: Option<i64> = transaction
                .query_row(
                    "SELECT note_id FROM mining_history WHERE mining_id=?1",
                    [&attempt.mining_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            if existing_note.is_some_and(|existing| existing != note_id) {
                return Err(storage_error(
                    "Mining history already points to a different Anki note.",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO mining_history(mining_id, note_id, asset_hashes_json, confirmed_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(mining_id) DO UPDATE SET asset_hashes_json=excluded.asset_hashes_json, confirmed_at=excluded.confirmed_at",
                    params![attempt.mining_id, note_id, asset_hashes_json, now],
                )
                .map_err(storage_error)?;
            for hash in &attempt.asset_hashes {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO asset_references(owner_kind, owner_id, asset_hash, created_at) VALUES ('history', ?1, ?2, ?3)",
                        params![attempt.mining_id, hash, now],
                    )
                    .map_err(storage_error)?;
            }
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn publish_attempt(&self, mining_id: &str) -> Result<Option<PublishAttempt>, AppErrorV1> {
        validate_mining_id(mining_id)?;
        self.connection()
            .query_row(
                "SELECT mining_id, job_id, stage, note_id, asset_hashes_json, terminal_error_json FROM publish_attempts WHERE mining_id=?1",
                [mining_id],
                publish_attempt_from_row,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn pending_publish_attempts(
        &self,
        limit: usize,
    ) -> Result<Vec<PublishAttempt>, AppErrorV1> {
        let limit = limit.min(MAX_PENDING_ATTEMPTS);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT attempt.mining_id, attempt.job_id, attempt.stage, attempt.note_id,
                        attempt.asset_hashes_json, attempt.terminal_error_json
                 FROM publish_attempts AS attempt
                 LEFT JOIN jobs AS job ON job.job_id=attempt.job_id
                 WHERE attempt.stage NOT IN ('confirmed', 'failed')
                   AND (job.stage IS NULL OR job.stage != 'cancelled')
                 ORDER BY attempt.updated_at ASC, attempt.mining_id ASC LIMIT ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([limit as i64], publish_attempt_from_row)
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    pub fn asset_hashes_for_mining_id(&self, mining_id: &str) -> Result<Vec<String>, AppErrorV1> {
        validate_mining_id(mining_id)?;
        let json: Option<String> = self
            .connection()
            .query_row(
                "SELECT asset_hashes_json FROM mining_history WHERE mining_id=?1",
                [mining_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        json.map(|value| serde_json::from_str(&value).map_err(storage_error))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn publish_stage_times(
        &self,
        mining_id: &str,
    ) -> Result<Option<PublishStageTimes>, AppErrorV1> {
        validate_mining_id(mining_id)?;
        self.connection()
            .query_row(
                "SELECT media_uploaded_at, note_created_at, confirmed_at
                 FROM publish_attempts WHERE mining_id=?1",
                [mining_id],
                |row| {
                    Ok(PublishStageTimes {
                        media_uploaded_at: row.get(0)?,
                        note_created_at: row.get(1)?,
                        confirmed_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    /// Claims one mining identity across threads, coordinator instances, and processes.
    pub fn try_acquire_publish_claim(
        &self,
        mining_id: &str,
        owner_token: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppErrorV1> {
        validate_mining_id(mining_id)?;
        validate_owner_token(owner_token)?;
        if !(1..=3_600).contains(&lease_seconds) {
            return Err(storage_error("Publish claim lease duration was invalid."));
        }
        let now = unix_timestamp();
        let expires = now.saturating_add(lease_seconds);
        let changed = self
            .connection()
            .execute(
                "INSERT INTO publish_claims(mining_id, owner_token, lease_expires_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(mining_id) DO UPDATE SET
                   owner_token=excluded.owner_token,
                   lease_expires_at=excluded.lease_expires_at,
                   updated_at=excluded.updated_at
                 WHERE publish_claims.owner_token=excluded.owner_token
                    OR publish_claims.lease_expires_at < excluded.updated_at",
                params![mining_id, owner_token, expires, now],
            )
            .map_err(storage_error)?;
        Ok(changed == 1)
    }

    /// Renews a claim and atomically crosses the point after which cancellation is refused.
    pub fn begin_note_publish(
        &self,
        mining_id: &str,
        job_id: &JobId,
        owner_token: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppErrorV1> {
        validate_mining_id(mining_id)?;
        validate_owner_token(owner_token)?;
        if !(1..=3_600).contains(&lease_seconds) {
            return Err(storage_error("Publish claim lease duration was invalid."));
        }
        let now = unix_timestamp();
        let expires = now.saturating_add(lease_seconds);
        let mut connection = self.connection();
        // Claim ownership and the cancellation boundary are one read/write
        // decision. Acquire the WAL writer reservation before reading either so
        // the transaction cannot fail while upgrading a stale snapshot.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let owns_claim = transaction
            .query_row(
                "SELECT 1 FROM publish_claims
                 WHERE mining_id=?1 AND owner_token=?2 AND lease_expires_at>=?3",
                params![mining_id, owner_token, now],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_error)?
            .is_some();
        if !owns_claim {
            return Ok(false);
        }
        let changed = transaction
            .execute(
                "UPDATE jobs SET stage='publishing_note', updated_at=?2, attempt_count=attempt_count+1
                 WHERE job_id=?1 AND kind='card_publish'
                   AND stage IN ('media_uploaded', 'publishing_note')",
                params![job_id.as_str(), now],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Ok(false);
        }
        transaction
            .execute(
                "UPDATE publish_claims SET lease_expires_at=?3, updated_at=?4
                 WHERE mining_id=?1 AND owner_token=?2",
                params![mining_id, owner_token, expires, now],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    pub fn renew_publish_claim(
        &self,
        mining_id: &str,
        owner_token: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppErrorV1> {
        validate_mining_id(mining_id)?;
        validate_owner_token(owner_token)?;
        if !(1..=3_600).contains(&lease_seconds) {
            return Err(storage_error("Publish claim lease duration was invalid."));
        }
        let now = unix_timestamp();
        let changed = self
            .connection()
            .execute(
                "UPDATE publish_claims SET lease_expires_at=?3, updated_at=?4
                 WHERE mining_id=?1 AND owner_token=?2 AND lease_expires_at>=?4",
                params![
                    mining_id,
                    owner_token,
                    now.saturating_add(lease_seconds),
                    now
                ],
            )
            .map_err(storage_error)?;
        Ok(changed == 1)
    }

    pub fn release_publish_claim(
        &self,
        mining_id: &str,
        owner_token: &str,
    ) -> Result<bool, AppErrorV1> {
        validate_mining_id(mining_id)?;
        validate_owner_token(owner_token)?;
        self.connection()
            .execute(
                "DELETE FROM publish_claims WHERE mining_id=?1 AND owner_token=?2",
                params![mining_id, owner_token],
            )
            .map(|changed| changed == 1)
            .map_err(storage_error)
    }
}

fn publish_attempt_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishAttempt> {
    let stage: String = row.get(2)?;
    let asset_hashes_json: String = row.get(4)?;
    let parsed_stage = PublishStage::parse(&stage).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let asset_hashes = serde_json::from_str(&asset_hashes_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(PublishAttempt {
        mining_id: row.get(0)?,
        job_id: JobId::new(row.get::<_, String>(1)?),
        stage: parsed_stage,
        note_id: row.get(3)?,
        asset_hashes,
        terminal_error_json: row.get(5)?,
    })
}

fn validate_publish_attempt(attempt: &PublishAttempt) -> Result<(), AppErrorV1> {
    validate_mining_id(&attempt.mining_id)?;
    if attempt.job_id.as_str().is_empty() || attempt.job_id.as_str().len() > 128 {
        return Err(storage_error("Publish job ID was invalid."));
    }
    if attempt.asset_hashes.len() > MAX_ASSET_HASHES {
        return Err(storage_error("Publish attempt referenced too many assets."));
    }
    for hash in &attempt.asset_hashes {
        validate_asset_hash(hash)?;
    }
    match attempt.stage {
        PublishStage::NoteCreated | PublishStage::Confirmed if attempt.note_id.is_none() => {
            return Err(storage_error(
                "A note-created publish stage requires a note ID.",
            ));
        }
        PublishStage::Validated | PublishStage::AssetsReady | PublishStage::MediaUploaded
            if attempt.note_id.is_some() =>
        {
            return Err(storage_error(
                "A pre-note publish stage cannot contain a note ID.",
            ));
        }
        _ => {}
    }
    if attempt.stage == PublishStage::Failed && attempt.terminal_error_json.is_none() {
        return Err(storage_error(
            "A failed publish attempt requires a terminal error.",
        ));
    }
    if let Some(error) = &attempt.terminal_error_json {
        if error.len() > 256 * 1024 {
            return Err(storage_error("Publish error payload was too large."));
        }
        serde_json::from_str::<serde_json::Value>(error).map_err(storage_error)?;
    }
    Ok(())
}

fn validate_mining_id(mining_id: &str) -> Result<(), AppErrorV1> {
    if mining_id.is_empty()
        || mining_id.len() > 128
        || !mining_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(storage_error("Mining ID was invalid."));
    }
    Ok(())
}

fn validate_asset_hash(hash: &str) -> Result<(), AppErrorV1> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(storage_error("Mining asset hash was invalid."));
    }
    Ok(())
}

fn validate_owner_token(owner_token: &str) -> Result<(), AppErrorV1> {
    if owner_token.is_empty()
        || owner_token.len() > 128
        || !owner_token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(storage_error("Publish claim owner token was invalid."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use contracts::{
        CueId, MediaSessionId, SubtitleCueV1, SubtitleSourceId, SubtitleStyleHintV1, TokenId,
        TokenV1,
    };

    use super::*;

    #[test]
    fn draft_snapshots_and_history_round_trip() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let draft = CardDraftV1 {
            draft_id: DraftId::new("draft"),
            revision: 1,
            session_id: MediaSessionId::new("session"),
            subtitle_source_id: SubtitleSourceId::new("sub"),
            subtitle_source_version: "v1".into(),
            cue: SubtitleCueV1 {
                cue_id: CueId::new("cue"),
                start_us: 1,
                end_us: 2,
                plain_text: "見る".into(),
                source_text: "見る".into(),
                track_order: 0,
                style_hint: SubtitleStyleHintV1::default(),
            },
            token: TokenV1 {
                token_id: TokenId::new("token"),
                surface: "見".into(),
                byte_start: 0,
                byte_end: 3,
                lemma: "見る".into(),
                reading: "ミル".into(),
                pronunciation: None,
                part_of_speech: vec!["verb".into()],
                lookup_candidate: true,
            },
            dictionary_entry: None,
            observed_source_time_us: 1,
            source_fingerprint: "fp".into(),
        };
        storage.insert_draft(&draft)?;
        assert_eq!(storage.get_draft(&draft.draft_id)?, Some(draft));
        storage.record_note("mining", 42)?;
        assert_eq!(storage.note_for_mining_id("mining")?, Some(42));
        Ok(())
    }

    #[test]
    fn publish_stages_recover_after_reopen_and_confirm_atomically() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(storage_error)?;
        let database = temporary.path().join("user.sqlite3");
        let mining_id = "a".repeat(64);
        let asset_hash = "b".repeat(64);
        let mut attempt = PublishAttempt {
            mining_id: mining_id.clone(),
            job_id: JobId::new("publish-job"),
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: Vec::new(),
            terminal_error_json: None,
        };
        {
            let storage = Storage::open(&database)?;
            storage.record_publish_stage(&attempt)?;
            attempt.stage = PublishStage::AssetsReady;
            attempt.asset_hashes = vec![asset_hash.clone()];
            storage.record_publish_stage(&attempt)?;
            assert_eq!(storage.pending_publish_attempts(10)?, vec![attempt.clone()]);
        }

        let storage = Storage::open(&database)?;
        assert_eq!(storage.publish_attempt(&mining_id)?, Some(attempt.clone()));
        attempt.stage = PublishStage::MediaUploaded;
        storage.record_publish_stage(&attempt)?;
        let uploaded_times = storage
            .publish_stage_times(&mining_id)?
            .ok_or_else(|| storage_error("Publish stage timestamps were missing."))?;
        assert!(uploaded_times.media_uploaded_at.is_some());
        assert!(uploaded_times.note_created_at.is_none());
        attempt.stage = PublishStage::NoteCreated;
        attempt.note_id = Some(42);
        storage.record_publish_stage(&attempt)?;
        attempt.stage = PublishStage::Confirmed;
        storage.record_publish_stage(&attempt)?;
        assert_eq!(storage.note_for_mining_id(&mining_id)?, Some(42));
        assert_eq!(
            storage.asset_hashes_for_mining_id(&mining_id)?,
            vec![asset_hash]
        );
        assert!(storage.pending_publish_attempts(10)?.is_empty());
        let times = storage
            .publish_stage_times(&mining_id)?
            .ok_or_else(|| storage_error("Publish stage timestamps were missing."))?;
        assert!(times.note_created_at.is_some());
        assert!(times.confirmed_at.is_some());
        Ok(())
    }

    #[test]
    fn publish_claim_is_exclusive_across_storage_instances() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(storage_error)?;
        let database = temporary.path().join("claims.sqlite3");
        let first = Storage::open(&database)?;
        let second = Storage::open(&database)?;
        let mining_id = "d".repeat(64);
        assert!(first.try_acquire_publish_claim(&mining_id, "owner-one", 60)?);
        assert!(!second.try_acquire_publish_claim(&mining_id, "owner-two", 60)?);
        first
            .connection()
            .execute(
                "UPDATE publish_claims SET lease_expires_at=?2 WHERE mining_id=?1",
                params![mining_id, unix_timestamp().saturating_sub(1)],
            )
            .map_err(storage_error)?;
        assert!(second.try_acquire_publish_claim(&mining_id, "owner-two", 60)?);
        assert!(!first.release_publish_claim(&mining_id, "owner-one")?);
        Ok(())
    }

    #[test]
    fn cancellation_and_note_start_are_one_atomic_decision() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let mining_id = "e".repeat(64);
        let job_id = JobId::new("atomic-publish");
        storage.upsert_job(&crate::jobs::DurableJob {
            job_id: job_id.clone(),
            kind: "card_publish".into(),
            stage: crate::jobs::JobStage::Validated.as_str().into(),
            payload_json: "{}".into(),
            terminal_error_json: None,
        })?;
        let mut job = storage
            .job(&job_id)?
            .ok_or_else(|| storage_error("Publish job was missing."))?;
        job.stage = crate::jobs::JobStage::AssetsReady.as_str().into();
        storage.upsert_job(&job)?;
        job.stage = crate::jobs::JobStage::MediaUploaded.as_str().into();
        storage.upsert_job(&job)?;
        assert!(storage.try_acquire_publish_claim(&mining_id, "owner", 60)?);
        assert!(storage.cancel_publish_job(&job_id)?);
        assert!(!storage.begin_note_publish(&mining_id, &job_id, "owner", 60)?);

        let second_job_id = JobId::new("atomic-publish-two");
        storage.upsert_job(&crate::jobs::DurableJob {
            job_id: second_job_id.clone(),
            kind: "card_publish".into(),
            stage: crate::jobs::JobStage::Validated.as_str().into(),
            payload_json: "{}".into(),
            terminal_error_json: None,
        })?;
        let mut second_job = storage
            .job(&second_job_id)?
            .ok_or_else(|| storage_error("Second publish job was missing."))?;
        second_job.stage = crate::jobs::JobStage::AssetsReady.as_str().into();
        storage.upsert_job(&second_job)?;
        second_job.stage = crate::jobs::JobStage::MediaUploaded.as_str().into();
        storage.upsert_job(&second_job)?;
        let second_mining_id = "f".repeat(64);
        assert!(storage.try_acquire_publish_claim(&second_mining_id, "owner", 60)?);
        assert!(storage.begin_note_publish(&second_mining_id, &second_job_id, "owner", 60)?);
        assert!(!storage.cancel_publish_job(&second_job_id)?);
        Ok(())
    }
}
