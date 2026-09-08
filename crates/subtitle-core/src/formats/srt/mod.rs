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
    let mut result = ParsedSubtitles::default();
    for (block_index, block) in normalized.split("\n\n").enumerate() {
        let lines: Vec<&str> = block.lines().collect();
        if lines.iter().all(|line| line.trim().is_empty()) {
            continue;
        }
        let timing_index = lines.iter().position(|line| line.contains("-->"));
        let Some(timing_index) = timing_index else {
            result.warnings.push(format!(
                "Skipped SRT block {} without timing.",
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
        let (Some(start_us), Some(end_us)) = (parse_hms(start, ','), parse_hms(end, ',')) else {
            result.warnings.push(format!(
                "Skipped SRT block {} with invalid timing.",
                block_index + 1
            ));
            continue;
        };
        if start_us < 0 || end_us <= start_us {
            result.warnings.push(format!(
                "Skipped SRT block {} with empty timing.",
                block_index + 1
            ));
            continue;
        }
        let source_text = lines[(timing_index + 1)..].join("\n");
        if source_text.len() > MAX_CUE_BYTES {
            result
                .warnings
                .push(format!("Skipped oversized SRT cue {}.", block_index + 1));
            continue;
        }
        let plain_text = strip_basic_markup(&source_text);
        if plain_text.trim().is_empty() {
            result.warnings.push(format!(
                "Skipped SRT block {} without visible text.",
                block_index + 1
            ));
            continue;
        }
        let identity = lines.first().filter(|_| timing_index == 1).map_or_else(
            || format!("block-{block_index}"),
            |value| (*value).to_owned(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bom_crlf_multiline_and_half_open_times() -> Result<(), contracts::AppErrorV1> {
        let parsed = parse(
            "source-v1",
            "\u{feff}1\r\n00:00:01,250 --> 00:00:03,000\r\n<b>日本語</b>\r\n二行目\r\n",
        )?;
        assert_eq!(parsed.cues[0].start_us, 1_250_000);
        assert_eq!(parsed.cues[0].end_us, 3_000_000);
        assert_eq!(parsed.cues[0].plain_text, "日本語\n二行目");
        Ok(())
    }

    #[test]
    fn malformed_block_is_a_warning() -> Result<(), contracts::AppErrorV1> {
        let parsed = parse(
            "source-v1",
            "broken\n\n1\n00:00:01,000 --> 00:00:02,000\n有効",
        )?;
        assert_eq!(parsed.cues.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
        Ok(())
    }

    #[test]
    fn duplicate_source_identifiers_still_produce_unique_cue_ids()
    -> Result<(), contracts::AppErrorV1> {
        let parsed = parse(
            "source-v1",
            "1\n00:00:01,000 --> 00:00:02,000\n一\n\n1\n00:00:03,000 --> 00:00:04,000\n二",
        )?;
        assert_ne!(parsed.cues[0].cue_id, parsed.cues[1].cue_id);
        Ok(())
    }

    #[test]
    fn whitespace_only_separator_ends_a_cue() -> Result<(), contracts::AppErrorV1> {
        let parsed = parse(
            "source-v1",
            "1\n00:00:01,000 --> 00:00:02,000\n一\n  \n2\n00:00:03,000 --> 00:00:04,000\n二",
        )?;
        assert_eq!(parsed.cues.len(), 2);
        Ok(())
    }
}
