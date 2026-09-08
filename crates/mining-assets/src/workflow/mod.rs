use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_COMPLETED_REQUESTS: usize = 1_024;

use contracts::{AppErrorV1, MediaSessionId, SubtitleCueV1, TimestampUs};
use parking_lot::{Condvar, Mutex};
use ports::{AssetExtractionPort, ExtractionSpec};
use sha2::{Digest, Sha256};

use crate::{
    spec::ClipPolicy,
    store::{AssetKind, AssetStore, StoredAsset},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetBundle {
    pub audio: StoredAsset,
    pub image: StoredAsset,
    pub audio_start_us: TimestampUs,
    pub audio_end_us: TimestampUs,
    pub frame_us: TimestampUs,
}

#[derive(Debug, Default)]
struct CompletedRequestCache {
    entries: HashMap<String, AssetBundle>,
    insertion_order: VecDeque<String>,
}

impl CompletedRequestCache {
    fn get(&self, key: &str) -> Option<AssetBundle> {
        self.entries.get(key).cloned()
    }

    fn remove_if_matches(&mut self, key: &str, bundle: &AssetBundle) {
        if self.entries.get(key) == Some(bundle) {
            self.entries.remove(key);
            self.insertion_order.retain(|candidate| candidate != key);
        }
    }

    fn insert(&mut self, key: String, bundle: AssetBundle) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) =
            self.entries.entry(key.clone())
        {
            entry.insert(bundle);
            return;
        }
        while self.entries.len() >= MAX_COMPLETED_REQUESTS {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(key.clone());
        self.entries.insert(key, bundle);
    }
}

/// Bounds automatic cleanup so foreground mining work does not trigger an
/// unbounded filesystem sweep. Assets younger than `minimum_orphan_age`, or
/// protected by either a live or durable reference, are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetRetentionPolicy {
    pub minimum_orphan_age: Duration,
    pub maintenance_interval: Duration,
    pub max_removals_per_run: usize,
}

impl Default for AssetRetentionPolicy {
    fn default() -> Self {
        Self {
            minimum_orphan_age: Duration::from_secs(7 * 24 * 60 * 60),
            maintenance_interval: Duration::from_secs(24 * 60 * 60),
            max_removals_per_run: 64,
        }
    }
}

#[derive(Debug, Default)]
struct InFlightRequest {
    result: Mutex<Option<Result<AssetBundle, AppErrorV1>>>,
    ready: Condvar,
}

impl InFlightRequest {
    fn wait(&self) -> Result<AssetBundle, AppErrorV1> {
        let mut result = self.result.lock();
        while result.is_none() {
            self.ready.wait(&mut result);
        }
        result.as_ref().cloned().ok_or_else(|| {
            metadata_error("An in-flight mining request completed without a result.")
        })?
    }

    fn complete(&self, result: &Result<AssetBundle, AppErrorV1>) {
        *self.result.lock() = Some(result.clone());
        self.ready.notify_all();
    }
}

pub struct MiningAssetService {
    extractor: Arc<dyn AssetExtractionPort>,
    store: Arc<AssetStore>,
    policy: ClipPolicy,
    completed: Mutex<CompletedRequestCache>,
    in_flight: Mutex<HashMap<String, Arc<InFlightRequest>>>,
    retention: AssetRetentionPolicy,
    next_maintenance_epoch_s: AtomicU64,
}

