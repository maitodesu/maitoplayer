use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anki_connect::{
    AnkiMedia, CardPublisher,
    note::AnkiProfile,
    publish::{PublishBoundary, PublishObserver, mining_id},
};
use app_core::AppCore;
use contracts::{
    AppErrorV1, CardDraftV1, CreateCardRequestV1, CreateCardResultV1, JobId, MediaSessionV1,
    error_codes,
};
use mining_assets::{AssetKind, AssetStore, ClipPolicy, MiningAssetService, StoredAsset};
use ports::DraftRepositoryPort;
use serde::{Deserialize, Serialize};
use storage::{
    Storage,
    jobs::{CancelledPublishJob, DurableJob, JobStage},
    mining::{PublishAttempt, PublishStage},
};

const MAX_PUBLISH_MEDIA_BYTES: u64 = 32 * 1024 * 1024;
const PUBLISH_JOB_KIND: &str = "card_publish";
const PUBLISH_PAYLOAD_VERSION: u32 = 1;
const CLAIM_LEASE_SECONDS: i64 = 180;
const CLAIM_WAIT_ATTEMPTS: usize = 600;
const CLAIM_WAIT_INTERVAL: Duration = Duration::from_millis(25);
const RECOVERY_CLAIM_WAIT_ATTEMPTS: usize = 740;
const RECOVERY_CLAIM_WAIT_INTERVAL: Duration = Duration::from_millis(250);
const CANCELLED_CLEANUP_BATCH_SIZE: usize = 1_000;
const MAX_CANCELLED_CLEANUPS_PER_RECOVERY: usize = 10_000;

static CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ProfileSnapshot {
    profile_id: String,
    deck_name: String,
    model_name: String,
    field_mapping: BTreeMap<String, String>,
    tags: Vec<String>,
}

impl From<&AnkiProfile> for ProfileSnapshot {
    fn from(profile: &AnkiProfile) -> Self {
        Self {
            profile_id: profile.profile_id.clone(),
            deck_name: profile.deck_name.clone(),
            model_name: profile.model_name.clone(),
            field_mapping: profile.field_mapping.clone(),
            tags: profile.tags.clone(),
        }
    }
}

