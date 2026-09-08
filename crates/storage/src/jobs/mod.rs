use contracts::{AppErrorV1, JobId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::db::{Storage, storage_error, unix_timestamp};

const MAX_JOB_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_TERMINAL_ERROR_BYTES: usize = 256 * 1024;
const MAX_RECOVERY_JOBS: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStage {
    Queued,
    Validated,
    Running,
    Extracting,
    AssetsReady,
    MediaUploaded,
    PublishingNote,
    NoteCreated,
    Confirmed,
    Completed,
    Cancelling,
    Cancelled,
    Failed,
}

impl JobStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Validated => "validated",
            Self::Running => "running",
            Self::Extracting => "extracting",
            Self::AssetsReady => "assets_ready",
            Self::MediaUploaded => "media_uploaded",
            Self::PublishingNote => "publishing_note",
            Self::NoteCreated => "note_created",
            Self::Confirmed => "confirmed",
            Self::Completed => "completed",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, AppErrorV1> {
        match value {
            "queued" => Ok(Self::Queued),
            "validated" => Ok(Self::Validated),
            "running" => Ok(Self::Running),
            "extracting" => Ok(Self::Extracting),
            "assets_ready" => Ok(Self::AssetsReady),
            "media_uploaded" => Ok(Self::MediaUploaded),
            "publishing_note" => Ok(Self::PublishingNote),
            "note_created" => Ok(Self::NoteCreated),
            "confirmed" => Ok(Self::Confirmed),
            "completed" => Ok(Self::Completed),
            "cancelling" => Ok(Self::Cancelling),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => Err(storage_error("Durable job stage was invalid.")),
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Confirmed | Self::Completed | Self::Cancelled | Self::Failed
        )
    }

    fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Queued => matches!(
                next,
                Self::Validated
                    | Self::Running
                    | Self::Extracting
                    | Self::Cancelling
                    | Self::Cancelled
                    | Self::Failed
            ),
            Self::Validated => matches!(
                next,
                Self::Extracting
                    | Self::AssetsReady
                    | Self::Cancelling
                    | Self::Cancelled
                    | Self::Failed
            ),
            Self::Running => matches!(
                next,
                Self::Completed | Self::Cancelling | Self::Cancelled | Self::Failed
            ),
            Self::Extracting => matches!(
                next,
                Self::AssetsReady | Self::Cancelling | Self::Cancelled | Self::Failed
            ),
            Self::AssetsReady => matches!(
                next,
                Self::MediaUploaded
                    | Self::NoteCreated
                    | Self::Confirmed
                    | Self::Cancelling
                    | Self::Cancelled
                    | Self::Failed
            ),
            Self::MediaUploaded => matches!(
                next,
                Self::PublishingNote
                    | Self::NoteCreated
                    | Self::Cancelling
                    | Self::Cancelled
                    | Self::Failed
            ),
            Self::PublishingNote => matches!(next, Self::NoteCreated | Self::Failed),
            Self::NoteCreated => matches!(next, Self::Confirmed | Self::Failed),
            Self::Cancelling => matches!(next, Self::Cancelled | Self::Failed),
            Self::Confirmed | Self::Completed | Self::Cancelled | Self::Failed => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableJob {
    pub job_id: JobId,
    pub kind: String,
    pub stage: String,
    pub payload_json: String,
    pub terminal_error_json: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelledPublishJob {
    pub job_id: JobId,
    pub payload_json: String,
    pub mining_id: String,
    pub asset_hashes: Vec<String>,
    pub cleanup_error_json: Option<String>,
}

impl DurableJob {
    pub fn stage(&self) -> Result<JobStage, AppErrorV1> {
        JobStage::parse(&self.stage)
    }
}

impl Storage {
    pub fn upsert_job(&self, job: &DurableJob) -> Result<(), AppErrorV1> {
        validate_job(job)?;
        let next_stage = job.stage()?;
        let now = unix_timestamp();
        let mut connection = self.connection();
        // This transaction reads the current stage before updating it. Reserve the
        // WAL writer slot before taking that snapshot so a competing publisher
        // cannot make the deferred read-to-write upgrade fail with SQLITE_BUSY.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current: Option<(String, i64, i64)> = transaction
            .query_row(
                "SELECT stage, attempt_count, created_at FROM jobs WHERE job_id = ?1",
                [job.job_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some((current_stage, attempt_count, created_at)) = current {
            let current_stage = JobStage::parse(&current_stage)?;
            if !current_stage.can_transition_to(next_stage) {
                return Err(storage_error(format!(
                    "Durable job cannot move from {} to {}.",
                    current_stage.as_str(),
                    next_stage.as_str()
                )));
            }
            let next_attempt = if current_stage == next_stage {
                attempt_count
            } else {
                attempt_count.saturating_add(1)
            };
            transaction
                .execute(
                    "UPDATE jobs SET kind=?2, stage=?3, payload_json=?4, terminal_error_json=?5, updated_at=?6, attempt_count=?7, created_at=?8 WHERE job_id=?1",
                    params![job.job_id.as_str(), job.kind, job.stage, job.payload_json, job.terminal_error_json, now, next_attempt, created_at],
                )
                .map_err(storage_error)?;
        } else {
            if !matches!(next_stage, JobStage::Queued | JobStage::Validated) {
                return Err(storage_error(
                    "A durable job must begin in the queued or validated stage.",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO jobs(job_id, kind, stage, payload_json, terminal_error_json, updated_at, attempt_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?6)",
                    params![job.job_id.as_str(), job.kind, job.stage, job.payload_json, job.terminal_error_json, now],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn job(&self, job_id: &JobId) -> Result<Option<DurableJob>, AppErrorV1> {
        self.connection()
            .query_row(
                "SELECT job_id, kind, stage, payload_json, terminal_error_json FROM jobs WHERE job_id = ?1",
                [job_id.as_str()],
                row_to_job,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn recoverable_jobs(&self, limit: usize) -> Result<Vec<DurableJob>, AppErrorV1> {
        let bounded_limit = limit.min(MAX_RECOVERY_JOBS);
        if bounded_limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT job_id, kind, stage, payload_json, terminal_error_json
                 FROM jobs
                 WHERE stage NOT IN ('confirmed', 'completed', 'cancelled', 'failed')
                 ORDER BY updated_at ASC, job_id ASC
                 LIMIT ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([bounded_limit as i64], row_to_job)
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    pub fn failed_jobs(&self, limit: usize) -> Result<Vec<DurableJob>, AppErrorV1> {
        let bounded_limit = limit.min(MAX_RECOVERY_JOBS);
        if bounded_limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT job_id, kind, stage, payload_json, terminal_error_json
                 FROM jobs WHERE stage='failed'
                 ORDER BY updated_at ASC, job_id ASC LIMIT ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([bounded_limit as i64], row_to_job)
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    /// Atomically accepts cancellation only while note creation can still be prevented.
    pub fn cancel_publish_job(&self, job_id: &JobId) -> Result<bool, AppErrorV1> {
        let changed = self
            .connection()
            .execute(
                "UPDATE jobs SET stage='cancelled', updated_at=?2, attempt_count=attempt_count+1
                 WHERE job_id=?1 AND kind='card_publish'
                   AND stage IN ('validated', 'extracting', 'assets_ready', 'media_uploaded')",
                params![job_id.as_str(), unix_timestamp()],
            )
            .map_err(storage_error)?;
        Ok(changed == 1)
    }

    pub fn publish_job_cancelled(&self, job_id: &JobId) -> Result<bool, AppErrorV1> {
        Ok(self
            .job(job_id)?
            .is_some_and(|job| job.stage == JobStage::Cancelled.as_str()))
    }

    /// Cancels every eligible publish job for the session at one SQLite linearization point.
    pub fn cancel_publish_jobs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<CancelledPublishJob>, AppErrorV1> {
        if session_id.is_empty() || session_id.len() > 128 {
            return Err(storage_error("Media session identity was invalid."));
        }
        let now = unix_timestamp();
        let mut connection = self.connection();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let rows = {
            let mut statement = transaction
                .prepare(
                    "SELECT job.job_id, job.payload_json, COALESCE(attempt.mining_id, ''),
                            COALESCE(attempt.asset_hashes_json, '[]'),
                            attempt.terminal_error_json
                     FROM jobs AS job
                     LEFT JOIN publish_attempts AS attempt ON attempt.job_id=job.job_id
                     WHERE job.kind='card_publish'
                       AND job.stage IN ('validated', 'extracting', 'assets_ready', 'media_uploaded')
                       AND CASE WHEN json_valid(job.payload_json)
                           THEN json_extract(job.payload_json, '$.session_id') END = ?1
                     ORDER BY job.job_id",
                )
                .map_err(storage_error)?;
            let mapped = statement
                .query_map([session_id], |row| {
                    let hashes_json: String = row.get(3)?;
                    let asset_hashes = serde_json::from_str(&hashes_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(CancelledPublishJob {
                        job_id: JobId::new(row.get::<_, String>(0)?),
                        payload_json: row.get(1)?,
                        mining_id: row.get(2)?,
                        asset_hashes,
                        cleanup_error_json: row.get(4)?,
                    })
                })
                .map_err(storage_error)?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let changed = transaction
            .execute(
                "UPDATE jobs SET stage='cancelled', updated_at=?2, attempt_count=attempt_count+1
                 WHERE kind='card_publish'
                   AND stage IN ('validated', 'extracting', 'assets_ready', 'media_uploaded')
                   AND CASE WHEN json_valid(payload_json)
                       THEN json_extract(payload_json, '$.session_id') END = ?1",
                params![session_id, now],
            )
            .map_err(storage_error)?;
        if changed != rows.len() {
            return Err(storage_error(
                "Publish cancellation did not update its complete transactional snapshot.",
            ));
        }
        transaction.commit().map_err(storage_error)?;
        Ok(rows)
    }

    /// Restarts a user-cancelled request only when explicitly invoked by a fresh publish action.
    pub fn restart_cancelled_publish_job(&self, job_id: &JobId) -> Result<bool, AppErrorV1> {
        let now = unix_timestamp();
        let mut connection = self.connection();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET stage='validated', updated_at=?2,
                        attempt_count=attempt_count+1, terminal_error_json=NULL
                 WHERE job_id=?1 AND kind='card_publish' AND stage='cancelled'
                   AND NOT EXISTS (
                     SELECT 1 FROM publish_attempts
                     WHERE publish_attempts.job_id=jobs.job_id
                       AND publish_attempts.asset_hashes_json != '[]'
                   )",
                params![job_id.as_str(), now],
            )
            .map_err(storage_error)?;
        if changed == 1 {
            transaction
                .execute(
                    "DELETE FROM publish_attempts WHERE job_id=?1",
                    [job_id.as_str()],
                )
                .map_err(storage_error)?;
            transaction
                .execute("DELETE FROM publish_claims WHERE mining_id IN (SELECT json_extract(payload_json, '$.mining_id') FROM jobs WHERE job_id=?1 AND json_valid(payload_json))", [job_id.as_str()])
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(changed == 1)
    }

    pub fn pending_cancelled_publish_cleanups(
        &self,
        limit: usize,
    ) -> Result<Vec<CancelledPublishJob>, AppErrorV1> {
        let bounded_limit = limit.min(MAX_RECOVERY_JOBS);
        if bounded_limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT job.job_id, job.payload_json, attempt.mining_id,
                        attempt.asset_hashes_json,
                        attempt.terminal_error_json
                 FROM jobs AS job
                 INNER JOIN publish_attempts AS attempt ON attempt.job_id=job.job_id
                 WHERE job.kind='card_publish' AND job.stage='cancelled'
                   AND attempt.asset_hashes_json != '[]'
                 ORDER BY attempt.updated_at ASC, job.job_id ASC LIMIT ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([bounded_limit as i64], cancelled_publish_job_from_row)
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    pub fn cancelled_publish_cleanup(
        &self,
        job_id: &JobId,
    ) -> Result<Option<CancelledPublishJob>, AppErrorV1> {
        self.connection()
            .query_row(
                "SELECT job.job_id, job.payload_json, attempt.mining_id,
                        attempt.asset_hashes_json,
                        attempt.terminal_error_json
                 FROM jobs AS job
                 INNER JOIN publish_attempts AS attempt ON attempt.job_id=job.job_id
                 WHERE job.job_id=?1 AND job.kind='card_publish' AND job.stage='cancelled'
                   AND attempt.asset_hashes_json != '[]'",
                [job_id.as_str()],
                cancelled_publish_job_from_row,
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn cancelled_publish_cleanup_count(&self) -> Result<usize, AppErrorV1> {
        let count: i64 = self
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM jobs AS job
                 INNER JOIN publish_attempts AS attempt ON attempt.job_id=job.job_id
                 WHERE job.kind='card_publish' AND job.stage='cancelled'
                   AND attempt.asset_hashes_json != '[]'",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        usize::try_from(count).map_err(storage_error)
    }

    pub fn acknowledge_cancelled_publish_cleanup(
        &self,
        job_id: &JobId,
    ) -> Result<bool, AppErrorV1> {
        let changed = self
            .connection()
            .execute(
                "UPDATE publish_attempts SET asset_hashes_json='[]',
                        terminal_error_json=NULL, updated_at=?2
                 WHERE job_id=?1 AND EXISTS (
                   SELECT 1 FROM jobs WHERE jobs.job_id=publish_attempts.job_id
                     AND jobs.kind='card_publish' AND jobs.stage='cancelled'
                 )",
                params![job_id.as_str(), unix_timestamp()],
            )
            .map_err(storage_error)?;
        Ok(changed == 1)
    }

    pub fn record_cancelled_publish_cleanup_error(
        &self,
        job_id: &JobId,
        error_json: &str,
    ) -> Result<bool, AppErrorV1> {
        validate_json(
            error_json,
            MAX_TERMINAL_ERROR_BYTES,
            "cancelled publish cleanup error",
        )?;
        let changed = self
            .connection()
            .execute(
                "UPDATE publish_attempts SET terminal_error_json=?2,
                        updated_at=MAX(?3, updated_at + 1)
                 WHERE job_id=?1 AND EXISTS (
                   SELECT 1 FROM jobs WHERE jobs.job_id=publish_attempts.job_id
                     AND jobs.kind='card_publish' AND jobs.stage='cancelled'
                 )",
                params![job_id.as_str(), error_json, unix_timestamp()],
            )
            .map_err(storage_error)?;
        Ok(changed == 1)
    }
}

fn cancelled_publish_job_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CancelledPublishJob> {
    let hashes_json: String = row.get(3)?;
    let asset_hashes = serde_json::from_str(&hashes_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(CancelledPublishJob {
        job_id: JobId::new(row.get::<_, String>(0)?),
        payload_json: row.get(1)?,
        mining_id: row.get(2)?,
        asset_hashes,
        cleanup_error_json: row.get(4)?,
    })
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableJob> {
    Ok(DurableJob {
        job_id: JobId::new(row.get::<_, String>(0)?),
        kind: row.get(1)?,
        stage: row.get(2)?,
        payload_json: row.get(3)?,
        terminal_error_json: row.get(4)?,
    })
}

fn validate_job(job: &DurableJob) -> Result<(), AppErrorV1> {
    if job.job_id.as_str().is_empty()
        || job.job_id.as_str().len() > 128
        || job.kind.is_empty()
        || job.kind.len() > 64
        || !job
            .kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(storage_error("Durable job identity was invalid."));
    }
    job.stage()?;
    validate_json(&job.payload_json, MAX_JOB_PAYLOAD_BYTES, "job payload")?;
    if let Some(error) = &job.terminal_error_json {
        validate_json(error, MAX_TERMINAL_ERROR_BYTES, "terminal error")?;
    }
    match (job.stage()?, job.terminal_error_json.is_some()) {
        (JobStage::Failed, false) => {
            return Err(storage_error(
                "A failed durable job must include a terminal error.",
            ));
        }
        (JobStage::Failed, true) | (_, false) => {}
        (_, true) => {
            return Err(storage_error(
                "Only a failed durable job may include a terminal error.",
            ));
        }
    }
    Ok(())
}

fn validate_json(value: &str, max_bytes: usize, label: &str) -> Result<(), AppErrorV1> {
    if value.len() > max_bytes {
        return Err(storage_error(format!("Durable {label} was too large.")));
    }
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(stage: JobStage) -> DurableJob {
        DurableJob {
            job_id: JobId::new("job"),
            kind: "card_publish".into(),
            stage: stage.as_str().into(),
            payload_json: "{}".into(),
            terminal_error_json: (stage == JobStage::Failed).then(|| "{}".into()),
        }
    }

    #[test]
    fn transition_rules_prevent_regression_and_recover_nonterminal_jobs() -> Result<(), AppErrorV1>
    {
        let storage = Storage::in_memory()?;
        storage.upsert_job(&job(JobStage::Queued))?;
        storage.upsert_job(&job(JobStage::Validated))?;
        storage.upsert_job(&job(JobStage::AssetsReady))?;
        assert!(storage.upsert_job(&job(JobStage::Validated)).is_err());
        assert_eq!(storage.recoverable_jobs(10)?.len(), 1);
        storage.upsert_job(&job(JobStage::Confirmed))?;
        assert!(storage.recoverable_jobs(10)?.is_empty());
        Ok(())
    }

    #[test]
    fn rejects_invalid_json_and_failed_job_without_error() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let mut invalid = job(JobStage::Queued);
        invalid.payload_json = "not-json".into();
        assert!(storage.upsert_job(&invalid).is_err());
        let mut failed = job(JobStage::Failed);
        failed.terminal_error_json = None;
        assert!(storage.upsert_job(&failed).is_err());
        Ok(())
    }

    #[test]
    fn accepted_publish_cancellation_is_terminal_and_not_recovered() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(storage_error)?;
        let database = temporary.path().join("cancelled.sqlite3");
        let queued = job(JobStage::Validated);
        {
            let storage = Storage::open(&database)?;
            storage.upsert_job(&queued)?;
            assert!(storage.cancel_publish_job(&queued.job_id)?);
            assert!(storage.publish_job_cancelled(&queued.job_id)?);
            assert!(!storage.cancel_publish_job(&queued.job_id)?);
            assert!(storage.recoverable_jobs(10)?.is_empty());
        }
        let reopened = Storage::open(&database)?;
        assert!(reopened.publish_job_cancelled(&queued.job_id)?);
        assert!(reopened.recoverable_jobs(10)?.is_empty());
        assert!(reopened.restart_cancelled_publish_job(&queued.job_id)?);
        assert!(!reopened.publish_job_cancelled(&queued.job_id)?);
        assert_eq!(reopened.recoverable_jobs(10)?.len(), 1);
        Ok(())
    }

    #[test]
    fn session_cancellation_has_no_recovery_limit_and_is_linearized() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let total = MAX_RECOVERY_JOBS + 5;
        for index in 0..total {
            storage.upsert_job(&DurableJob {
                job_id: JobId::new(format!("bulk-{index}")),
                kind: "card_publish".into(),
                stage: JobStage::Validated.as_str().into(),
                payload_json: serde_json::json!({
                    "session_id": "bulk-session",
                    "mining_id": format!("mining-{index}")
                })
                .to_string(),
                terminal_error_json: None,
            })?;
        }
        let cancelled = storage.cancel_publish_jobs_for_session("bulk-session")?;
        assert_eq!(cancelled.len(), total);
        assert!(storage.recoverable_jobs(MAX_RECOVERY_JOBS)?.is_empty());

        let after_linearization = DurableJob {
            job_id: JobId::new("inserted-after-cancel"),
            kind: "card_publish".into(),
            stage: JobStage::Validated.as_str().into(),
            payload_json: serde_json::json!({
                "session_id": "bulk-session",
                "mining_id": "inserted-after"
            })
            .to_string(),
            terminal_error_json: None,
        };
        storage.upsert_job(&after_linearization)?;
        assert_eq!(storage.recoverable_jobs(10)?, vec![after_linearization]);
        Ok(())
    }
}