impl std::fmt::Debug for MiningAssetService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MiningAssetService")
            .field("store", &self.store)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl MiningAssetService {
    #[must_use]
    pub fn new(
        extractor: Arc<dyn AssetExtractionPort>,
        store: Arc<AssetStore>,
        policy: ClipPolicy,
    ) -> Self {
        Self::new_with_retention(extractor, store, policy, AssetRetentionPolicy::default())
    }

    #[must_use]
    pub fn new_with_retention(
        extractor: Arc<dyn AssetExtractionPort>,
        store: Arc<AssetStore>,
        policy: ClipPolicy,
        retention: AssetRetentionPolicy,
    ) -> Self {
        Self {
            extractor,
            store,
            policy,
            completed: Mutex::new(CompletedRequestCache::default()),
            in_flight: Mutex::new(HashMap::new()),
            retention,
            next_maintenance_epoch_s: AtomicU64::new(0),
        }
    }

    pub fn create(
        &self,
        session_id: MediaSessionId,
        cue: &SubtitleCueV1,
        observed_source_time_us: TimestampUs,
        media_duration_us: TimestampUs,
    ) -> Result<AssetBundle, AppErrorV1> {
        self.run_maintenance_if_due()?;
        let spec = self
            .policy
            .compute(cue, observed_source_time_us, media_duration_us)?;
        let extraction_spec = ExtractionSpec {
            session_id,
            start_us: spec.audio_start_us,
            end_us: spec.audio_end_us,
            frame_us: spec.frame_us,
            profile: spec.profile.clone(),
        };
        let request_key = request_key(&extraction_spec);
        let completed = self.completed.lock().get(&request_key);
        if let Some(bundle) = completed {
            let audio_present = self
                .store
                .find(&bundle.audio.hash, AssetKind::Audio)?
                .is_some();
            let image_present = self
                .store
                .find(&bundle.image.hash, AssetKind::Image)?
                .is_some();
            if audio_present && image_present {
                return Ok(bundle);
            }
            let mut completed = self.completed.lock();
            completed.remove_if_matches(&request_key, &bundle);
        }

        let (in_flight, leader) = {
            let mut requests = self.in_flight.lock();
            if let Some(existing) = requests.get(&request_key) {
                (existing.clone(), false)
            } else if let Some(bundle) = self.completed.lock().get(&request_key) {
                return Ok(bundle);
            } else {
                let request = Arc::new(InFlightRequest::default());
                requests.insert(request_key.clone(), request.clone());
                (request, true)
            }
        };
        if !leader {
            return in_flight.wait();
        }

        let result = self.create_uncached(&extraction_spec, &spec);
        if let Ok(bundle) = &result {
            self.completed
                .lock()
                .insert(request_key.clone(), bundle.clone());
        }
        in_flight.complete(&result);
        let mut requests = self.in_flight.lock();
        if requests
            .get(&request_key)
            .is_some_and(|current| Arc::ptr_eq(current, &in_flight))
        {
            requests.remove(&request_key);
        }
        result
    }

    fn create_uncached(
        &self,
        extraction_spec: &ExtractionSpec,
        spec: &crate::spec::AssetSpec,
    ) -> Result<AssetBundle, AppErrorV1> {
        let extracted = self.extractor.extract(extraction_spec)?;
        let result = (|| {
            validate_extraction_metadata(&extracted.metadata, extraction_spec)?;
            let audio = self.store.ingest(&extracted.audio_path, AssetKind::Audio)?;
            let image = match self.store.ingest(&extracted.image_path, AssetKind::Image) {
                Ok(image) => image,
                Err(error) => {
                    self.store.release(&audio.hash);
                    return Err(error);
                }
            };
            Ok((audio, image))
        })();
        remove_staging_assets(&extracted);
        let (audio, image) = result?;
        let bundle = AssetBundle {
            audio,
            image,
            audio_start_us: spec.audio_start_us,
            audio_end_us: spec.audio_end_us,
            frame_us: spec.frame_us,
        };
        Ok(bundle)
    }

    fn run_maintenance_if_due(&self) -> Result<(), AppErrorV1> {
        let now = epoch_seconds();
        loop {
            let due = self.next_maintenance_epoch_s.load(Ordering::Acquire);
            if due != 0 && now < due {
                return Ok(());
            }
            let interval = self.retention.maintenance_interval.as_secs().max(1);
            let next = now.saturating_add(interval);
            if self
                .next_maintenance_epoch_s
                .compare_exchange(due, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let result = self.store.cleanup_orphans(
                    self.retention.minimum_orphan_age,
                    self.retention.max_removals_per_run,
                );
                if result.is_err() {
                    self.next_maintenance_epoch_s.store(0, Ordering::Release);
                }
                return result.map(|_| ());
            }
        }
    }

    pub fn create_for_owner(
        &self,
        owner: &str,
        session_id: MediaSessionId,
        cue: &SubtitleCueV1,
        observed_source_time_us: TimestampUs,
        media_duration_us: TimestampUs,
    ) -> Result<AssetBundle, AppErrorV1> {
        let bundle = self.create(session_id, cue, observed_source_time_us, media_duration_us)?;
        if let Err(error) = self.store.protect_durable(owner, &bundle.audio.hash) {
            self.store.release(&bundle.audio.hash);
            self.store.release(&bundle.image.hash);
            return Err(error);
        }
        if let Err(error) = self.store.protect_durable(owner, &bundle.image.hash) {
            let _ = self.store.release_durable(owner, &bundle.audio.hash);
            self.store.release(&bundle.audio.hash);
            self.store.release(&bundle.image.hash);
            return Err(error);
        }
        self.store.release(&bundle.audio.hash);
        self.store.release(&bundle.image.hash);
        Ok(bundle)
    }
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn remove_staging_assets(extracted: &ports::ExtractedAssets) {
    let _ = std::fs::remove_file(&extracted.audio_path);
    if extracted.image_path != extracted.audio_path {
        let _ = std::fs::remove_file(&extracted.image_path);
    }
}

fn request_key(spec: &ExtractionSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-extraction-v1\0");
    update_component(&mut hasher, spec.session_id.as_str().as_bytes());
    hasher.update(spec.start_us.to_le_bytes());
    hasher.update(spec.end_us.to_le_bytes());
    hasher.update(spec.frame_us.to_le_bytes());
    update_component(&mut hasher, spec.profile.as_bytes());
    hex::encode(hasher.finalize())
}

fn update_component(hasher: &mut Sha256, component: &[u8]) {
    hasher.update((component.len() as u64).to_le_bytes());
    hasher.update(component);
}

fn validate_extraction_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
    spec: &ExtractionSpec,
) -> Result<(), AppErrorV1> {
    for required in [
        "audio_duration_us",
        "audio_mime",
        "image_mime",
        "image_width",
        "image_height",
    ] {
        if !metadata.contains_key(required) {
            return Err(metadata_error(format!(
                "Extracted asset metadata omitted required field {required}."
            )));
        }
    }
    let value = metadata
        .get("audio_duration_us")
        .ok_or_else(|| metadata_error("Extracted audio duration was missing."))?;
    let actual = value.parse::<TimestampUs>().map_err(metadata_error)?;
    let expected = spec.end_us.saturating_sub(spec.start_us);
    if actual <= 0 || actual.abs_diff(expected) > 500_000 {
        return Err(metadata_error(
            "Extracted audio duration did not match the canonical source interval.",
        ));
    }
    if let Some(value) = metadata.get("audio_mime")
        && !matches!(value.as_str(), "audio/mpeg" | "audio/ogg" | "audio/opus")
    {
        return Err(metadata_error("Extracted audio MIME type was unexpected."));
    }
    if let Some(value) = metadata.get("image_mime")
        && !matches!(value.as_str(), "image/jpeg" | "image/png")
    {
        return Err(metadata_error("Extracted image MIME type was unexpected."));
    }
    for key in ["image_width", "image_height"] {
        if let Some(value) = metadata.get(key) {
            let dimension = value.parse::<u32>().map_err(metadata_error)?;
            if dimension == 0 || dimension > 16_384 {
                return Err(metadata_error("Extracted image dimensions were invalid."));
            }
        }
    }
    Ok(())
}