impl From<&ProfileSnapshot> for AnkiProfile {
    fn from(profile: &ProfileSnapshot) -> Self {
        Self {
            profile_id: profile.profile_id.clone(),
            deck_name: profile.deck_name.clone(),
            model_name: profile.model_name.clone(),
            field_mapping: profile.field_mapping.clone(),
            tags: profile.tags.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ClipPolicySnapshot {
    leading_padding_us: i64,
    trailing_padding_us: i64,
    audio_profile: String,
    image_profile: String,
}

impl From<&ClipPolicy> for ClipPolicySnapshot {
    fn from(policy: &ClipPolicy) -> Self {
        Self {
            leading_padding_us: policy.leading_padding_us,
            trailing_padding_us: policy.trailing_padding_us,
            audio_profile: policy.audio_profile.clone(),
            image_profile: policy.image_profile.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct DurablePublishPayload {
    version: u32,
    mining_id: String,
    session_id: contracts::MediaSessionId,
    request: CreateCardRequestV1,
    profile: ProfileSnapshot,
    clip_policy: ClipPolicySnapshot,
}

#[derive(Clone, Debug, Default)]
pub struct RecoveryReport {
    pub attempted: usize,
    pub completed: usize,
    pub errors: Vec<RecoveryIssue>,
}

#[derive(Clone, Debug)]
pub struct RecoveryIssue {
    pub job_id: Option<JobId>,
    pub error: AppErrorV1,
}

impl RecoveryReport {
    pub fn clear_job(&mut self, job_id: &JobId) {
        self.errors
            .retain(|issue| issue.job_id.as_ref() != Some(job_id));
    }

    #[cfg(test)]
    pub fn error_for_jobs(&self, jobs: &[DurableJob]) -> Option<AppErrorV1> {
        self.errors
            .iter()
            .find(|issue| {
                issue
                    .job_id
                    .as_ref()
                    .is_none_or(|job_id| jobs.iter().any(|job| &job.job_id == job_id))
            })
            .map(|issue| issue.error.clone())
    }
}

#[derive(Debug)]
pub struct MiningCoordinator {
    core: Arc<AppCore>,
    assets: MiningAssetService,
    asset_store: Arc<AssetStore>,
    publisher: CardPublisher,
    storage: Arc<Storage>,
    clip_policy: ClipPolicy,
}

impl MiningCoordinator {
    #[must_use]
    pub fn new(
        core: Arc<AppCore>,
        assets: MiningAssetService,
        asset_store: Arc<AssetStore>,
        publisher: CardPublisher,
        storage: Arc<Storage>,
        clip_policy: ClipPolicy,
    ) -> Self {
        Self {
            core,
            assets,
            asset_store,
            publisher,
            storage,
            clip_policy,
        }
    }

    pub fn publish(&self, request: CreateCardRequestV1) -> Result<CreateCardResultV1, AppErrorV1> {
        let draft = self
            .storage
            .get_draft(&request.draft_id)?
            .ok_or_else(|| request_error("The card draft no longer exists."))?;
        if draft.revision != request.expected_draft_revision {
            return Err(AppErrorV1::new(
                error_codes::STALE_REVISION,
                "The card draft changed. Review it again before publishing.",
                true,
            ));
        }
        let stable_mining_id = mining_id(&draft, &request.profile_id);
        let job_id = JobId::new(format!("publish_{}", &stable_mining_id[..24]));
        let profile = match self.publisher.profile_snapshot(&request.profile_id) {
            Some(profile) => profile,
            None => {
                return Err(snapshot_changed_error(
                    "The selected Anki profile does not exist.",
                ));
            }
        };
        let requested_payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: stable_mining_id.clone(),
            session_id: draft.session_id.clone(),
            request,
            profile: ProfileSnapshot::from(&profile),
            clip_policy: ClipPolicySnapshot::from(&self.clip_policy),
        };
        let payload = match ensure_publish_job(self.storage.as_ref(), &job_id, &requested_payload) {
            Ok(payload) => payload,
            Err(error) => {
                if is_permanent_recovery_error(&error)
                    && let Some(job) = self.storage.job(&job_id)?
                {
                    record_permanent_failure(&self.storage, &job, &error);
                }
                return Err(error);
            }
        };
        if self.storage.publish_job_cancelled(&job_id)? {
            if let Some(cleanup) = self.storage.cancelled_publish_cleanup(&job_id)?
                && let Err(error) = cleanup_cancelled_publish_job(
                    self.storage.as_ref(),
                    self.asset_store.as_ref(),
                    &cleanup,
                )
            {
                return Err(retain_cancelled_cleanup_error(
                    self.storage.as_ref(),
                    &job_id,
                    error,
                ));
            }
            if !self.storage.restart_cancelled_publish_job(&job_id)? {
                return Err(media_error(
                    "The cancelled publish could not restart until its retained assets are released.",
                ));
            }
        }
        let result = self.publish_payload(payload, draft, false);
        if let Err(error) = &result
            && is_permanent_recovery_error(error)
            && let Ok(Some(job)) = self.storage.job(&job_id)
        {
            record_permanent_failure(&self.storage, &job, error);
        }
        result
    }

    fn publish_payload(
        &self,
        payload: DurablePublishPayload,
        draft: CardDraftV1,
        recovery: bool,
    ) -> Result<CreateCardResultV1, AppErrorV1> {
        if draft.revision != payload.request.expected_draft_revision
            || draft.session_id != payload.session_id
            || mining_id(&draft, &payload.request.profile_id) != payload.mining_id
        {
            return Err(request_error(
                "The durable publish snapshot no longer matches its immutable draft.",
            ));
        }
        self.require_current_snapshot(&payload)?;
        let stable_mining_id = payload.mining_id.clone();
        let job_id = JobId::new(format!("publish_{}", &stable_mining_id[..24]));
        if self.storage.publish_job_cancelled(&job_id)? {
            return Err(cancelled_error());
        }
        let existing = self.storage.publish_attempt(&stable_mining_id)?;
        if let Some(failed) = existing
            .as_ref()
            .filter(|attempt| attempt.stage == PublishStage::Failed)
        {
            for hash in &failed.asset_hashes {
                self.asset_store.release_durable(&stable_mining_id, hash)?;
            }
            self.storage
                .clear_failed_publish_attempt(&stable_mining_id)?;
        }
        let mut attempt = existing
            .filter(|attempt| attempt.stage != PublishStage::Failed)
            .unwrap_or(PublishAttempt {
                mining_id: stable_mining_id.clone(),
                job_id: job_id.clone(),
                stage: PublishStage::Validated,
                note_id: None,
                asset_hashes: Vec::new(),
                terminal_error_json: None,
            });
        if self.storage.publish_attempt(&stable_mining_id)?.is_none() {
            self.storage.record_publish_stage(&attempt)?;
        }
        advance_job_to(self.storage.as_ref(), &attempt.job_id, attempt.stage)?;

        if attempt.stage == PublishStage::Confirmed {
            return self.publisher.reconcile(payload.request);
        }

        let media = if stage_has_assets(attempt.stage) {
            self.resume_media(&attempt)?
        } else {
            set_raw_job_stage(self.storage.as_ref(), &attempt.job_id, JobStage::Extracting)?;
            let (session, recovered_session) = self.extraction_session(&draft)?;
            let bundle = self.assets.create_for_owner(
                &stable_mining_id,
                session.session_id.clone(),
                &draft.cue,
                draft.observed_source_time_us,
                session.duration_us,
            );
            let close_result = recovered_session
                .then(|| self.core.close_session(&session.session_id))
                .transpose();
            let bundle = bundle?;
            close_result?;
            if self.storage.publish_job_cancelled(&job_id)? {
                self.asset_store
                    .release_durable(&stable_mining_id, &bundle.audio.hash)?;
                self.asset_store
                    .release_durable(&stable_mining_id, &bundle.image.hash)?;
                return Err(cancelled_error());
            }
            attempt.asset_hashes = vec![bundle.audio.hash.clone(), bundle.image.hash.clone()];
            advance_attempt(
                self.storage.as_ref(),
                &mut attempt,
                PublishStage::AssetsReady,
                None,
            )?;
            media_payload(&bundle.audio, &bundle.image)?
        };
        let claim =
            PublishClaimGuard::acquire(self.storage.clone(), &stable_mining_id, &job_id, recovery)?;
        let observer = DurablePublishObserver::new(
            self.storage.clone(),
            attempt,
            claim.owner_token().to_owned(),
        );
        let profile = AnkiProfile::from(&payload.profile);
        self.publisher
            .publish_snapshot_with_media(payload.request, &profile, &media, &observer)
    }

    pub fn validate_profile(&self, profile_id: &str) -> Result<(), AppErrorV1> {
        self.publisher.validate_profile(profile_id)
    }

    pub fn resume_pending(&self) -> RecoveryReport {
        let mut report = RecoveryReport::default();
        replay_cancelled_publish_cleanups(&self.storage, &self.asset_store, &mut report);
        let jobs = match self.storage.recoverable_jobs(1_000) {
            Ok(jobs) => jobs,
            Err(error) => {
                report.errors.push(RecoveryIssue {
                    job_id: None,
                    error,
                });
                return report;
            }
        };
        for job in jobs.into_iter().filter(|job| job.kind == PUBLISH_JOB_KIND) {
            report.attempted += 1;
            let payload = match serde_json::from_str::<DurablePublishPayload>(&job.payload_json) {
                Ok(payload) if payload.version == PUBLISH_PAYLOAD_VERSION => payload,
                Ok(_) => {
                    let error = request_error(
                        "Stored publish request used an unsupported snapshot version.",
                    );
                    record_permanent_failure(&self.storage, &job, &error);
                    report.errors.push(RecoveryIssue {
                        job_id: Some(job.job_id.clone()),
                        error,
                    });
                    continue;
                }
                Err(error) => {
                    let error =
                        request_error(&format!("Stored publish request was invalid: {error}"));
                    record_permanent_failure(&self.storage, &job, &error);
                    report.errors.push(RecoveryIssue {
                        job_id: Some(job.job_id.clone()),
                        error,
                    });
                    continue;
                }
            };
            let draft = match self.storage.get_draft(&payload.request.draft_id) {
                Ok(Some(draft)) => draft,
                Ok(None) => {
                    let error = request_error("The stored card draft no longer exists.");
                    record_permanent_failure(&self.storage, &job, &error);
                    report.errors.push(RecoveryIssue {
                        job_id: Some(job.job_id.clone()),
                        error,
                    });
                    continue;
                }
                Err(error) => {
                    report.errors.push(RecoveryIssue {
                        job_id: Some(job.job_id.clone()),
                        error,
                    });
                    continue;
                }
            };
            match self.publish_payload(payload, draft, true) {
                Ok(_) => report.completed += 1,
                Err(error) => {
                    if is_permanent_recovery_error(&error) {
                        record_permanent_failure(&self.storage, &job, &error);
                    }
                    report.errors.push(RecoveryIssue {
                        job_id: Some(job.job_id),
                        error,
                    });
                }
            }
        }
        report
    }

    pub fn cancel_session(&self, session_id: &contracts::MediaSessionId) -> bool {
        cancel_publish_jobs_for_session(&self.storage, &self.asset_store, session_id)
    }

    fn require_current_snapshot(&self, payload: &DurablePublishPayload) -> Result<(), AppErrorV1> {
        let current_profile = self
            .publisher
            .profile_snapshot(&payload.request.profile_id)
            .ok_or_else(|| {
                snapshot_changed_error("The saved Anki profile is no longer configured.")
            })?;
        if ProfileSnapshot::from(&current_profile) != payload.profile
            || ClipPolicySnapshot::from(&self.clip_policy) != payload.clip_policy
        {
            return Err(snapshot_changed_error(
                "The Anki profile or clip policy changed after this publish began.",
            ));
        }
        Ok(())
    }

    fn extraction_session(
        &self,
        draft: &CardDraftV1,
    ) -> Result<(MediaSessionV1, bool), AppErrorV1> {
        match self.core.session(&draft.session_id) {
            Ok(session) => return Ok((session, false)),
            Err(error) if error.code == error_codes::MEDIA_SCOPE_DENIED => {}
            Err(error) => return Err(error),
        }
        let recent = self
            .storage
            .recent(&draft.source_fingerprint)?
            .ok_or_else(|| {
                AppErrorV1::new(
                    error_codes::MEDIA_SCOPE_DENIED,
                    "This interrupted publish needs its source media. Reopen it, choose Remember, and retry recovery.",
                    true,
                )
            })?;
        let session = self.core.import_media(recent.path)?;
        if session.source_fingerprint != draft.source_fingerprint {
            let _ = self.core.close_session(&session.session_id);
            return Err(AppErrorV1::new(
                error_codes::MEDIA_SCOPE_DENIED,
                "The remembered source changed after the card draft was created. Locate and approve the original file again.",
                false,
            ));
        }
        Ok((session, true))
    }

    fn resume_media(&self, attempt: &PublishAttempt) -> Result<Vec<AnkiMedia>, AppErrorV1> {
        let [audio_hash, image_hash] = attempt.asset_hashes.as_slice() else {
            return Err(media_error(
                "The recoverable publish attempt did not retain exactly two asset hashes.",
            ));
        };
        let audio = self
            .asset_store
            .find(audio_hash, AssetKind::Audio)?
            .ok_or_else(|| media_error("The recoverable audio asset is missing."))?;
        let image = self
            .asset_store
            .find(image_hash, AssetKind::Image)?
            .ok_or_else(|| media_error("The recoverable image asset is missing."))?;
        media_payload(&audio, &image)
    }
}

fn cancel_publish_jobs_for_session(
    storage: &Storage,
    asset_store: &AssetStore,
    session_id: &contracts::MediaSessionId,
) -> bool {
    let Ok(cancelled) = storage.cancel_publish_jobs_for_session(session_id.as_str()) else {
        return false;
    };
    for job in &cancelled {
        if let Err(error) = cleanup_cancelled_publish_job(storage, asset_store, job) {
            let _ = retain_cancelled_cleanup_error(storage, &job.job_id, error);
        }
    }
    !cancelled.is_empty()
}

fn cleanup_cancelled_publish_job(
    storage: &Storage,
    asset_store: &AssetStore,
    job: &CancelledPublishJob,
) -> Result<(), AppErrorV1> {
    if job.asset_hashes.is_empty() {
        return Ok(());
    }
    for hash in &job.asset_hashes {
        asset_store.release_durable(&job.mining_id, hash)?;
    }
    if !storage.acknowledge_cancelled_publish_cleanup(&job.job_id)? {
        return Err(media_error(
            "The cancelled publish cleanup marker disappeared before acknowledgement.",
        ));
    }
    Ok(())
}

fn retain_cancelled_cleanup_error(
    storage: &Storage,
    job_id: &JobId,
    error: AppErrorV1,
) -> AppErrorV1 {
    let Ok(error_json) = serde_json::to_string(&error) else {
        return error;
    };
    match storage.record_cancelled_publish_cleanup_error(job_id, &error_json) {
        Ok(true) => error,
        Ok(false) => media_error("The cancelled publish cleanup marker was not retained."),
        Err(storage_error) => storage_error,
    }
}

fn replay_cancelled_publish_cleanups(
    storage: &Storage,
    asset_store: &AssetStore,
    report: &mut RecoveryReport,
) {
    let mut attempted = BTreeSet::new();
    while attempted.len() < MAX_CANCELLED_CLEANUPS_PER_RECOVERY {
        let remaining = MAX_CANCELLED_CLEANUPS_PER_RECOVERY - attempted.len();
        let batch = match storage
            .pending_cancelled_publish_cleanups(remaining.min(CANCELLED_CLEANUP_BATCH_SIZE))
        {
            Ok(batch) => batch,
            Err(error) => {
                report.errors.push(RecoveryIssue {
                    job_id: None,
                    error,
                });
                return;
            }
        };
        if batch.is_empty() {
            return;
        }
        let mut made_progress = false;
        for job in batch {
            if !attempted.insert(job.job_id.clone()) {
                continue;
            }
            made_progress = true;
            if let Err(error) = cleanup_cancelled_publish_job(storage, asset_store, &job) {
                let error = retain_cancelled_cleanup_error(storage, &job.job_id, error);
                report.errors.push(RecoveryIssue {
                    job_id: Some(job.job_id),
                    error,
                });
            }
        }
        if !made_progress {
            return;
        }
    }
}

struct PublishClaimGuard {
    storage: Arc<Storage>,
    mining_id: String,
    owner_token: String,
}

impl PublishClaimGuard {
    fn acquire(
        storage: Arc<Storage>,
        mining_id: &str,
        job_id: &JobId,
        recovery: bool,
    ) -> Result<Self, AppErrorV1> {
        let owner_token = new_owner_token();
        let (attempts, interval) = if recovery {
            (RECOVERY_CLAIM_WAIT_ATTEMPTS, RECOVERY_CLAIM_WAIT_INTERVAL)
        } else {
            (CLAIM_WAIT_ATTEMPTS, CLAIM_WAIT_INTERVAL)
        };
        for _ in 0..attempts {
            if storage.publish_job_cancelled(job_id)? {
                return Err(cancelled_error());
            }
            if storage.try_acquire_publish_claim(mining_id, &owner_token, CLAIM_LEASE_SECONDS)? {
                return Ok(Self {
                    storage,
                    mining_id: mining_id.to_owned(),
                    owner_token,
                });
            }
            thread::sleep(interval);
        }
        Err(AppErrorV1::new(
            error_codes::PUBLISH_OUTCOME_UNCERTAIN,
            "Another application instance is still publishing this card. Wait briefly, then retry.",
            true,
        ))
    }

    fn owner_token(&self) -> &str {
        &self.owner_token
    }
}

impl Drop for PublishClaimGuard {
    fn drop(&mut self) {
        let _ = self
            .storage
            .release_publish_claim(&self.mining_id, &self.owner_token);
    }
}

struct DurablePublishObserver {
    storage: Arc<Storage>,
    attempt: parking_lot::Mutex<PublishAttempt>,
    owner_token: String,
}

impl DurablePublishObserver {
    fn new(storage: Arc<Storage>, attempt: PublishAttempt, owner_token: String) -> Self {
        Self {
            storage,
            attempt: parking_lot::Mutex::new(attempt),
            owner_token,
        }
    }

    fn renew_or_stop(&self) -> Result<(), AppErrorV1> {
        let attempt = self.attempt.lock();
        if self.storage.publish_job_cancelled(&attempt.job_id)? {
            return Err(cancelled_error());
        }
        if !self.storage.renew_publish_claim(
            &attempt.mining_id,
            &self.owner_token,
            CLAIM_LEASE_SECONDS,
        )? {
            return Err(claim_lost_error());
        }
        Ok(())
    }

    fn record_stage(&self, stage: PublishStage, note_id: Option<i64>) -> Result<(), AppErrorV1> {
        let mut attempt = self.attempt.lock();
        if let Some(current) = self.storage.publish_attempt(&attempt.mining_id)?
            && publish_stage_rank(current.stage) > publish_stage_rank(stage)
        {
            *attempt = current;
            return Ok(());
        }
        attempt.stage = stage;
        attempt.note_id = note_id;
        attempt.terminal_error_json = None;
        self.storage.record_publish_stage(&attempt)?;
        if !self.storage.publish_job_cancelled(&attempt.job_id)? {
            set_job_stage(&self.storage, &attempt.job_id, stage)?;
        }
        Ok(())
    }
}

const fn publish_stage_rank(stage: PublishStage) -> u8 {
    match stage {
        PublishStage::Validated => 0,
        PublishStage::AssetsReady => 1,
        PublishStage::MediaUploaded => 2,
        PublishStage::NoteCreated => 3,
        PublishStage::Confirmed => 4,
        PublishStage::Failed => 5,
    }
}

impl PublishObserver for DurablePublishObserver {
    fn observe(&self, boundary: PublishBoundary) -> Result<(), AppErrorV1> {
        match boundary {
            PublishBoundary::BeforeLookup | PublishBoundary::BeforeMediaUpload => {
                self.renew_or_stop()
            }
            PublishBoundary::MediaUploaded => self.record_stage(PublishStage::MediaUploaded, None),
            PublishBoundary::BeforeNoteCreation => {
                let attempt = self.attempt.lock();
                if self.storage.begin_note_publish(
                    &attempt.mining_id,
                    &attempt.job_id,
                    &self.owner_token,
                    CLAIM_LEASE_SECONDS,
                )? {
                    Ok(())
                } else if self.storage.publish_job_cancelled(&attempt.job_id)? {
                    Err(cancelled_error())
                } else {
                    Err(claim_lost_error())
                }
            }
            PublishBoundary::NoteCreated(note_id) => {
                self.record_stage(PublishStage::NoteCreated, Some(note_id))
            }
            PublishBoundary::Confirmed(note_id) => {
                self.record_stage(PublishStage::Confirmed, Some(note_id))
            }
        }
    }
}

fn media_payload(audio: &StoredAsset, image: &StoredAsset) -> Result<Vec<AnkiMedia>, AppErrorV1> {
    let total = audio
        .size_bytes
        .checked_add(image.size_bytes)
        .ok_or_else(|| media_error("Mining media size overflowed."))?;
    if total > MAX_PUBLISH_MEDIA_BYTES {
        return Err(media_error(
            "Mining media exceeded the thirty-two MiB publish limit.",
        ));
    }
    Ok(vec![read_asset(audio)?, read_asset(image)?])
}

fn stage_has_assets(stage: PublishStage) -> bool {
    matches!(
        stage,
        PublishStage::AssetsReady
            | PublishStage::MediaUploaded
            | PublishStage::NoteCreated
            | PublishStage::Confirmed
    )
}

fn read_asset(asset: &StoredAsset) -> Result<AnkiMedia, AppErrorV1> {
    let data = fs::read(&asset.path).map_err(media_error)?;
    if data.is_empty() || data.len() as u64 != asset.size_bytes {
        return Err(media_error(
            "A mining asset changed before it could be published.",
        ));
    }
    Ok(AnkiMedia {
        media_name: asset.media_name.clone(),
        data,
    })
}

fn advance_attempt(
    storage: &Storage,
    attempt: &mut PublishAttempt,
    target: PublishStage,
    note_id: Option<i64>,
) -> Result<(), AppErrorV1> {
    let stages = [
        PublishStage::Validated,
        PublishStage::AssetsReady,
        PublishStage::MediaUploaded,
        PublishStage::NoteCreated,
        PublishStage::Confirmed,
    ];
    let current = stages
        .iter()
        .position(|stage| *stage == attempt.stage)
        .ok_or_else(|| request_error("Publish attempt has no recoverable stage."))?;
    let destination = stages
        .iter()
        .position(|stage| *stage == target)
        .ok_or_else(|| request_error("Publish stage was invalid."))?;
    if destination < current {
        return Ok(());
    }
    for next in stages.iter().take(destination + 1).skip(current + 1) {
        attempt.stage = *next;
        attempt.note_id = matches!(next, PublishStage::NoteCreated | PublishStage::Confirmed)
            .then_some(note_id)
            .flatten();
        attempt.terminal_error_json = None;
        storage.record_publish_stage(attempt)?;
        advance_job_to(storage, &attempt.job_id, *next)?;
    }
    Ok(())
}

fn ensure_publish_job(
    storage: &Storage,
    job_id: &JobId,
    payload: &DurablePublishPayload,
) -> Result<DurablePublishPayload, AppErrorV1> {
    let payload_json = serde_json::to_string(payload).map_err(media_error)?;
    if let Some(existing) = storage.job(job_id)? {
        let original = serde_json::from_str::<DurablePublishPayload>(&existing.payload_json)
            .map_err(|error| {
                request_error(&format!("Stored publish request was invalid: {error}"))
            })?;
        if existing.kind != PUBLISH_JOB_KIND || original != *payload {
            return Err(AppErrorV1::new(
                error_codes::STALE_REVISION,
                "This publish already started with different card, profile, or clip settings. Resume the immutable original request.",
                true,
            ));
        }
        return Ok(original);
    }
    storage.upsert_job(&DurableJob {
        job_id: job_id.clone(),
        kind: PUBLISH_JOB_KIND.into(),
        stage: JobStage::Validated.as_str().into(),
        payload_json,
        terminal_error_json: None,
    })?;
    Ok(payload.clone())
}

fn set_job_stage(
    storage: &Storage,
    job_id: &JobId,
    target: PublishStage,
) -> Result<(), AppErrorV1> {
    let Some(mut job) = storage.job(job_id)? else {
        return Err(request_error("Durable publish request was missing."));
    };
    job.stage = match target {
        PublishStage::Validated => JobStage::Validated,
        PublishStage::AssetsReady => JobStage::AssetsReady,
        PublishStage::MediaUploaded => JobStage::MediaUploaded,
        PublishStage::NoteCreated => JobStage::NoteCreated,
        PublishStage::Confirmed => JobStage::Confirmed,
        PublishStage::Failed => return Ok(()),
    }
    .as_str()
    .into();
    storage.upsert_job(&job)
}

fn set_raw_job_stage(
    storage: &Storage,
    job_id: &JobId,
    target: JobStage,
) -> Result<(), AppErrorV1> {
    let Some(mut job) = storage.job(job_id)? else {
        return Err(request_error("Durable publish request was missing."));
    };
    job.stage = target.as_str().into();
    storage.upsert_job(&job)
}

fn advance_job_to(
    storage: &Storage,
    job_id: &JobId,
    target: PublishStage,
) -> Result<(), AppErrorV1> {
    let Some(mut job) = storage.job(job_id)? else {
        return Err(request_error("Durable publish request was missing."));
    };
    let target = match target {
        PublishStage::Validated => JobStage::Validated,
        PublishStage::AssetsReady => JobStage::AssetsReady,
        PublishStage::MediaUploaded => JobStage::MediaUploaded,
        PublishStage::NoteCreated => JobStage::NoteCreated,
        PublishStage::Confirmed => JobStage::Confirmed,
        PublishStage::Failed => return Ok(()),
    };
    let stages = [
        JobStage::Validated,
        JobStage::AssetsReady,
        JobStage::MediaUploaded,
        JobStage::NoteCreated,
        JobStage::Confirmed,
    ];
    let current = job.stage()?;
    if current == JobStage::Cancelled {
        return Err(cancelled_error());
    }
    if current == JobStage::Extracting && target == JobStage::Validated {
        return Ok(());
    }
    if current == JobStage::Extracting && target == JobStage::AssetsReady {
        job.stage = JobStage::AssetsReady.as_str().into();
        return storage.upsert_job(&job);
    }
    if current == JobStage::PublishingNote && target == JobStage::MediaUploaded {
        return Ok(());
    }
    let current = stages
        .iter()
        .position(|stage| *stage == current)
        .ok_or_else(|| request_error("Durable publish job had an incompatible stage."))?;
    let destination = stages
        .iter()
        .position(|stage| *stage == target)
        .ok_or_else(|| request_error("Durable publish target stage was invalid."))?;
    for next in stages.iter().take(destination + 1).skip(current + 1) {
        job.stage = next.as_str().into();
        storage.upsert_job(&job)?;
    }
    Ok(())
}

fn request_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::INVALID_REQUEST,
        "The card publish request could not be resumed safely.",
        false,
    )
    .with_diagnostics(detail)
}

fn snapshot_changed_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::STALE_REVISION,
        "This interrupted publish is locked to its original Anki profile and clip settings. Restore those settings or cancel the pending job.",
        true,
    )
    .with_diagnostics(detail)
}

fn is_permanent_recovery_error(error: &AppErrorV1) -> bool {
    !error.retryable && error.code == error_codes::INVALID_REQUEST
}

fn record_permanent_failure(storage: &Storage, job: &DurableJob, error: &AppErrorV1) {
    let Ok(terminal_error_json) = serde_json::to_string(error) else {
        return;
    };
    if let Ok(payload) = serde_json::from_str::<DurablePublishPayload>(&job.payload_json)
        && let Ok(Some(mut attempt)) = storage.publish_attempt(&payload.mining_id)
        && matches!(
            attempt.stage,
            PublishStage::Validated
                | PublishStage::AssetsReady
                | PublishStage::MediaUploaded
                | PublishStage::Failed
        )
    {
        attempt.stage = PublishStage::Failed;
        attempt.note_id = None;
        attempt.terminal_error_json = Some(terminal_error_json.clone());
        let _ = storage.record_publish_stage(&attempt);
    }
    if let Ok(Some(mut current)) = storage.job(&job.job_id)
        && current.stage().is_ok_and(|stage| !stage.is_terminal())
    {
        current.stage = JobStage::Failed.as_str().into();
        current.terminal_error_json = Some(terminal_error_json);
        let _ = storage.upsert_job(&current);
    }
}

fn cancelled_error() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_CANCELLED,
        "Card publishing was cancelled before the note was created.",
        false,
    )
}

