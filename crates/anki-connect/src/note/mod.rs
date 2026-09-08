use std::collections::{BTreeMap, HashSet};

use contracts::{AppErrorV1, CardDraftV1, DictionaryEntrySummaryV1, error_codes};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnkiProfile {
    pub profile_id: String,
    pub deck_name: String,
    pub model_name: String,
    pub field_mapping: BTreeMap<String, String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnkiNote {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: String,
    pub fields: BTreeMap<String, String>,
    pub tags: Vec<String>,
    pub options: NoteOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NoteOptions {
    #[serde(rename = "allowDuplicate")]
    pub allow_duplicate: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoteMedia {
    pub audio_media_name: Option<String>,
    pub image_media_name: Option<String>,
}

pub fn build_note(
    profile: &AnkiProfile,
    draft: &CardDraftV1,
    editable_fields: &BTreeMap<String, String>,
    mining_id: &str,
) -> Result<AnkiNote, AppErrorV1> {
    build_note_with_media(
        profile,
        draft,
        editable_fields,
        mining_id,
        &NoteMedia::default(),
    )
}

pub fn build_note_with_media(
    profile: &AnkiProfile,
    draft: &CardDraftV1,
    editable_fields: &BTreeMap<String, String>,
    mining_id: &str,
    media: &NoteMedia,
) -> Result<AnkiNote, AppErrorV1> {
    validate_profile_shape(profile)?;
    validate_mining_id(mining_id)?;
    if editable_fields.len() > 32
        || editable_fields
            .values()
            .any(|value| value.len() > 1024 * 1024)
    {
        return Err(mapping_error(
            "Editable card fields exceeded their safety limit.",
        ));
    }
    if editable_fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "expression" | "reading" | "sentence" | "definition"
        )
    }) {
        return Err(editable_field_error(
            "Only expression, reading, sentence, and definition are user-editable.",
        ));
    }
    if profile.deck_name.trim().is_empty() || profile.model_name.trim().is_empty() {
        return Err(mapping_error("Deck and model are required."));
    }
    let mut logical = BTreeMap::new();
    logical.insert("expression", draft.token.surface.clone());
    logical.insert("reading", draft.token.reading.clone());
    logical.insert("sentence", draft.cue.plain_text.clone());
    logical.insert(
        "definition",
        draft
            .dictionary_entry
            .as_ref()
            .map(format_dictionary_definition)
            .unwrap_or_default(),
    );
    logical.insert(
        "source",
        draft.source_fingerprint.chars().take(12).collect(),
    );
    logical.insert("timestamp", format_timestamp(draft.observed_source_time_us));
    logical.insert("mining_id", mining_id.to_owned());
    logical.insert(
        "audio",
        generated_audio_markup(media.audio_media_name.as_deref())?,
    );
    logical.insert(
        "image",
        generated_image_markup(media.image_media_name.as_deref())?,
    );
    for (key, value) in editable_fields {
        if logical.contains_key(key.as_str()) {
            logical.insert(key, value.clone());
        }
    }
    let mut fields = BTreeMap::new();
    for (logical_name, model_field) in &profile.field_mapping {
        let value = logical
            .get(logical_name.as_str())
            .ok_or_else(|| mapping_error(&format!("Unknown logical field {logical_name}.")))?;
        if model_field.trim().is_empty() {
            return Err(mapping_error("Mapped model field cannot be empty."));
        }
        let rendered = if matches!(logical_name.as_str(), "audio" | "image") {
            value.clone()
        } else {
            escape_html(value)
        };
        fields.insert(model_field.clone(), rendered);
    }
    let mut tags = profile
        .tags
        .iter()
        .map(|tag| sanitize_tag(tag))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    tags.push(marker_tag(mining_id));
    tags.sort();
    tags.dedup();
    Ok(AnkiNote {
        deck_name: profile.deck_name.clone(),
        model_name: profile.model_name.clone(),
        fields,
        tags,
        options: NoteOptions {
            allow_duplicate: false,
        },
    })
}

