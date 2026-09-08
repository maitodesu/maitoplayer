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
    let mut in_events = false;
    let mut fields: Vec<String> = vec![
        "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut result = ParsedSubtitles::default();
    for (line_number, line) in normalized.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_events = trimmed.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events || trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        let directive = trimmed.split_once(':');
        if let Some((_, value)) = directive.filter(|(name, _)| name.eq_ignore_ascii_case("format"))
        {
            fields = value
                .split(',')
                .map(|field| field.trim().to_ascii_lowercase())
                .collect();
            continue;
        }
        let Some((_, payload)) =
            directive.filter(|(name, _)| name.eq_ignore_ascii_case("dialogue"))
        else {
            continue;
        };
        let values: Vec<&str> = payload.splitn(fields.len(), ',').collect();
        if values.len() != fields.len() {
            result.warnings.push(format!(
                "Skipped ASS dialogue on line {} with missing fields.",
                line_number + 1
            ));
            continue;
        }
        let value = |name: &str| {
            fields
                .iter()
                .position(|field| field == name)
                .and_then(|index| values.get(index))
                .copied()
        };
        let (Some(start_us), Some(end_us), Some(text)) = (
            value("start").and_then(|item| parse_hms(item, '.')),
            value("end").and_then(|item| parse_hms(item, '.')),
            value("text"),
        ) else {
            result.warnings.push(format!(
                "Skipped ASS dialogue on line {} with invalid required fields.",
                line_number + 1
            ));
            continue;
        };
        if end_us <= start_us {
            result.warnings.push(format!(
                "Skipped ASS dialogue on line {} with empty timing.",
                line_number + 1
            ));
            continue;
        }
        if text.len() > MAX_CUE_BYTES {
            result.warnings.push(format!(
                "Skipped oversized ASS dialogue on line {}.",
                line_number + 1
            ));
            continue;
        }
        let plain_text = ass_plain_text(text);
        if plain_text.trim().is_empty() {
            continue;
        }
        let actor = value("name")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let alignment = alignment_hint(text);
        if result.cues.len() == MAX_CUES {
            return Err(subtitle_error(
                "Subtitle file exceeds the 100,000 cue safety limit.",
            ));
        }
        result.cues.push(make_cue(
            source_version,
            &format!("line-{line_number}"),
            start_us,
            end_us,
            text.to_owned(),
            plain_text,
            line_number as u32,
            SubtitleStyleHintV1 { alignment, actor },
        ));
    }
    if result.cues.is_empty() {
        return Err(subtitle_error("No valid subtitle cues were found."));
    }
    Ok(result)
}

fn ass_plain_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut drawing_mode = false;
    while let Some(character) = chars.next() {
        if character == '{' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                tag.push(next);
            }
            update_drawing_mode(&tag, &mut drawing_mode);
            continue;
        }
        if drawing_mode {
            continue;
        }
        if character == '\\' {
            match chars.peek().copied() {
                Some('N' | 'n') => {
                    chars.next();
                    output.push('\n');
                }
                Some('h') => {
                    chars.next();
                    output.push(' ');
                }
                Some('{' | '}') => {
                    if let Some(value) = chars.next() {
                        output.push(value);
                    }
                }
                _ => output.push(character),
            }
        } else {
            output.push(character);
        }
    }
    strip_basic_markup(&output)
}

fn update_drawing_mode(tag: &str, drawing_mode: &mut bool) {
    for command in tag.split('\\').skip(1) {
        let Some(value) = command
            .strip_prefix('p')
            .or_else(|| command.strip_prefix('P'))
        else {
            continue;
        };
        let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(scale) = digits.parse::<u8>() {
            *drawing_mode = scale != 0;
        }
    }
}

fn alignment_hint(input: &str) -> Option<u8> {
    let mut rest = input;
    let mut alignment = None;
    while let Some(start) = rest.find('{') {
        let candidate = &rest[(start + 1)..];
        let Some(end) = candidate.find('}') else {
            break;
        };
        for command in candidate[..end].split('\\').skip(1) {
            let Some(value) = command
                .strip_prefix("an")
                .or_else(|| command.strip_prefix("AN"))
            else {
                continue;
            };
            alignment = value
                .chars()
                .next()
                .and_then(|value| value.to_digit(10))
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| (1..=9).contains(value));
        }
        rest = &candidate[(end + 1)..];
    }
    alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dynamic_fields_commas_tags_and_line_breaks() -> Result<(), contracts::AppErrorV1> {
        let input = "[Events]\nFormat: Start, End, Name, Text\nDialogue: 0:00:01.00,0:00:02.50,Aoi,{\\an8}<b>見</b>る, 本当？\\Nはい";
        let parsed = parse("source", input)?;
        assert_eq!(parsed.cues[0].plain_text, "見る, 本当？\nはい");
        assert_eq!(parsed.cues[0].style_hint.alignment, Some(8));
        assert_eq!(parsed.cues[0].style_hint.actor.as_deref(), Some("Aoi"));
        Ok(())
    }

    #[test]
    fn drawing_commands_are_applied_in_order_and_case_is_accepted()
    -> Result<(), contracts::AppErrorV1> {
        let input = "[Events]\nFORMAT: Start, End, Text\nDIALOGUE: 0:00:01.00,0:00:02.50,{\\p1}m 0 0 l 1 1{\\p0}本文";
        let parsed = parse("source", input)?;
        assert_eq!(parsed.cues[0].plain_text, "本文");
        Ok(())
    }

    #[test]
    fn last_override_alignment_wins_and_plain_text_is_ignored() {
        assert_eq!(alignment_hint("{\\an7}上{\\an2}下"), Some(2));
        assert_eq!(alignment_hint("literal \\an8"), None);
    }
}
