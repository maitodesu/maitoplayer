use contracts::SubtitleStyleHintV1;

use crate::{
    ParsedSubtitles,
    formats::{
        MAX_CUE_BYTES, MAX_CUES, MAX_SUBTITLE_BYTES, make_cue, normalize_lines, parse_hms,
        strip_basic_markup,
    },
    subtitle_error,
};

pub fn parse(source_version: &str, input: &str) -> Result<ParsedSubtitles, contracts::AppErrorV1> {
    if input.len() > MAX_SUBTITLE_BYTES {
        return Err(subtitle_error(
            "Subtitle file exceeds the 32 MiB safety limit.",
        ));
    }
    let normalized = normalize_lines(input);
    let mut blocks = normalized.split("\n\n");
    let header = blocks.next().unwrap_or_default();
    if !header.trim_start().starts_with("WEBVTT") {
        return Err(subtitle_error("WebVTT header is missing."));
    }
    let mut result = ParsedSubtitles::default();
    for (block_index, block) in blocks.enumerate() {
        let lines: Vec<&str> = block.lines().collect();
        if lines.is_empty() || is_metadata_block(lines[0]) {
            continue;
        }
        let Some(timing_index) = lines.iter().position(|line| line.contains("-->")) else {
            result.warnings.push(format!(
                "Skipped WebVTT block {} without timing.",
                block_index + 1
            ));
            continue;
        };
        let Some((start, end_and_settings)) = lines[timing_index].split_once("-->") else {
            continue;
        };
        let end = end_and_settings
            .split_whitespace()
            .next()
            .unwrap_or_default();
        let (Some(start_us), Some(end_us)) = (parse_hms(start, '.'), parse_hms(end, '.')) else {
            result.warnings.push(format!(
                "Skipped WebVTT block {} with invalid timing.",
                block_index + 1
            ));
            continue;
        };
        if end_us <= start_us {
            result.warnings.push(format!(
                "Skipped WebVTT block {} with empty timing.",
                block_index + 1
            ));
            continue;
        }
        let source_text = lines[(timing_index + 1)..].join("\n");
        if source_text.len() > MAX_CUE_BYTES {
            result
                .warnings
                .push(format!("Skipped oversized WebVTT cue {}.", block_index + 1));
            continue;
        }
        let plain_text = strip_basic_markup(&source_text);
        if plain_text.trim().is_empty() {
            result.warnings.push(format!(
                "Skipped WebVTT cue {} without visible text.",
                block_index + 1
            ));
            continue;
        }
        let identity = if timing_index == 1 {
            lines[0].to_owned()
        } else {
            format!("block-{block_index}")
        };
        if result.cues.len() == MAX_CUES {
            return Err(subtitle_error(
                "Subtitle file exceeds the 100,000 cue safety limit.",
            ));
        }
        result.cues.push(make_cue(
            source_version,
            &identity,
            start_us,
            end_us,
            source_text,
            plain_text,
            block_index as u32,
            SubtitleStyleHintV1::default(),
        ));
    }
    if result.cues.is_empty() {
        return Err(subtitle_error("No valid subtitle cues were found."));
    }
    Ok(result)
}

fn is_metadata_block(first_line: &str) -> bool {
    let first_line = first_line.trim();
    first_line == "STYLE"
        || first_line == "REGION"
        || first_line == "NOTE"
        || first_line
            .strip_prefix("NOTE")
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_identifier_settings_and_two_component_timestamp() -> Result<(), contracts::AppErrorV1>
    {
        let parsed = parse(
            "source",
            "WEBVTT\n\nid-a\n00:01.500 --> 00:03.000 align:start\n<v Aoi>見る</v>",
        )?;
        assert_eq!(parsed.cues[0].start_us, 1_500_000);
        assert_eq!(parsed.cues[0].plain_text, "見る");
        Ok(())
    }

    #[test]
    fn preserves_non_tag_angle_brackets_and_decodes_entities() -> Result<(), contracts::AppErrorV1>
    {
        let parsed = parse(
            "source",
            "WEBVTT\n\n00:01.500 --> 00:03.000\n1 < 2 &amp;&amp; <ruby>日<rt>にち</rt></ruby>",
        )?;
        assert_eq!(parsed.cues[0].plain_text, "1 < 2 && 日にち");
        Ok(())
    }

    #[test]
    fn notebook_is_a_cue_identifier_not_a_note() -> Result<(), contracts::AppErrorV1> {
        let parsed = parse(
            "source",
            "WEBVTT\n\nNOTEBOOK\n00:01.000 --> 00:02.000\n本文",
        )?;
        assert_eq!(parsed.cues[0].plain_text, "本文");
        Ok(())
    }
}