fn format_dictionary_definition(entry: &DictionaryEntrySummaryV1) -> String {
    entry
        .senses
        .iter()
        .enumerate()
        .filter_map(|(index, sense)| {
            let glosses = unique_trimmed(sense.glosses.iter()).join("; ");
            if glosses.is_empty() {
                return None;
            }

            let labels = unique_trimmed(
                sense
                    .parts_of_speech
                    .iter()
                    .chain(&sense.fields)
                    .chain(&sense.dialects)
                    .chain(&sense.misc),
            );
            let labels = if labels.is_empty() {
                String::new()
            } else {
                format!(" ({})", labels.join(", "))
            };
            Some(format!("{}.{} {glosses}", index + 1, labels))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unique_trimmed<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .filter_map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_owned()) {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
        .collect()
}

#[must_use]
pub fn marker_tag(mining_id: &str) -> String {
    format!(
        "migaku_id_{}",
        mining_id
            .chars()
            .filter(|character| character.is_ascii_hexdigit())
            .take(64)
            .collect::<String>()
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('\n', "<br>")
}

fn sanitize_tag(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
        .take(128)
        .collect()
}

fn validate_profile_shape(profile: &AnkiProfile) -> Result<(), AppErrorV1> {
    if profile.profile_id.is_empty()
        || profile.profile_id.len() > 128
        || profile.deck_name.len() > 255
        || profile.model_name.len() > 255
        || profile.field_mapping.is_empty()
        || profile.field_mapping.len() > 32
        || profile.tags.len() > 64
        || profile.tags.iter().any(|tag| tag.len() > 256)
    {
        return Err(mapping_error(
            "The Anki profile exceeded its safety limits.",
        ));
    }
    let mut target_fields = HashSet::new();
    for (logical, model) in &profile.field_mapping {
        if logical.is_empty()
            || logical.len() > 64
            || model.trim().is_empty()
            || model.len() > 255
            || !target_fields.insert(model)
        {
            return Err(mapping_error(
                "Logical fields must map one-to-one to non-empty model fields.",
            ));
        }
    }
    Ok(())
}

fn validate_mining_id(mining_id: &str) -> Result<(), AppErrorV1> {
    if mining_id.len() != 64
        || !mining_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(mapping_error("The stable mining identifier was invalid."));
    }
    Ok(())
}

fn generated_audio_markup(media_name: Option<&str>) -> Result<String, AppErrorV1> {
    media_name
        .map(|name| {
            validate_media_name(name, &["mp3", "ogg"])?;
            Ok(format!("[sound:{name}]"))
        })
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn generated_image_markup(media_name: Option<&str>) -> Result<String, AppErrorV1> {
    media_name
        .map(|name| {
            validate_media_name(name, &["jpg", "png"])?;
            Ok(format!("<img src=\"{name}\">"))
        })
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn validate_media_name(media_name: &str, allowed_extensions: &[&str]) -> Result<(), AppErrorV1> {
    let valid = media_name
        .strip_prefix("migaku-")
        .and_then(|remainder| remainder.rsplit_once('.'))
        .is_some_and(|(hash, extension)| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && allowed_extensions.contains(&extension)
        });
    if !valid {
        return Err(mapping_error(
            "An application-generated media name was invalid.",
        ));
    }
    Ok(())
}

fn format_timestamp(timestamp_us: i64) -> String {
    let total_seconds = timestamp_us.max(0) / 1_000_000;
    format!(
        "{:02}:{:02}:{:02}",
        total_seconds / 3_600,
        (total_seconds / 60) % 60,
        total_seconds % 60
    )
}

fn mapping_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::ANKI_SCHEMA_MISMATCH,
        "The Anki field mapping is incomplete or no longer matches the note type.",
        true,
    )
    .with_diagnostics(detail)
}

fn editable_field_error(detail: &str) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::INVALID_REQUEST,
        "The card contains an unknown or protected editable field.",
        false,
    )
    .with_diagnostics(detail)
}

#[cfg(test)]
mod tests {
    use contracts::{
        CueId, DictionaryEntryId, DictionaryEntrySummaryV1, DictionarySenseV1, DraftId,
        MediaSessionId, SubtitleCueV1, SubtitleSourceId, SubtitleStyleHintV1, TokenId, TokenV1,
    };

    use super::*;

    fn draft() -> CardDraftV1 {
        CardDraftV1 {
            draft_id: DraftId::new("draft"),
            revision: 1,
            session_id: MediaSessionId::new("session"),
            subtitle_source_id: SubtitleSourceId::new("subtitle"),
            subtitle_source_version: "version".into(),
            cue: SubtitleCueV1 {
                cue_id: CueId::new("cue"),
                start_us: 1_000_000,
                end_us: 2_000_000,
                plain_text: "<b>見る & 学ぶ</b>".into(),
                source_text: "<b>見る & 学ぶ</b>".into(),
                track_order: 0,
                style_hint: SubtitleStyleHintV1::default(),
            },
            token: TokenV1 {
                token_id: TokenId::new("token"),
                surface: "<見る>".into(),
                byte_start: 0,
                byte_end: 3,
                lemma: "見る".into(),
                reading: "みる".into(),
                pronunciation: None,
                part_of_speech: vec!["verb".into()],
                lookup_candidate: true,
            },
            dictionary_entry: None,
            observed_source_time_us: 1_500_000,
            source_fingerprint: "source".into(),
        }
    }

