use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use contracts::{AppErrorV1, error_codes};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};

const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CLEANUP_SCAN: usize = 100_000;
const MAX_CLEANUP_REMOVALS_PER_RUN: usize = 256;
static TEMPORARY_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetKind {
    Audio,
    Image,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAsset {
    pub hash: String,
    pub media_name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: AssetKind,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    pub scanned: usize,
    pub removed_assets: usize,
    pub removed_partial_files: usize,
    pub bytes_reclaimed: u64,
}

#[derive(Debug)]
pub struct AssetStore {
    root: PathBuf,
    protected: RwLock<HashSet<String>>,
    maintenance: Mutex<()>,
}

impl AssetStore {
    pub fn open(root: PathBuf) -> Result<Self, AppErrorV1> {
        fs::create_dir_all(&root).map_err(store_error)?;
        fs::create_dir_all(root.join(".refs")).map_err(store_error)?;
        Ok(Self {
            root,
            protected: RwLock::new(HashSet::new()),
            maintenance: Mutex::new(()),
        })
    }

    pub fn ingest(&self, source: &Path, kind: AssetKind) -> Result<StoredAsset, AppErrorV1> {
        let _maintenance = self.maintenance.lock();
        let metadata = source.symlink_metadata().map_err(store_error)?;
        if !metadata.file_type().is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_ASSET_BYTES
        {
            return Err(store_error("Extracted asset failed file-size validation."));
        }
        let extension = validated_extension(source, kind)?;
        validate_signature(source, kind)?;
        let hash = hash_file(source)?;
        let media_name = format!("migaku-{hash}.{extension}");
        let final_path = self.root.join(&media_name);
        if final_path.exists() {
            validate_existing(&final_path, &hash, kind)?;
        } else {
            self.copy_and_promote(source, &final_path, &media_name, &hash, kind)?;
        }
        let size_bytes = final_path.metadata().map_err(store_error)?.len();
        self.protected.write().insert(hash.clone());
        Ok(StoredAsset {
            hash,
            media_name,
            path: final_path,
            size_bytes,
            kind,
        })
    }

    pub fn find(&self, hash: &str, kind: AssetKind) -> Result<Option<StoredAsset>, AppErrorV1> {
        validate_hash(hash)?;
        for extension in extensions(kind) {
            let media_name = format!("migaku-{hash}.{extension}");
            let path = self.root.join(&media_name);
            if path.exists() {
                validate_existing(&path, hash, kind)?;
                return Ok(Some(StoredAsset {
                    size_bytes: path.metadata().map_err(store_error)?.len(),
                    hash: hash.to_owned(),
                    media_name,
                    path,
                    kind,
                }));
            }
        }
        Ok(None)
    }

    pub fn protect(&self, hash: &str) {
        let _maintenance = self.maintenance.lock();
        self.protected.write().insert(hash.to_owned());
    }

    pub fn release(&self, hash: &str) {
        let _maintenance = self.maintenance.lock();
        self.protected.write().remove(hash);
    }

    pub fn protect_durable(&self, owner: &str, hash: &str) -> Result<(), AppErrorV1> {
        validate_owner(owner)?;
        validate_hash(hash)?;
        let _maintenance = self.maintenance.lock();
        if !self.asset_exists(hash) {
            return Err(store_error(
                "A reference cannot protect an asset that is not present.",
            ));
        }
        let owner_root = self.root.join(".refs").join(owner);
        fs::create_dir_all(&owner_root).map_err(store_error)?;
        let final_path = owner_root.join(hash);
        if final_path.exists() {
            return Ok(());
        }
        let temporary = owner_root.join(format!(".{hash}.part-{}", next_nonce()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(store_error)?;
        file.write_all(b"v1\n").map_err(store_error)?;
        file.sync_all().map_err(store_error)?;
        match fs::rename(&temporary, &final_path) {
            Ok(()) => Ok(()),
            Err(_) if final_path.exists() => {
                let _ = fs::remove_file(&temporary);
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(store_error(error))
            }
        }
    }

    pub fn release_durable(&self, owner: &str, hash: &str) -> Result<(), AppErrorV1> {
        validate_owner(owner)?;
        validate_hash(hash)?;
        let _maintenance = self.maintenance.lock();
        let path = self.root.join(".refs").join(owner).join(hash);
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }

    pub fn cleanup_orphans(
        &self,
        minimum_age: Duration,
        max_removals: usize,
    ) -> Result<CleanupReport, AppErrorV1> {
        let _maintenance = self.maintenance.lock();
        let protected = self.protected.read().clone();
        let mut report = CleanupReport::default();
        let max_removals = max_removals.min(MAX_CLEANUP_REMOVALS_PER_RUN);
        if max_removals == 0 {
            return Ok(report);
        }
        for entry in fs::read_dir(&self.root).map_err(store_error)? {
            if report.scanned >= MAX_CLEANUP_SCAN
                || report.removed_assets + report.removed_partial_files >= max_removals
            {
                break;
            }
            let entry = entry.map_err(store_error)?;
            let metadata = entry.metadata().map_err(store_error)?;
            if !metadata.is_file() {
                continue;
            }
            report.scanned += 1;
            if file_age(&metadata) < minimum_age {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".part-") {
                fs::remove_file(entry.path()).map_err(store_error)?;
                report.removed_partial_files += 1;
                report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(metadata.len());
                continue;
            }
            let Some(hash) = asset_hash_from_name(&name) else {
                continue;
            };
            if protected.contains(hash) || self.has_durable_reference(hash)? {
                continue;
            }
            fs::remove_file(entry.path()).map_err(store_error)?;
            report.removed_assets += 1;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(metadata.len());
        }
        Ok(report)
    }

    fn copy_and_promote(
        &self,
        source: &Path,
        final_path: &Path,
        media_name: &str,
        hash: &str,
        kind: AssetKind,
    ) -> Result<(), AppErrorV1> {
        let temporary = self
            .root
            .join(format!(".{media_name}.part-{}", next_nonce()));
        fs::copy(source, &temporary).map_err(store_error)?;
        let temporary_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(store_error)?;
        temporary_file.sync_all().map_err(store_error)?;
        if hash_file(&temporary)? != hash {
            let _ = fs::remove_file(&temporary);
            return Err(store_error("Asset changed while it was being copied."));
        }
        validate_signature(&temporary, kind)?;
        match fs::rename(&temporary, final_path) {
            Ok(()) => Ok(()),
            Err(_) if final_path.exists() => {
                let _ = fs::remove_file(&temporary);
                validate_existing(final_path, hash, kind)
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(store_error(error))
            }
        }
    }

    fn asset_exists(&self, hash: &str) -> bool {
        ["mp3", "ogg", "jpg", "png"].iter().any(|extension| {
            self.root
                .join(format!("migaku-{hash}.{extension}"))
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_file())
        })
    }

    fn has_durable_reference(&self, hash: &str) -> Result<bool, AppErrorV1> {
        let refs_root = self.root.join(".refs");
        for (scanned, entry) in fs::read_dir(refs_root).map_err(store_error)?.enumerate() {
            if scanned >= MAX_CLEANUP_SCAN {
                return Err(store_error(
                    "Asset reference scan exceeded its safety limit.",
                ));
            }
            let entry = entry.map_err(store_error)?;
            if entry.file_type().map_err(store_error)?.is_dir() && entry.path().join(hash).is_file()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn next_nonce() -> u64 {
    TEMPORARY_NONCE.fetch_add(1, Ordering::Relaxed)
}

fn hash_file(path: &Path) -> Result<String, AppErrorV1> {
    let mut file = File::open(path).map_err(store_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(store_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn validated_extension(path: &Path, kind: AssetKind) -> Result<&'static str, AppErrorV1> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match (kind, extension.as_deref()) {
        (AssetKind::Audio, Some("mp3")) => Ok("mp3"),
        (AssetKind::Audio, Some("ogg" | "opus")) => Ok("ogg"),
        (AssetKind::Image, Some("jpg" | "jpeg")) => Ok("jpg"),
        (AssetKind::Image, Some("png")) => Ok("png"),
        _ => Err(store_error(
            "Extracted asset type did not match its requested profile.",
        )),
    }
}

fn extensions(kind: AssetKind) -> &'static [&'static str] {
    match kind {
        AssetKind::Audio => &["mp3", "ogg"],
        AssetKind::Image => &["jpg", "png"],
    }
}

fn validate_signature(path: &Path, kind: AssetKind) -> Result<(), AppErrorV1> {
    let mut prefix = [0_u8; 12];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut prefix))
        .map_err(store_error)?;
    let valid = match kind {
        AssetKind::Audio => {
            (read >= 3 && &prefix[..3] == b"ID3")
                || (read >= 2 && prefix[0] == 0xff && prefix[1] & 0xe0 == 0xe0)
                || (read >= 4 && &prefix[..4] == b"OggS")
        }
        AssetKind::Image => {
            (read >= 3 && prefix[..3] == [0xff, 0xd8, 0xff])
                || (read >= 8 && prefix[..8] == [137, 80, 78, 71, 13, 10, 26, 10])
        }
    };
    if !valid {
        return Err(store_error(
            "Extracted asset contents did not match the requested media type.",
        ));
    }
    Ok(())
}

fn validate_existing(path: &Path, hash: &str, kind: AssetKind) -> Result<(), AppErrorV1> {
    let metadata = path.symlink_metadata().map_err(store_error)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_ASSET_BYTES {
        return Err(store_error("Cached asset failed file-size validation."));
    }
    validate_signature(path, kind)?;
    if hash_file(path)? != hash {
        return Err(store_error(
            "Cached asset content did not match its content-addressed name.",
        ));
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<(), AppErrorV1> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(store_error("Asset hash was invalid."));
    }
    Ok(())
}

fn validate_owner(owner: &str) -> Result<(), AppErrorV1> {
    if owner.is_empty()
        || owner.len() > 128
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(store_error("Asset reference owner was invalid."));
    }
    Ok(())
}

fn asset_hash_from_name(name: &str) -> Option<&str> {
    let remainder = name.strip_prefix("migaku-")?;
    let (hash, extension) = remainder.rsplit_once('.')?;
    if !["mp3", "ogg", "jpg", "png"].contains(&extension) || validate_hash(hash).is_err() {
        return None;
    }
    Some(hash)
}

fn file_age(metadata: &fs::Metadata) -> Duration {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .unwrap_or_default()
}

fn store_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::CONVERSION_FAILED,
        "A mined media asset could not be validated or stored.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, discriminator: u8) -> std::io::Result<()> {
        let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
        bytes.extend([discriminator; 16]);
        fs::write(path, bytes)
    }

    #[test]
    fn repeated_ingest_is_content_addressed_and_tampering_is_detected() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(store_error)?;
        let source = temporary.path().join("frame.png");
        write_png(&source, 7).map_err(store_error)?;
        let store = AssetStore::open(temporary.path().join("assets"))?;
        let first = store.ingest(&source, AssetKind::Image)?;
        let second = store.ingest(&source, AssetKind::Image)?;
        assert_eq!(first, second);

        write_png(&first.path, 9).map_err(store_error)?;
        assert!(store.ingest(&source, AssetKind::Image).is_err());
        Ok(())
    }

    #[test]
    fn durable_reference_survives_reopen_and_blocks_cleanup() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(store_error)?;
        let source = temporary.path().join("frame.png");
        write_png(&source, 4).map_err(store_error)?;
        let root = temporary.path().join("assets");
        let store = AssetStore::open(root.clone())?;
        let asset = store.ingest(&source, AssetKind::Image)?;
        store.protect_durable("history_42", &asset.hash)?;
        store.release(&asset.hash);
        drop(store);

        let reopened = AssetStore::open(root)?;
        assert_eq!(
            reopened.cleanup_orphans(Duration::ZERO, 10)?.removed_assets,
            0
        );
        reopened.release_durable("history_42", &asset.hash)?;
        assert_eq!(
            reopened.cleanup_orphans(Duration::ZERO, 10)?.removed_assets,
            1
        );
        Ok(())
    }

    #[test]
    fn rejects_extension_spoofing_and_reference_traversal() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(store_error)?;
        let source = temporary.path().join("fake.jpg");
        fs::write(&source, b"not an image").map_err(store_error)?;
        let store = AssetStore::open(temporary.path().join("assets"))?;
        assert!(store.ingest(&source, AssetKind::Image).is_err());
        assert!(store.protect_durable("../escape", &"a".repeat(64)).is_err());
        Ok(())
    }
}
