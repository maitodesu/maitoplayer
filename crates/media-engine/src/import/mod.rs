use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use contracts::{AppErrorV1, MediaSessionId, error_codes};
use sha2::{Digest, Sha256};

const SAMPLE_BYTES: usize = 64 * 1024;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024;
static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct ImportedSource {
    pub canonical_path: PathBuf,
    pub display_name: String,
    pub source_fingerprint: String,
    pub session_id: MediaSessionId,
    pub size_bytes: u64,
}

pub fn import_authorized_path(path: &Path) -> Result<ImportedSource, AppErrorV1> {
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        AppErrorV1::new(
            error_codes::MEDIA_SCOPE_DENIED,
            "The selected media file is unavailable. Locate it again.",
            true,
        )
        .with_diagnostics(error.kind().to_string())
    })?;
    let metadata = fs::metadata(&canonical_path).map_err(scope_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AppErrorV1::new(
            error_codes::MEDIA_SCOPE_DENIED,
            "Select a regular local media file.",
            false,
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
        return Err(AppErrorV1::new(
            error_codes::MEDIA_UNSUPPORTED,
            "The selected file is empty or exceeds the configured size policy.",
            false,
        ));
    }
    let display_name = canonical_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Selected media".to_owned());
    let source_fingerprint = sampled_fingerprint(&canonical_path, &metadata)?;
    let session_id = new_session_id(&source_fingerprint);
    Ok(ImportedSource {
        canonical_path,
        display_name,
        source_fingerprint,
        session_id,
        size_bytes: metadata.len(),
    })
}

fn sampled_fingerprint(path: &Path, metadata: &fs::Metadata) -> Result<String, AppErrorV1> {
    let mut file = File::open(path).map_err(scope_error)?;
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-source-v1\0");
    hasher.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified()
        && let Ok(elapsed) = modified.duration_since(UNIX_EPOCH)
    {
        hasher.update(elapsed.as_nanos().to_le_bytes());
    }
    let mut first = vec![0_u8; SAMPLE_BYTES.min(metadata.len() as usize)];
    file.read_exact(&mut first).map_err(scope_error)?;
    hasher.update(&first);
    if metadata.len() > SAMPLE_BYTES as u64 {
        let tail_size = SAMPLE_BYTES.min(metadata.len() as usize);
        file.seek(SeekFrom::End(-(tail_size as i64)))
            .map_err(scope_error)?;
        let mut last = vec![0_u8; tail_size];
        file.read_exact(&mut last).map_err(scope_error)?;
        hasher.update(&last);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn new_session_id(fingerprint: &str) -> MediaSessionId {
    let sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-session-v1\0");
    hasher.update(fingerprint.as_bytes());
    hasher.update(sequence.to_le_bytes());
    hasher.update(now.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    MediaSessionId::new(format!("ms_{}", &digest[..32]))
}

fn scope_error(error: std::io::Error) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "The selected media file cannot be read. Check its permissions or locate it again.",
        true,
    )
    .with_diagnostics(error.kind().to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn import_returns_safe_name_and_opaque_identity() -> Result<(), Box<dyn std::error::Error>> {
        let mut file = NamedTempFile::new()?;
        file.write_all(b"synthetic media bytes")?;
        let imported = import_authorized_path(file.path())?;
        let expected_name = file
            .path()
            .file_name()
            .ok_or_else(|| std::io::Error::other("temporary file has no name"))?
            .to_string_lossy();
        assert_eq!(imported.display_name, expected_name);
        assert!(!imported.source_fingerprint.contains(&imported.display_name));
        assert!(imported.session_id.as_str().starts_with("ms_"));
        Ok(())
    }
}