fn claim_lost_error() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::PUBLISH_OUTCOME_UNCERTAIN,
        "Another application instance took over this interrupted publish. Reconcile before retrying.",
        true,
    )
}

fn new_owner_token() -> String {
    let sequence = CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("p{}-t{sequence}-n{nanos}", std::process::id())
}

fn media_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "The card audio or image could not be read safely.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use anki_connect::transport::AnkiApi;
    use contracts::{
        CardDraftV1, CreateCardOutcomeV1, CueId, DraftId, MediaSessionId, SubtitleCueV1,
        SubtitleSourceId, SubtitleStyleHintV1, TokenId, TokenV1,
    };
    use ports::{DraftRepositoryPort, PublishHistoryPort};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Default)]
    struct SharedApi {
        state: Mutex<(Option<i64>, usize)>,
    }

    impl SharedApi {
        fn lock(&self) -> MutexGuard<'_, (Option<i64>, usize)> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl AnkiApi for SharedApi {
        fn invoke(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
            match action {
                "version" => Ok(json!(6)),
                "deckNames" => Ok(json!(["Mining"])),
                "modelNames" => Ok(json!(["Basic"])),
                "modelFieldNames" => Ok(json!(["Front"])),
                "findNotes" => Ok(json!(self.lock().0.into_iter().collect::<Vec<_>>())),
                "addNote" => {
                    let _ = params;
                    let mut state = self.lock();
                    state.1 += 1;
                    state.0 = Some(42);
                    Ok(json!(42))
                }
                _ => Err(request_error("Unexpected mock Anki action.")),
            }
        }
    }

    fn test_draft() -> CardDraftV1 {
        CardDraftV1 {
            draft_id: DraftId::new("concurrent-draft"),
            revision: 1,
            session_id: MediaSessionId::new("concurrent-session"),
            subtitle_source_id: SubtitleSourceId::new("subtitles"),
            subtitle_source_version: "v1".into(),
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
            source_fingerprint: "fingerprint".into(),
        }
    }

    fn test_profile() -> AnkiProfile {
        AnkiProfile {
            profile_id: "default".into(),
            deck_name: "Mining".into(),
            model_name: "Basic".into(),
            field_mapping: BTreeMap::from([("expression".into(), "Front".into())]),
            tags: vec!["migaku".into()],
        }
    }

    #[test]
    fn advancing_to_confirmation_persists_every_recovery_stage() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let mining_id = "a".repeat(64);
        let job_id = JobId::new("publish-test");
        let request = CreateCardRequestV1 {
            draft_id: contracts::DraftId::new("draft-test"),
            expected_draft_revision: 1,
            profile_id: "default".into(),
            editable_fields: std::collections::BTreeMap::from([(
                "expression".into(),
                "映画".into(),
            )]),
            confirmed: true,
        };
        let payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: mining_id.clone(),
            session_id: contracts::MediaSessionId::new("session"),
            request,
            profile: ProfileSnapshot {
                profile_id: "default".into(),
                deck_name: "Mining".into(),
                model_name: "Basic".into(),
                field_mapping: BTreeMap::new(),
                tags: vec!["migaku".into()],
            },
            clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
        };
        ensure_publish_job(&storage, &job_id, &payload)?;
        let mut changed_payload = payload.clone();
        changed_payload.request.confirmed = false;
        let Err(error) = ensure_publish_job(&storage, &job_id, &changed_payload) else {
            return Err(request_error(
                "A changed durable publish request was unexpectedly accepted.",
            ));
        };
        assert_eq!(error.code, error_codes::STALE_REVISION);
        let mut changed_profile = payload.clone();
        changed_profile.profile.deck_name = "Different deck".into();
        let Err(drift) = ensure_publish_job(&storage, &job_id, &changed_profile) else {
            return Err(request_error(
                "A changed profile unexpectedly replaced the immutable snapshot.",
            ));
        };
        assert_eq!(drift.code, error_codes::STALE_REVISION);
        assert!(drift.retryable);
        assert!(!is_permanent_recovery_error(&drift));
        let mut changed_clip = payload.clone();
        changed_clip.clip_policy.leading_padding_us += 1;
        assert!(ensure_publish_job(&storage, &job_id, &changed_clip).is_err());
        assert_eq!(ensure_publish_job(&storage, &job_id, &payload)?, payload);
        assert_eq!(
            storage.job(&job_id)?.and_then(|job| job.stage().ok()),
            Some(JobStage::Validated)
        );
        let mut attempt = PublishAttempt {
            mining_id: mining_id.clone(),
            job_id,
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: vec!["b".repeat(64), "c".repeat(64)],
            terminal_error_json: None,
        };
        storage.record_publish_stage(&attempt)?;

        advance_attempt(&storage, &mut attempt, PublishStage::Confirmed, Some(42))?;

        assert_eq!(attempt.stage, PublishStage::Confirmed);
        assert_eq!(attempt.note_id, Some(42));
        assert_eq!(storage.publish_attempt(&mining_id)?, Some(attempt));
        assert_eq!(storage.note_for_mining_id(&mining_id)?, Some(42));
        assert!(storage.pending_publish_attempts(10)?.is_empty());
        Ok(())
    }

    #[test]
    fn cancelling_session_cancels_every_matching_job() -> Result<(), AppErrorV1> {
        let session_id = contracts::MediaSessionId::new("shared-session");
        let storage = Storage::in_memory()?;
        for index in 0..2 {
            let mining_id = if index == 0 {
                "1".repeat(64)
            } else {
                "2".repeat(64)
            };
            let request = CreateCardRequestV1 {
                draft_id: contracts::DraftId::new(format!("draft-{index}")),
                expected_draft_revision: 1,
                profile_id: "default".into(),
                editable_fields: BTreeMap::new(),
                confirmed: true,
            };
            let payload = DurablePublishPayload {
                version: PUBLISH_PAYLOAD_VERSION,
                mining_id,
                session_id: session_id.clone(),
                request,
                profile: ProfileSnapshot {
                    profile_id: "default".into(),
                    deck_name: "Mining".into(),
                    model_name: "Basic".into(),
                    field_mapping: BTreeMap::new(),
                    tags: Vec::new(),
                },
                clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
            };
            ensure_publish_job(
                &storage,
                &JobId::new(format!("publish-cancel-{index}")),
                &payload,
            )?;
        }
        assert_eq!(
            storage
                .cancel_publish_jobs_for_session(session_id.as_str())?
                .len(),
            2
        );
        assert!(storage.recoverable_jobs(10)?.is_empty());
        Ok(())
    }

    #[test]
    fn two_publishers_with_separate_connections_add_exactly_one_note() -> Result<(), AppErrorV1> {
        let database = std::env::temp_dir().join(format!(
            "migaku-publish-claim-{}.sqlite3",
            new_owner_token()
        ));
        let first_storage = Arc::new(Storage::open(&database)?);
        let second_storage = Arc::new(Storage::open(&database)?);
        let draft = test_draft();
        first_storage.insert_draft(&draft)?;
        let request = CreateCardRequestV1 {
            draft_id: draft.draft_id.clone(),
            expected_draft_revision: draft.revision,
            profile_id: "default".into(),
            editable_fields: BTreeMap::new(),
            confirmed: true,
        };
        let stable_mining_id = mining_id(&draft, &request.profile_id);
        let job_id = JobId::new(format!("publish_{}", &stable_mining_id[..24]));
        let payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: stable_mining_id.clone(),
            session_id: draft.session_id,
            request: request.clone(),
            profile: ProfileSnapshot::from(&test_profile()),
            clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
        };
        ensure_publish_job(&first_storage, &job_id, &payload)?;
        let mut attempt = PublishAttempt {
            mining_id: stable_mining_id.clone(),
            job_id: job_id.clone(),
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: Vec::new(),
            terminal_error_json: None,
        };
        first_storage.record_publish_stage(&attempt)?;
        advance_attempt(
            &first_storage,
            &mut attempt,
            PublishStage::AssetsReady,
            None,
        )?;

        let api = Arc::new(SharedApi::default());
        let first_publisher = CardPublisher::new(
            api.clone(),
            first_storage.clone(),
            first_storage.clone(),
            [test_profile()],
        );
        let second_publisher = CardPublisher::new(
            api.clone(),
            second_storage.clone(),
            second_storage.clone(),
            [test_profile()],
        );
        let workers = [
            (first_storage.clone(), first_publisher),
            (second_storage.clone(), second_publisher),
        ]
        .into_iter()
        .map(|(storage, publisher)| {
            let request = request.clone();
            let stable_mining_id = stable_mining_id.clone();
            let job_id = job_id.clone();
            std::thread::spawn(move || {
                let claim =
                    PublishClaimGuard::acquire(storage.clone(), &stable_mining_id, &job_id, false)?;
                let current = storage
                    .publish_attempt(&stable_mining_id)?
                    .ok_or_else(|| request_error("Publish attempt disappeared."))?;
                if current.stage == PublishStage::Confirmed {
                    return publisher.reconcile(request);
                }
                let observer =
                    DurablePublishObserver::new(storage, current, claim.owner_token().to_owned());
                publisher.publish_snapshot_with_media(request, &test_profile(), &[], &observer)
            })
        })
        .collect::<Vec<_>>();
        let mut created = 0;
        for worker in workers {
            let result = worker
                .join()
                .map_err(|_| request_error("Concurrent publisher panicked."))??;
            created += usize::from(result.outcome == CreateCardOutcomeV1::Created);
        }
        assert_eq!(created, 1);
        assert_eq!(api.lock().1, 1);
        assert_eq!(
            first_storage.note_for_mining_id(&stable_mining_id)?,
            Some(42)
        );
        let times = first_storage
            .publish_stage_times(&stable_mining_id)?
            .ok_or_else(|| request_error("Publish boundary timestamps were missing."))?;
        assert!(times.media_uploaded_at.is_some());
        assert!(times.note_created_at.is_some());
        assert!(times.confirmed_at.is_some());
        drop(first_storage);
        drop(second_storage);
        drop(api);
        let _ = std::fs::remove_file(database);
        Ok(())
    }

    #[test]
    fn recovery_wait_covers_lease_and_health_errors_remain_job_scoped() {
        let recovery_wait = RECOVERY_CLAIM_WAIT_INTERVAL
            .checked_mul(RECOVERY_CLAIM_WAIT_ATTEMPTS as u32)
            .unwrap_or(Duration::MAX);
        assert!(recovery_wait > Duration::from_secs(CLAIM_LEASE_SECONDS as u64));

        let first = JobId::new("first-job");
        let second = JobId::new("second-job");
        let error = request_error("injected");
        let mut report = RecoveryReport {
            attempted: 2,
            completed: 0,
            errors: vec![
                RecoveryIssue {
                    job_id: Some(first.clone()),
                    error: error.clone(),
                },
                RecoveryIssue {
                    job_id: Some(second.clone()),
                    error: error.clone(),
                },
            ],
        };
        let pending = DurableJob {
            job_id: second.clone(),
            kind: PUBLISH_JOB_KIND.into(),
            stage: JobStage::Validated.as_str().into(),
            payload_json: "{}".into(),
            terminal_error_json: None,
        };
        assert_eq!(report.error_for_jobs(&[pending]), Some(error));
        report.clear_job(&first);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].job_id, Some(second));
        assert!(report.error_for_jobs(&[]).is_none());
    }

    #[test]
    fn extracting_and_permanent_failure_are_durable_but_transient_errors_are_not()
    -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let mining_id = "7".repeat(64);
        let job_id = JobId::new("permanent-recovery");
        let payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: mining_id.clone(),
            session_id: MediaSessionId::new("session"),
            request: CreateCardRequestV1 {
                draft_id: DraftId::new("missing-draft"),
                expected_draft_revision: 1,
                profile_id: "default".into(),
                editable_fields: BTreeMap::new(),
                confirmed: true,
            },
            profile: ProfileSnapshot::from(&test_profile()),
            clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
        };
        ensure_publish_job(&storage, &job_id, &payload)?;
        storage.record_publish_stage(&PublishAttempt {
            mining_id: mining_id.clone(),
            job_id: job_id.clone(),
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: Vec::new(),
            terminal_error_json: None,
        })?;
        set_raw_job_stage(&storage, &job_id, JobStage::Extracting)?;
        assert_eq!(
            storage.job(&job_id)?.and_then(|job| job.stage().ok()),
            Some(JobStage::Extracting)
        );
        let permanent = request_error("injected corrupt payload");
        let job = storage
            .job(&job_id)?
            .ok_or_else(|| request_error("Durable job disappeared."))?;
        record_permanent_failure(&storage, &job, &permanent);
        assert_eq!(
            storage.job(&job_id)?.and_then(|job| job.stage().ok()),
            Some(JobStage::Failed)
        );
        assert_eq!(
            storage.publish_attempt(&mining_id)?.map(|item| item.stage),
            Some(PublishStage::Failed)
        );
        assert!(storage.recoverable_jobs(10)?.is_empty());
        let failed = storage.failed_jobs(10)?;
        assert_eq!(failed.len(), 1);
        assert_eq!(
            failed[0]
                .terminal_error_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<AppErrorV1>(json).ok()),
            Some(permanent)
        );
        assert!(!is_permanent_recovery_error(&snapshot_changed_error(
            "injected settings drift"
        )));
        assert!(!is_permanent_recovery_error(&AppErrorV1::new(
            error_codes::ANKI_OFFLINE,
            "offline",
            true
        )));
        Ok(())
    }

    #[test]
    fn cancelled_asset_cleanup_replays_after_reopen_then_allows_restart() -> Result<(), AppErrorV1>
    {
        let test_root = std::env::temp_dir().join(format!("migaku-cancel-{}", new_owner_token()));
        std::fs::create_dir_all(&test_root).map_err(media_error)?;
        let audio_source = test_root.join("source.mp3");
        let image_source = test_root.join("source.png");
        std::fs::write(&audio_source, b"ID3 audio").map_err(media_error)?;
        std::fs::write(&image_source, [137, 80, 78, 71, 13, 10, 26, 10, 1]).map_err(media_error)?;
        let asset_root = test_root.join("assets");
        let asset_store = AssetStore::open(asset_root.clone())?;
        let audio = asset_store.ingest(&audio_source, AssetKind::Audio)?;
        let image = asset_store.ingest(&image_source, AssetKind::Image)?;
        let mining_id = "8".repeat(64);
        asset_store.protect_durable(&mining_id, &audio.hash)?;
        asset_store.protect_durable(&mining_id, &image.hash)?;
        asset_store.release(&audio.hash);
        asset_store.release(&image.hash);

        let database = test_root.join("storage.sqlite3");
        let storage = Storage::open(&database)?;
        let job_id = JobId::new("cancel-assets");
        let session_id = MediaSessionId::new("asset-session");
        let payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: mining_id.clone(),
            session_id: session_id.clone(),
            request: CreateCardRequestV1 {
                draft_id: DraftId::new("asset-draft"),
                expected_draft_revision: 1,
                profile_id: "default".into(),
                editable_fields: BTreeMap::new(),
                confirmed: true,
            },
            profile: ProfileSnapshot::from(&test_profile()),
            clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
        };
        ensure_publish_job(&storage, &job_id, &payload)?;
        let mut attempt = PublishAttempt {
            mining_id: mining_id.clone(),
            job_id: job_id.clone(),
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: vec![audio.hash.clone(), image.hash.clone()],
            terminal_error_json: None,
        };
        storage.record_publish_stage(&attempt)?;
        advance_attempt(&storage, &mut attempt, PublishStage::AssetsReady, None)?;
        assert_eq!(
            storage
                .cancel_publish_jobs_for_session(session_id.as_str())?
                .len(),
            1
        );
        assert_eq!(storage.cancelled_publish_cleanup_count()?, 1);
        drop(storage);

        let storage = Storage::open(&database)?;
        let mut recovery = RecoveryReport::default();
        replay_cancelled_publish_cleanups(&storage, &asset_store, &mut recovery);
        assert!(recovery.errors.is_empty());
        assert_eq!(storage.cancelled_publish_cleanup_count()?, 0);
        assert!(
            !asset_root
                .join(".refs")
                .join(&mining_id)
                .join(&audio.hash)
                .exists()
        );
        assert!(
            !asset_root
                .join(".refs")
                .join(&mining_id)
                .join(&image.hash)
                .exists()
        );
        assert!(storage.restart_cancelled_publish_job(&job_id)?);
        assert!(storage.publish_attempt(&mining_id)?.is_none());
        drop(asset_store);
        let _ = std::fs::remove_dir_all(test_root);
        Ok(())
    }

    #[test]
    fn cancelled_asset_release_failure_stays_visible_and_retries() -> Result<(), AppErrorV1> {
        let test_root =
            std::env::temp_dir().join(format!("migaku-cancel-failure-{}", new_owner_token()));
        std::fs::create_dir_all(&test_root).map_err(media_error)?;
        let source = test_root.join("source.mp3");
        std::fs::write(&source, b"ID3 audio").map_err(media_error)?;
        let asset_root = test_root.join("assets");
        let asset_store = AssetStore::open(asset_root.clone())?;
        let asset = asset_store.ingest(&source, AssetKind::Audio)?;
        let mining_id = "9".repeat(64);
        asset_store.protect_durable(&mining_id, &asset.hash)?;
        asset_store.release(&asset.hash);

        let storage = Storage::in_memory()?;
        let job_id = JobId::new("cancel-release-failure");
        let session_id = MediaSessionId::new("failure-session");
        let payload = DurablePublishPayload {
            version: PUBLISH_PAYLOAD_VERSION,
            mining_id: mining_id.clone(),
            session_id: session_id.clone(),
            request: CreateCardRequestV1 {
                draft_id: DraftId::new("failure-draft"),
                expected_draft_revision: 1,
                profile_id: "default".into(),
                editable_fields: BTreeMap::new(),
                confirmed: true,
            },
            profile: ProfileSnapshot::from(&test_profile()),
            clip_policy: ClipPolicySnapshot::from(&ClipPolicy::default()),
        };
        ensure_publish_job(&storage, &job_id, &payload)?;
        let mut attempt = PublishAttempt {
            mining_id: mining_id.clone(),
            job_id: job_id.clone(),
            stage: PublishStage::Validated,
            note_id: None,
            asset_hashes: vec![asset.hash.clone()],
            terminal_error_json: None,
        };
        storage.record_publish_stage(&attempt)?;
        advance_attempt(&storage, &mut attempt, PublishStage::AssetsReady, None)?;

        let reference = asset_root.join(".refs").join(&mining_id).join(&asset.hash);
        std::fs::remove_file(&reference).map_err(media_error)?;
        std::fs::create_dir(&reference).map_err(media_error)?;
        assert!(cancel_publish_jobs_for_session(
            &storage,
            &asset_store,
            &session_id
        ));
        let cleanup = storage
            .cancelled_publish_cleanup(&job_id)?
            .ok_or_else(|| request_error("Cleanup marker disappeared after release failure."))?;
        assert!(cleanup.cleanup_error_json.is_some());
        assert!(!storage.restart_cancelled_publish_job(&job_id)?);

        std::fs::remove_dir(&reference).map_err(media_error)?;
        let mut recovery = RecoveryReport::default();
        replay_cancelled_publish_cleanups(&storage, &asset_store, &mut recovery);
        assert!(recovery.errors.is_empty());
        assert!(storage.cancelled_publish_cleanup(&job_id)?.is_none());
        assert!(storage.restart_cancelled_publish_job(&job_id)?);
        drop(asset_store);
        let _ = std::fs::remove_dir_all(test_root);
        Ok(())
    }
}
