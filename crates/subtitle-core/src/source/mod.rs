use std::{fs::File, io::Read, path::Path};

use contracts::{AppErrorV1, SubtitleFormatV1, error_codes};
use sha2::{Digest, Sha256};

const MAX_SUBTITLE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSubtitle {
    pub text: String,
    pub format: SubtitleFormatV1,
    pub source_version: String,
}

pub fn load(path: &Path) -> Result<LoadedSubtitle, AppErrorV1> {
    let metadata = path.metadata().map_err(source_error)?;
    if !metadata.is_file() || metadata.len() > MAX_SUBTITLE_BYTES {
        return Err(AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "Select a subtitle file smaller than 32 MiB.",
            false,
        ));
    }
    let format = format_for_path(path).ok_or_else(|| {
        AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "Supported subtitle formats are SRT, ASS/SSA, and WebVTT.",
            false,
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(source_error)?
        .take(MAX_SUBTITLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(source_error)?;
    if bytes.len() as u64 > MAX_SUBTITLE_BYTES {
        return Err(AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "Select a subtitle file smaller than 32 MiB.",
            false,
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"subtitle-parser-v1\0");
    hasher.update(format_label(format).as_bytes());
    hasher.update([0]);
    hasher.update(&bytes);
    let source_version = format!("sp1:sha256:{}", hex::encode(hasher.finalize()));
    let text = String::from_utf8(bytes).map_err(|_| {
        AppErrorV1::new(
            error_codes::INVALID_REQUEST,
            "The subtitle file must be UTF-8 encoded.",
            false,
        )
    })?;
    Ok(LoadedSubtitle {
        text,
        format,
        source_version,
    })
}

fn format_label(format: SubtitleFormatV1) -> &'static str {
    match format {
        SubtitleFormatV1::Srt => "srt",
        SubtitleFormatV1::Ass => "ass",
        SubtitleFormatV1::Ssa => "ssa",
        SubtitleFormatV1::Vtt => "vtt",
    }
}

#[must_use]
pub fn format_for_path(path: &Path) -> Option<SubtitleFormatV1> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "srt" => Some(SubtitleFormatV1::Srt),
        "ass" => Some(SubtitleFormatV1::Ass),
        "ssa" => Some(SubtitleFormatV1::Ssa),
        "vtt" => Some(SubtitleFormatV1::Vtt),
        _ => None,
    }
}

fn source_error(error: std::io::Error) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "The subtitle file cannot be read. Locate it again.",
        true,
    )
    .with_diagnostics(error.kind().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn source_version_includes_parser_format() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let srt = directory.path().join("same.srt");
        let vtt = directory.path().join("same.vtt");
        fs::write(&srt, b"same bytes")?;
        fs::write(&vtt, b"same bytes")?;
        assert_ne!(load(&srt)?.source_version, load(&vtt)?.source_version);
        Ok(())
    }

    #[test]
    fn format_detection_is_case_insensitive() {
        assert_eq!(
            format_for_path(Path::new("episode.JA.SRT")),
            Some(SubtitleFormatV1::Srt)
        );
    }
}
