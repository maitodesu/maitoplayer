use std::{
    fs,
    path::{Path, PathBuf},
};

use contracts::{AppErrorV1, SubtitleFormatV1, error_codes};

use crate::source::format_for_path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleCandidate {
    pub path: PathBuf,
    pub format: SubtitleFormatV1,
    pub language: Option<String>,
    pub selected: bool,
    pub persisted_binding: bool,
}

pub fn discover_adjacent(media_path: &Path) -> Result<Vec<SubtitleCandidate>, AppErrorV1> {
    let directory = media_path.parent().ok_or_else(discovery_error)?;
    let media_stem = media_path
        .file_stem()
        .map(|value| value.to_string_lossy().to_lowercase())
        .ok_or_else(discovery_error)?;
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| discovery_error())? {
        let entry = entry.map_err(|_| discovery_error())?;
        if !entry.file_type().map_err(|_| discovery_error())?.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(format) = format_for_path(&path) else {
            continue;
        };
        let Some(stem) = path
            .file_stem()
            .map(|value| value.to_string_lossy().to_lowercase())
        else {
            continue;
        };
        if stem == media_stem || stem.starts_with(&format!("{media_stem}.")) {
            let language = stem
                .strip_prefix(&format!("{media_stem}."))
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            candidates.push(SubtitleCandidate {
                path,
                format,
                language,
                selected: false,
                persisted_binding: false,
            });
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

pub fn choose<'a>(
    candidates: &'a [SubtitleCandidate],
    language_priority: &[String],
) -> Result<Option<&'a SubtitleCandidate>, AppErrorV1> {
    let Some(best_rank) = candidates
        .iter()
        .map(|candidate| rank(candidate, language_priority))
        .min()
    else {
        return Ok(None);
    };
    let best: Vec<_> = candidates
        .iter()
        .filter(|candidate| rank(candidate, language_priority) == best_rank)
        .collect();
    if best.len() > 1 {
        return Err(AppErrorV1::new(
            error_codes::SUBTITLE_AMBIGUOUS,
            "Multiple equally suitable subtitles were found. Choose one explicitly.",
            true,
        ));
    }
    Ok(best.first().copied())
}

fn rank(candidate: &SubtitleCandidate, language_priority: &[String]) -> (u8, usize) {
    let language = candidate
        .language
        .as_deref()
        .and_then(|value| language_rank(value, language_priority));
    let class = if candidate.selected {
        0
    } else if candidate.persisted_binding {
        1
    } else if language.is_some() {
        2
    } else if candidate.language.is_none() {
        3
    } else {
        4
    };
    let language = language.unwrap_or(usize::MAX);
    (class, language)
}

fn language_rank(language: &str, priorities: &[String]) -> Option<usize> {
    priorities.iter().position(|priority| {
        language.eq_ignore_ascii_case(priority)
            || language
                .split(['.', '-', '_'])
                .next()
                .is_some_and(|primary| primary.eq_ignore_ascii_case(priority))
    })
}

fn discovery_error() -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::MEDIA_SCOPE_DENIED,
        "Adjacent subtitles could not be inspected. Choose a subtitle file manually.",
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn explicit_selection_wins() -> Result<(), AppErrorV1> {
        let candidates = vec![
            SubtitleCandidate {
                path: "show.ja.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: Some("ja".into()),
                selected: false,
                persisted_binding: true,
            },
            SubtitleCandidate {
                path: "manual.ass".into(),
                format: SubtitleFormatV1::Ass,
                language: None,
                selected: true,
                persisted_binding: false,
            },
        ];
        assert_eq!(
            choose(&candidates, &["ja".into()])?.map(|item| &item.path),
            Some(&PathBuf::from("manual.ass"))
        );
        Ok(())
    }

    #[test]
    fn tied_candidates_require_user_selection() {
        let candidates = vec![
            SubtitleCandidate {
                path: "show.ja.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: Some("ja".into()),
                selected: false,
                persisted_binding: false,
            },
            SubtitleCandidate {
                path: "show.ja.ass".into(),
                format: SubtitleFormatV1::Ass,
                language: Some("ja".into()),
                selected: false,
                persisted_binding: false,
            },
        ];
        let error = choose(&candidates, &["ja".into()]);
        assert_eq!(
            error.err().map(|value| value.code),
            Some(error_codes::SUBTITLE_AMBIGUOUS.into())
        );
    }

    #[test]
    fn preferred_language_beats_unlabelled_and_unknown_language() -> Result<(), AppErrorV1> {
        let candidates = vec![
            SubtitleCandidate {
                path: "show.en.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: Some("en".into()),
                selected: false,
                persisted_binding: false,
            },
            SubtitleCandidate {
                path: "show.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: None,
                selected: false,
                persisted_binding: false,
            },
            SubtitleCandidate {
                path: "show.JA-forced.ass".into(),
                format: SubtitleFormatV1::Ass,
                language: Some("ja-forced".into()),
                selected: false,
                persisted_binding: false,
            },
        ];
        assert_eq!(
            choose(&candidates, &["ja".into()])?.map(|item| &item.path),
            Some(&PathBuf::from("show.JA-forced.ass"))
        );
        Ok(())
    }

    #[test]
    fn unknown_language_does_not_beat_exact_stem() -> Result<(), AppErrorV1> {
        let candidates = vec![
            SubtitleCandidate {
                path: "show.en.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: Some("en".into()),
                selected: false,
                persisted_binding: false,
            },
            SubtitleCandidate {
                path: "show.srt".into(),
                format: SubtitleFormatV1::Srt,
                language: None,
                selected: false,
                persisted_binding: false,
            },
        ];
        assert_eq!(
            choose(&candidates, &["ja".into()])?.map(|item| &item.path),
            Some(&PathBuf::from("show.srt"))
        );
        Ok(())
    }

    #[test]
    fn discovery_is_deterministic_and_ignores_non_files() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let media = directory.path().join("Show.MKV");
        fs::write(&media, b"media")?;
        fs::write(directory.path().join("show.JA.srt"), b"subtitle")?;
        fs::write(directory.path().join("show.ass"), b"subtitle")?;
        fs::create_dir(directory.path().join("show.vtt"))?;
        let candidates = discover_adjacent(&media)?;
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].path < candidates[1].path);
        Ok(())
    }
}
