//! Subtitle parsing, safe text normalization, source discovery, and timelines.

pub mod discovery;
pub mod formats;
pub mod source;
pub mod timeline;

use contracts::{AppErrorV1, SubtitleCueV1, SubtitleFormatV1, error_codes};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedSubtitles {
    pub cues: Vec<SubtitleCueV1>,
    pub warnings: Vec<String>,
}

pub fn parse(
    format: SubtitleFormatV1,
    source_version: &str,
    input: &str,
) -> Result<ParsedSubtitles, AppErrorV1> {
    match format {
        SubtitleFormatV1::Srt => formats::srt::parse(source_version, input),
        SubtitleFormatV1::Vtt => formats::vtt::parse(source_version, input),
        SubtitleFormatV1::Ass | SubtitleFormatV1::Ssa => formats::ass::parse(source_version, input),
    }
}

pub(crate) fn subtitle_error(message: impl Into<String>) -> AppErrorV1 {
    AppErrorV1::new(error_codes::INVALID_REQUEST, message, false)
}