    fn profile() -> AnkiProfile {
        AnkiProfile {
            profile_id: "default".into(),
            deck_name: "Mining".into(),
            model_name: "Basic".into(),
            field_mapping: BTreeMap::from([
                ("expression".into(), "Front".into()),
                ("sentence".into(), "Sentence".into()),
                ("image".into(), "Image".into()),
                ("audio".into(), "Audio".into()),
            ]),
            tags: vec!["日本語 deck".into()],
        }
    }

    #[test]
    fn escapes_text_but_preserves_only_valid_generated_media_markup() -> Result<(), AppErrorV1> {
        let media = NoteMedia {
            audio_media_name: Some(format!("migaku-{}.mp3", "a".repeat(64))),
            image_media_name: Some(format!("migaku-{}.jpg", "b".repeat(64))),
        };
        let note = build_note_with_media(
            &profile(),
            &draft(),
            &BTreeMap::new(),
            &"c".repeat(64),
            &media,
        )?;
        assert_eq!(note.fields["Front"], "&lt;見る&gt;");
        assert!(note.fields["Sentence"].contains("&lt;b&gt;"));
        assert!(note.fields["Audio"].starts_with("[sound:migaku-"));
        assert!(note.fields["Image"].starts_with("<img src=\"migaku-"));
        Ok(())
    }

    #[test]
    fn rejects_forged_editable_media_markup() {
        let editable = BTreeMap::from([("audio".into(), "<img src=x onerror=alert(1)>".into())]);
        let result = build_note_with_media(
            &profile(),
            &draft(),
            &editable,
            &"c".repeat(64),
            &NoteMedia::default(),
        );
        assert!(matches!(
            result,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::INVALID_REQUEST
        ));
    }

    #[test]
    fn rejects_duplicate_targets_and_unsafe_media_names() {
        let mut invalid_profile = profile();
        invalid_profile
            .field_mapping
            .insert("reading".into(), "Front".into());
        assert!(
            build_note(
                &invalid_profile,
                &draft(),
                &BTreeMap::new(),
                &"c".repeat(64)
            )
            .is_err()
        );

        let media = NoteMedia {
            audio_media_name: Some("../escape.mp3".into()),
            image_media_name: None,
        };
        assert!(
            build_note_with_media(
                &profile(),
                &draft(),
                &BTreeMap::new(),
                &"c".repeat(64),
                &media
            )
            .is_err()
        );
    }

    #[test]
    fn default_anki_definition_preserves_all_ranked_senses() -> Result<(), AppErrorV1> {
        let mut card = draft();
        card.dictionary_entry = Some(DictionaryEntrySummaryV1 {
            entry_id: DictionaryEntryId::new("entry"),
            headwords: vec!["見る".into()],
            readings: vec!["みる".into()],
            senses: vec![
                DictionarySenseV1 {
                    glosses: vec!["to see".into(), "to look".into()],
                    parts_of_speech: vec!["Ichidan verb".into()],
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: vec!["common".into()],
                },
                DictionarySenseV1 {
                    glosses: vec!["to examine".into()],
                    parts_of_speech: vec!["transitive verb".into()],
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: Vec::new(),
                },
            ],
            pitch_accents: Vec::new(),
            jlpt_level: None,
            jlpt_source: None,
            match_reason: "exact lemma".into(),
            priority_score: 100,
        });
        let mut word_profile = profile();
        word_profile
            .field_mapping
            .insert("definition".into(), "Definition".into());

        let note = build_note(&word_profile, &card, &BTreeMap::new(), &"c".repeat(64))?;
        assert_eq!(
            note.fields["Definition"],
            "1. (Ichidan verb, common) to see; to look<br>2. (transitive verb) to examine"
        );
        Ok(())
    }

    #[test]
    fn default_anki_definition_matches_trimmed_composer_content() -> Result<(), AppErrorV1> {
        let mut card = draft();
        card.dictionary_entry = Some(DictionaryEntrySummaryV1 {
            entry_id: DictionaryEntryId::new("entry"),
            headwords: vec!["見る".into()],
            readings: vec!["みる".into()],
            senses: vec![DictionarySenseV1 {
                glosses: vec![" to see ".into(), "to see".into(), " ".into()],
                parts_of_speech: vec![" verb ".into()],
                restrictions: Vec::new(),
                fields: vec!["general".into()],
                dialects: Vec::new(),
                misc: vec!["verb".into(), " ".into()],
            }],
            pitch_accents: Vec::new(),
            jlpt_level: None,
            jlpt_source: None,
            match_reason: "exact lemma".into(),
            priority_score: 100,
        });
        let mut word_profile = profile();
        word_profile
            .field_mapping
            .insert("definition".into(), "Definition".into());

        let note = build_note(&word_profile, &card, &BTreeMap::new(), &"c".repeat(64))?;
        assert_eq!(note.fields["Definition"], "1. (verb, general) to see");
        Ok(())
    }
}