fn metadata_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        contracts::error_codes::CONVERSION_FAILED,
        "Extracted mining media did not pass validation.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::PathBuf,
        sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
            mpsc::{self, Receiver},
        },
        time::Duration,
    };

    use contracts::{CueId, SubtitleStyleHintV1};
    use ports::ExtractedAssets;

    use super::*;

    #[derive(Debug)]
    struct FakeExtractor {
        calls: AtomicUsize,
        audio: PathBuf,
        image: PathBuf,
        metadata: BTreeMap<String, String>,
    }

    impl AssetExtractionPort for FakeExtractor {
        fn extract(&self, _spec: &ExtractionSpec) -> Result<ExtractedAssets, AppErrorV1> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(ExtractedAssets {
                audio_path: self.audio.clone(),
                image_path: self.image.clone(),
                metadata: self.metadata.clone(),
            })
        }
    }

    #[derive(Debug, Default)]
    struct ReleaseGate {
        released: Mutex<bool>,
        ready: Condvar,
    }

    impl ReleaseGate {
        fn wait(&self) {
            let mut released = self.released.lock();
            while !*released {
                self.ready.wait(&mut released);
            }
        }

        fn release(&self) {
            *self.released.lock() = true;
            self.ready.notify_all();
        }
    }

    #[derive(Debug)]
    struct BlockingExtractor {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        started: std::sync::mpsc::Sender<()>,
        gate: Arc<ReleaseGate>,
        output_root: PathBuf,
    }

    impl AssetExtractionPort for BlockingExtractor {
        fn extract(&self, spec: &ExtractionSpec) -> Result<ExtractedAssets, AppErrorV1> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            let _ = self.started.send(());
            self.gate.wait();

            let audio = self.output_root.join(format!("audio-{call}.mp3"));
            let image = self.output_root.join(format!("image-{call}.png"));
            fs::write(&audio, b"ID3fake audio").map_err(metadata_error)?;
            fs::write(&image, [137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4])
                .map_err(metadata_error)?;
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(ExtractedAssets {
                audio_path: audio,
                image_path: image,
                metadata: valid_metadata(spec.end_us.saturating_sub(spec.start_us)),
            })
        }
    }

    type BlockingServiceFixture = (
        Arc<BlockingExtractor>,
        Arc<MiningAssetService>,
        Receiver<()>,
        Arc<ReleaseGate>,
    );

    fn valid_metadata(duration_us: TimestampUs) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("audio_duration_us".into(), duration_us.to_string()),
            ("audio_mime".into(), "audio/mpeg".into()),
            ("image_mime".into(), "image/png".into()),
            ("image_width".into(), "1".into()),
            ("image_height".into(), "1".into()),
        ])
    }

    fn blocking_service(root: &std::path::Path) -> Result<BlockingServiceFixture, AppErrorV1> {
        let (started, receiver) = mpsc::channel();
        let gate = Arc::new(ReleaseGate::default());
        let extractor = Arc::new(BlockingExtractor {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            started,
            gate: gate.clone(),
            output_root: root.to_owned(),
        });
        let service = Arc::new(MiningAssetService::new(
            extractor.clone(),
            Arc::new(AssetStore::open(root.join("assets"))?),
            ClipPolicy::default(),
        ));
        Ok((extractor, service, receiver, gate))
    }

    fn create_on_thread(
        service: Arc<MiningAssetService>,
        barrier: Arc<Barrier>,
        session: &'static str,
    ) -> std::thread::JoinHandle<Result<AssetBundle, AppErrorV1>> {
        std::thread::spawn(move || {
            barrier.wait();
            service.create(MediaSessionId::new(session), &cue(), 1_750_000, 10_000_000)
        })
    }

    fn join_result(
        handle: std::thread::JoinHandle<Result<AssetBundle, AppErrorV1>>,
    ) -> Result<AssetBundle, AppErrorV1> {
        handle
            .join()
            .map_err(|_| metadata_error("A mining test worker panicked."))?
    }

    fn cue() -> SubtitleCueV1 {
        SubtitleCueV1 {
            cue_id: CueId::new("cue"),
            start_us: 1_000_000,
            end_us: 2_000_000,
            plain_text: "見る".into(),
            source_text: "見る".into(),
            track_order: 0,
            style_hint: SubtitleStyleHintV1::default(),
        }
    }

    fn service(
        root: &std::path::Path,
        mut metadata: BTreeMap<String, String>,
    ) -> Result<(Arc<FakeExtractor>, MiningAssetService), AppErrorV1> {
        for (key, value) in [
            ("audio_duration_us", "1750000"),
            ("audio_mime", "audio/mpeg"),
            ("image_mime", "image/png"),
            ("image_width", "1"),
            ("image_height", "1"),
        ] {
            metadata.entry(key.into()).or_insert_with(|| value.into());
        }
        let audio = root.join("audio.mp3");
        let image = root.join("image.png");
        fs::write(&audio, b"ID3fake audio").map_err(metadata_error)?;
        fs::write(&image, [137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4]).map_err(metadata_error)?;
        let extractor = Arc::new(FakeExtractor {
            calls: AtomicUsize::new(0),
            audio,
            image,
            metadata,
        });
        let service = MiningAssetService::new(
            extractor.clone(),
            Arc::new(AssetStore::open(root.join("assets"))?),
            ClipPolicy::default(),
        );
        Ok((extractor, service))
    }

    #[test]
    fn repeated_request_is_idempotent_and_uses_canonical_time() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(metadata_error)?;
        let (extractor, service) = service(temporary.path(), BTreeMap::new())?;
        let first = service.create(
            MediaSessionId::new("session"),
            &cue(),
            1_750_000,
            10_000_000,
        )?;
        let second = service.create(
            MediaSessionId::new("session"),
            &cue(),
            1_750_000,
            10_000_000,
        )?;
        assert_eq!(first, second);
        assert_eq!(first.frame_us, 1_750_000);
        assert_eq!(extractor.calls.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn rejects_inconsistent_extractor_metadata() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(metadata_error)?;
        let metadata = BTreeMap::from([("audio_duration_us".into(), "10".into())]);
        let (_, service) = service(temporary.path(), metadata)?;
        assert!(
            service
                .create(
                    MediaSessionId::new("session"),
                    &cue(),
                    1_500_000,
                    10_000_000
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn distinct_requests_are_not_serialized_by_the_idempotency_cache() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(metadata_error)?;
        let (extractor, service, started, gate) = blocking_service(temporary.path())?;
        let barrier = Arc::new(Barrier::new(3));
        let first = create_on_thread(service.clone(), barrier.clone(), "session-a");
        let second = create_on_thread(service, barrier.clone(), "session-b");
        barrier.wait();

        let first_started = started.recv_timeout(Duration::from_secs(2));
        let second_started = started.recv_timeout(Duration::from_secs(2));
        gate.release();
        let first_result = join_result(first);
        let second_result = join_result(second);

        assert!(first_started.is_ok());
        assert!(second_started.is_ok());
        first_result?;
        second_result?;
        assert_eq!(extractor.calls.load(Ordering::Acquire), 2);
        assert_eq!(extractor.max_active.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[test]
    fn concurrent_identical_requests_share_one_extraction() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(metadata_error)?;
        let (extractor, service, started, gate) = blocking_service(temporary.path())?;
        let barrier = Arc::new(Barrier::new(3));
        let first = create_on_thread(service.clone(), barrier.clone(), "same-session");
        let second = create_on_thread(service, barrier.clone(), "same-session");
        barrier.wait();

        let first_started = started.recv_timeout(Duration::from_secs(2));
        let duplicate_started = started.recv_timeout(Duration::from_millis(200));
        gate.release();
        let first_result = join_result(first)?;
        let second_result = join_result(second)?;

        assert!(first_started.is_ok());
        assert!(duplicate_started.is_err());
        assert_eq!(first_result, second_result);
        assert_eq!(extractor.calls.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[test]
    fn automatic_cleanup_removes_only_unreferenced_old_assets() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(metadata_error)?;
        let root = temporary.path();
        let store = Arc::new(AssetStore::open(root.join("assets"))?);
        let orphan_source = root.join("orphan.png");
        let referenced_source = root.join("referenced.png");
        fs::write(
            &orphan_source,
            [137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4],
        )
        .map_err(metadata_error)?;
        fs::write(
            &referenced_source,
            [137, 80, 78, 71, 13, 10, 26, 10, 5, 6, 7, 8],
        )
        .map_err(metadata_error)?;
        let orphan = store.ingest(&orphan_source, AssetKind::Image)?;
        let referenced = store.ingest(&referenced_source, AssetKind::Image)?;
        store.protect_durable("published_note", &referenced.hash)?;
        store.release(&orphan.hash);
        store.release(&referenced.hash);

        let audio = root.join("audio.mp3");
        let image = root.join("image.png");
        fs::write(&audio, b"ID3fake audio").map_err(metadata_error)?;
        fs::write(&image, [137, 80, 78, 71, 13, 10, 26, 10, 9, 10, 11, 12])
            .map_err(metadata_error)?;
        let extractor = Arc::new(FakeExtractor {
            calls: AtomicUsize::new(0),
            audio,
            image,
            metadata: valid_metadata(1_750_000),
        });
        let service = MiningAssetService::new_with_retention(
            extractor,
            store,
            ClipPolicy::default(),
            AssetRetentionPolicy {
                minimum_orphan_age: Duration::ZERO,
                maintenance_interval: Duration::from_secs(60),
                max_removals_per_run: 64,
            },
        );
        service.create(
            MediaSessionId::new("session"),
            &cue(),
            1_750_000,
            10_000_000,
        )?;

        assert!(!orphan.path.exists());
        assert!(referenced.path.exists());
        Ok(())
    }

    #[test]
    fn completed_request_cache_is_bounded() {
        let mut cache = CompletedRequestCache::default();
        for index in 0..=MAX_COMPLETED_REQUESTS {
            let key = format!("request-{index}");
            cache.insert(
                key,
                AssetBundle {
                    audio: StoredAsset {
                        hash: format!("audio-{index}"),
                        media_name: format!("audio-{index}.mp3"),
                        path: PathBuf::from(format!("audio-{index}.mp3")),
                        size_bytes: 1,
                        kind: AssetKind::Audio,
                    },
                    image: StoredAsset {
                        hash: format!("image-{index}"),
                        media_name: format!("image-{index}.png"),
                        path: PathBuf::from(format!("image-{index}.png")),
                        size_bytes: 1,
                        kind: AssetKind::Image,
                    },
                    audio_start_us: 0,
                    audio_end_us: 1,
                    frame_us: 0,
                },
            );
        }

        assert_eq!(cache.entries.len(), MAX_COMPLETED_REQUESTS);
        assert!(!cache.entries.contains_key("request-0"));
        assert!(
            cache
                .entries
                .contains_key(&format!("request-{MAX_COMPLETED_REQUESTS}"))
        );
        assert_eq!(cache.insertion_order.len(), cache.entries.len());
    }

    #[test]
    fn invalidated_completed_requests_do_not_leak_order_entries() {
        let mut cache = CompletedRequestCache::default();
        let bundle = AssetBundle {
            audio: StoredAsset {
                hash: "audio".into(),
                media_name: "audio.mp3".into(),
                path: PathBuf::from("audio.mp3"),
                size_bytes: 1,
                kind: AssetKind::Audio,
            },
            image: StoredAsset {
                hash: "image".into(),
                media_name: "image.jpg".into(),
                path: PathBuf::from("image.jpg"),
                size_bytes: 1,
                kind: AssetKind::Image,
            },
            audio_start_us: 0,
            audio_end_us: 1,
            frame_us: 0,
        };

        for _ in 0..MAX_COMPLETED_REQUESTS * 2 {
            cache.insert("request".into(), bundle.clone());
            cache.remove_if_matches("request", &bundle);
        }

        assert!(cache.entries.is_empty());
        assert!(cache.insertion_order.is_empty());
    }
}
