pub mod ass;
pub mod srt;
pub mod vtt;

use contracts::{CueId, SubtitleCueV1, SubtitleStyleHintV1};
use sha2::{Digest, Sha256};

pub(crate) const MAX_SUBTITLE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_CUE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_CUES: usize = 100_000;

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_cue(
    source_version: &str,
    identity: &str,
    start_us: i64,
    end_us: i64,
    source_text: String,
    plain_text: String,
    track_order: u32,
    style_hint: SubtitleStyleHintV1,
) -> SubtitleCueV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"maitoplayer-cue-v1\0");
    hasher.update(source_version.as_bytes());
    hasher.update([0]);
    hasher.update(identity.as_bytes());
    hasher.update(start_us.to_le_bytes());
    hasher.update(end_us.to_le_bytes());
    hasher.update(track_order.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    SubtitleCueV1 {
        cue_id: CueId::new(format!("cue_{}", &digest[..24])),
        start_us,
        end_us,
        plain_text,
        source_text,
        track_order,
        style_hint,
    }
}

pub(crate) fn normalize_lines(input: &str) -> String {
    let normalized = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut output = String::with_capacity(normalized.len());
    for (index, line) in normalized.split('\n').enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if !line.trim().is_empty() {
            output.push_str(line);
        }
    }
    output
}

pub(crate) fn strip_basic_markup(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        result.push_str(&rest[..start]);
        let candidate = &rest[(start + 1)..];
        let Some(end) = candidate.find('>') else {
            result.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = candidate[..end].trim_start();
        if is_markup_tag(tag) {
            rest = &candidate[(end + 1)..];
        } else {
            result.push('<');
            rest = candidate;
        }
    }
    result.push_str(rest);
    decode_entities(&result)
}

fn is_markup_tag(tag: &str) -> bool {
    let tag = tag.strip_prefix('/').unwrap_or(tag);
    tag.starts_with(|character: char| {
        character.is_ascii_alphabetic() || matches!(character, '!' | '?')
    }) || tag
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit() && tag.contains(':'))
}

fn decode_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let candidate = &rest[(start + 1)..];
        let Some(end) = candidate.find(';').filter(|end| *end <= 16) else {
            output.push('&');
            rest = candidate;
            continue;
        };
        let entity = &candidate[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "nbsp" => Some(' '),
            "quot" => Some('"'),
            "apos" => Some('\''),
            value if value.starts_with("#x") || value.starts_with("#X") => {
                u32::from_str_radix(&value[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            value if value.starts_with('#') => {
                value[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(character) = decoded {
            output.push(character);
        } else {
            output.push('&');
            output.push_str(entity);
            output.push(';');
        }
        rest = &candidate[(end + 1)..];
    }
    output.push_str(rest);
    output
}

pub(crate) fn parse_hms(value: &str, decimal_separator: char) -> Option<i64> {
    let normalized = value.trim().replace(decimal_separator, ".");
    let parts: Vec<&str> = normalized.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [hours, minutes, seconds] => (
            hours.parse::<i64>().ok()?,
            minutes.parse::<i64>().ok()?,
            *seconds,
        ),
        [minutes, seconds] => (0, minutes.parse::<i64>().ok()?, *seconds),
        _ => return None,
    };
    let (whole_seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, "0"));
    if !whole_seconds.chars().all(|value| value.is_ascii_digit())
        || fraction.is_empty()
        || fraction.len() > 6
        || !fraction.chars().all(|value| value.is_ascii_digit())
    {
        return None;
    }
    let seconds = whole_seconds.parse::<i64>().ok()?;
    if hours < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return None;
    }
    let fraction_us =
        fraction.parse::<i64>().ok()? * 10_i64.pow(6_u32.saturating_sub(fraction.len() as u32));
    hours
        .checked_mul(3_600_000_000)?
        .checked_add(minutes * 60_000_000)?
        .checked_add(seconds * 1_000_000)?
        .checked_add(fraction_us)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_stripping_preserves_comparisons_and_decodes_safe_entities() {
        assert_eq!(
            strip_basic_markup("1 < 2 &amp;&amp; <b>3</b> &#x65E5;&#26412;"),
            "1 < 2 && 3 日本"
        );
        assert_eq!(strip_basic_markup("unclosed <b"), "unclosed <b");
    }

    #[test]
    fn timestamp_parser_rejects_invalid_or_overprecise_values() {
        assert_eq!(parse_hms("00:01:02.123456", '.'), Some(62_123_456));
        assert_eq!(parse_hms("00:01:02.", '.'), None);
        assert_eq!(parse_hms("00:01:02.1234567", '.'), None);
        assert_eq!(parse_hms("00:61:02.000", '.'), None);
    }
}
