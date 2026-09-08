use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use contracts::{AppErrorV1, error_codes};
use csv::StringRecord;
use dictionary::metadata;
use dictionary::schema::{self, ImportEntry, ImportReading, ImportSense};
use quick_xml::{events::Event, reader::Reader};
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_XML_TOKEN_BYTES: usize = 1024 * 1024;
const MAX_XML_DEPTH: usize = 64;
const MAX_FORMS_PER_ENTRY: usize = 256;
const MAX_SENSES_PER_ENTRY: usize = 512;
const MAX_VALUES_PER_FIELD: usize = 512;
const MAX_UNIDIC_COLUMNS: usize = 64;
const UNIDIC_SURFACE: usize = 0;
const UNIDIC_LEMMA_READING: usize = 10;
const UNIDIC_LEMMA: usize = 11;
const UNIDIC_ORTH_BASE: usize = 14;
const UNIDIC_KANA: usize = 24;
const UNIDIC_KANA_BASE: usize = 25;
const UNIDIC_ACCENT_TYPE: usize = 28;

pub fn build(
    source_path: &Path,
    expected_sha256: &str,
    source_version: &str,
    output_path: &Path,
) -> Result<usize, AppErrorV1> {
    validate_source_bounds(source_path)?;
    let actual_sha256 = sha256_file(source_path)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(builder_error(
            "JMdict checksum did not match the pinned source.",
        ));
    }
    if output_path.exists() {
        return Err(builder_error(
            "Output already exists; choose a new path for atomic promotion.",
        ));
    }
    let parent = output_path
        .parent()
        .ok_or_else(|| builder_error("Output needs a parent directory."))?;
    fs::create_dir_all(parent).map_err(builder_error)?;
    let temporary = temporary_path(output_path);
    let result = build_temporary(source_path, &actual_sha256, source_version, &temporary);
    match result {
        Ok(count) => {
            fs::rename(&temporary, output_path).map_err(builder_error)?;
            Ok(count)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_metadata(
    pitch_source_path: &Path,
    expected_pitch_sha256: &str,
    pitch_source_version: &str,
    jlpt_directory: &Path,
    expected_jlpt_sha256: &str,
    jlpt_source_version: &str,
    output_path: &Path,
) -> Result<(usize, usize), AppErrorV1> {
    validate_source_bounds(pitch_source_path)?;
    let actual_pitch_sha256 = sha256_file(pitch_source_path)?;
    if !actual_pitch_sha256.eq_ignore_ascii_case(expected_pitch_sha256) {
        return Err(builder_error(
            "UniDic checksum did not match the pinned source.",
        ));
    }
    if !jlpt_directory.is_dir() {
        return Err(builder_error("JLPT bank directory is missing."));
    }
    if expected_jlpt_sha256.len() != 64
        || !expected_jlpt_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err(builder_error("JLPT archive checksum is malformed."));
    }
    if output_path.exists() {
        return Err(builder_error(
            "Output already exists; choose a new path for atomic promotion.",
        ));
    }
    let parent = output_path
        .parent()
        .ok_or_else(|| builder_error("Output needs a parent directory."))?;
    fs::create_dir_all(parent).map_err(builder_error)?;
    let temporary = temporary_path(output_path);
    let result = build_metadata_temporary(
        pitch_source_path,
        &actual_pitch_sha256,
        pitch_source_version,
        jlpt_directory,
        expected_jlpt_sha256,
        jlpt_source_version,
        &temporary,
    );
    match result {
        Ok(counts) => {
            fs::rename(&temporary, output_path).map_err(builder_error)?;
            Ok(counts)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_metadata_temporary(
    pitch_source_path: &Path,
    pitch_source_sha256: &str,
    pitch_source_version: &str,
    jlpt_directory: &Path,
    jlpt_source_sha256: &str,
    jlpt_source_version: &str,
    temporary: &Path,
) -> Result<(usize, usize), AppErrorV1> {
    let mut connection = Connection::open(temporary).map_err(builder_error)?;
    metadata::create(
        &connection,
        pitch_source_sha256,
        pitch_source_version,
        jlpt_source_sha256,
        jlpt_source_version,
    )?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -131072;",
        )
        .map_err(builder_error)?;
    let transaction = connection.transaction().map_err(builder_error)?;
    import_unidic_pitch(&transaction, pitch_source_path)?;
    import_jlpt_estimates(&transaction, jlpt_directory)?;
    transaction.commit().map_err(builder_error)?;
    let counts = metadata::validate(&connection)?;
    connection
        .execute_batch("PRAGMA optimize;")
        .map_err(builder_error)?;
    Ok(counts)
}

fn import_unidic_pitch(connection: &Connection, source_path: &Path) -> Result<(), AppErrorV1> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(source_path)
        .map_err(builder_error)?;
    for record in reader.records() {
        let record = record.map_err(builder_error)?;
        if record.len() <= UNIDIC_ACCENT_TYPE || record.len() > MAX_UNIDIC_COLUMNS {
            return Err(builder_error(format!(
                "Unexpected UniDic column count {}.",
                record.len()
            )));
        }
        for accent_type in unidic_accent_types(&record) {
            for (spelling_index, reading_index) in [
                (UNIDIC_SURFACE, UNIDIC_KANA),
                (UNIDIC_ORTH_BASE, UNIDIC_KANA_BASE),
                (UNIDIC_LEMMA, UNIDIC_LEMMA_READING),
            ] {
                if let (Some(spelling), Some(reading)) =
                    (record.get(spelling_index), record.get(reading_index))
                {
                    metadata::insert_pitch(connection, spelling, reading, accent_type)?;
                }
            }
        }
    }
    Ok(())
}

fn unidic_accent_types(record: &StringRecord) -> Vec<u32> {
    record
        .get(UNIDIC_ACCENT_TYPE)
        .unwrap_or_default()
        .split(',')
        .filter_map(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value <= 64)
        .collect()
}

#[derive(Debug, Deserialize)]
struct JlptMeta {
    reading: String,
    frequency: JlptFrequency,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JlptFrequency {
    display_value: String,
}

fn import_jlpt_estimates(connection: &Connection, jlpt_directory: &Path) -> Result<(), AppErrorV1> {
    for bank in 1..=5 {
        let path = jlpt_directory.join(format!("term_meta_bank_{bank}.json"));
        validate_source_bounds(&path)?;
        let entries: Vec<(String, String, JlptMeta)> =
            serde_json::from_reader(BufReader::new(File::open(&path).map_err(builder_error)?))
                .map_err(builder_error)?;
        for (spelling, mode, value) in entries {
            if mode != "freq" {
                continue;
            }
            let Some(level) = value
                .frequency
                .display_value
                .strip_prefix('N')
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|level| (1..=5).contains(level))
            else {
                continue;
            };
            metadata::insert_jlpt(connection, &spelling, &value.reading, level)?;
        }
    }
    Ok(())
}

fn build_temporary(
    source_path: &Path,
    source_sha256: &str,
    source_version: &str,
    temporary: &Path,
) -> Result<usize, AppErrorV1> {
    if source_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return build_simplified_json_temporary(
            source_path,
            source_sha256,
            source_version,
            temporary,
        );
    }
    let mut connection = Connection::open(temporary).map_err(builder_error)?;
    schema::create(&connection, source_sha256, source_version)?;
    let transaction = connection.transaction().map_err(builder_error)?;
    let file = File::open(source_path).map_err(builder_error)?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut stack = Vec::<String>::new();
    let mut current: Option<EntryBuilder> = None;
    let mut excluded_gloss_depth = None;
    let mut count = 0_usize;
    loop {
        match reader.read_event_into(&mut buffer).map_err(builder_error)? {
            Event::Start(start) => {
                if stack.len() == MAX_XML_DEPTH {
                    return Err(builder_error(
                        "JMdict XML nesting exceeds the safety limit.",
                    ));
                }
                let name = start.name().as_ref().to_owned();
                if name == "entry" {
                    if current.is_some() {
                        return Err(builder_error("JMdict entries cannot be nested."));
                    }
                    current = Some(EntryBuilder::default());
                } else if let Some(entry) = current.as_mut() {
                    entry.start_element(&name)?;
                }
                if name == "gloss" && !is_english_gloss(&start)? {
                    excluded_gloss_depth = Some(stack.len() + 1);
                }
                stack.push(name);
            }
            Event::Text(text) => {
                if excluded_gloss_depth.is_none() {
                    let value = quick_xml::escape::unescape(text.as_ref())
                        .map_err(builder_error)?
                        .into_owned();
                    if let Some(entry) = current.as_mut() {
                        entry.text(&stack, value)?;
                    }
                }
            }
            Event::CData(text) => {
                if excluded_gloss_depth.is_none()
                    && let Some(entry) = current.as_mut()
                {
                    entry.text(&stack, text.as_ref().to_owned())?;
                }
            }
            Event::GeneralRef(reference) => {
                if excluded_gloss_depth.is_none()
                    && let Some(entry) = current.as_mut()
                {
                    entry.text(&stack, reference.as_ref().to_owned())?;
                }
            }
            Event::End(end) => {
                let name = end.name().as_ref().to_owned();
                if stack.last() != Some(&name) {
                    return Err(builder_error(format!(
                        "Malformed JMdict element nesting at closing {name}."
                    )));
                }
                if name == "entry" {
                    let entry = current
                        .take()
                        .ok_or_else(|| builder_error("Malformed JMdict entry nesting."))?
                        .finish()?;
                    schema::insert(&transaction, &entry)?;
                    count = count.saturating_add(1);
                } else if let Some(entry) = current.as_mut() {
                    entry.end_element(&name)?;
                }
                if excluded_gloss_depth == Some(stack.len()) {
                    excluded_gloss_depth = None;
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() || current.is_some() {
        return Err(builder_error("JMdict ended with unclosed elements."));
    }
    transaction.commit().map_err(builder_error)?;
    let validated = schema::validate(&connection)?;
    if count == 0 || validated != count {
        return Err(builder_error(
            "JMdict import produced no entries or failed count validation.",
        ));
    }
    connection
        .execute_batch("PRAGMA optimize;")
        .map_err(builder_error)?;
    Ok(count)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimplifiedJmdict {
    version: String,
    languages: Vec<String>,
    dict_date: String,
    tags: BTreeMap<String, String>,
    words: Vec<SimplifiedWord>,
}

#[derive(Debug, Deserialize)]
struct SimplifiedWord {
    id: String,
    kanji: Vec<SimplifiedForm>,
    kana: Vec<SimplifiedReading>,
    sense: Vec<SimplifiedSense>,
}

#[derive(Debug, Deserialize)]
struct SimplifiedForm {
    common: bool,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimplifiedReading {
    common: bool,
    text: String,
    applies_to_kanji: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimplifiedSense {
    part_of_speech: Vec<String>,
    applies_to_kanji: Vec<String>,
    applies_to_kana: Vec<String>,
    field: Vec<String>,
    dialect: Vec<String>,
    misc: Vec<String>,
    info: Vec<String>,
    gloss: Vec<SimplifiedGloss>,
}

#[derive(Debug, Deserialize)]
struct SimplifiedGloss {
    text: String,
}

fn build_simplified_json_temporary(
    source_path: &Path,
    source_sha256: &str,
    source_version: &str,
    temporary: &Path,
) -> Result<usize, AppErrorV1> {
    let source: SimplifiedJmdict = serde_json::from_reader(BufReader::new(
        File::open(source_path).map_err(builder_error)?,
    ))
    .map_err(builder_error)?;
    if source.version.is_empty()
        || !source_version.starts_with(&source.version)
        || source.dict_date.len() != 10
        || !source.languages.iter().any(|language| language == "eng")
        || source.words.is_empty()
    {
        return Err(builder_error(
            "JMdict-Simplified metadata did not match the pinned English source version.",
        ));
    }

    let mut connection = Connection::open(temporary).map_err(builder_error)?;
    schema::create(&connection, source_sha256, source_version)?;
    let transaction = connection.transaction().map_err(builder_error)?;
    let mut count = 0_usize;
    for word in source.words {
        let entry = simplified_entry(word, &source.tags)?;
        schema::insert(&transaction, &entry)?;
        count = count.saturating_add(1);
    }
    transaction.commit().map_err(builder_error)?;
    let validated = schema::validate(&connection)?;
    if count == 0 || validated != count {
        return Err(builder_error(
            "JMdict-Simplified import produced no entries or failed count validation.",
        ));
    }
    connection
        .execute_batch("PRAGMA optimize;")
        .map_err(builder_error)?;
    Ok(count)
}

fn simplified_entry(
    word: SimplifiedWord,
    tags: &BTreeMap<String, String>,
) -> Result<ImportEntry, AppErrorV1> {
    let sequence = word
        .id
        .parse::<i64>()
        .map_err(|_| builder_error("JMdict-Simplified entry id was not an integer."))?;
    let writings = word
        .kanji
        .into_iter()
        .map(|form| (form.text, if form.common { 100 } else { 0 }))
        .collect();
    let readings = word
        .kana
        .into_iter()
        .map(|reading| ImportReading {
            text: reading.text,
            priority: if reading.common { 100 } else { 0 },
            restrictions: concrete_restrictions(&reading.applies_to_kanji),
        })
        .collect();
    let senses = word
        .sense
        .into_iter()
        .filter_map(|sense| {
            let glosses = sense
                .gloss
                .into_iter()
                .map(|gloss| gloss.text)
                .filter(|gloss| !gloss.is_empty())
                .collect::<Vec<_>>();
            if glosses.is_empty() {
                return None;
            }
            let mut restrictions = BTreeSet::new();
            restrictions.extend(concrete_restrictions(&sense.applies_to_kanji));
            restrictions.extend(concrete_restrictions(&sense.applies_to_kana));
            let mut misc = expand_tags(sense.misc, tags);
            misc.extend(sense.info.into_iter().filter(|value| !value.is_empty()));
            Some(ImportSense {
                glosses,
                parts_of_speech: expand_tags(sense.part_of_speech, tags),
                restrictions: restrictions.into_iter().collect(),
                fields: expand_tags(sense.field, tags),
                dialects: expand_tags(sense.dialect, tags),
                misc,
            })
        })
        .collect();
    Ok(ImportEntry {
        sequence,
        writings,
        readings,
        senses,
    })
}

fn concrete_restrictions(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !value.is_empty() && value.as_str() != "*")
        .cloned()
        .collect()
}

fn expand_tags(values: Vec<String>, tags: &BTreeMap<String, String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| tags.get(&value).cloned().unwrap_or(value))
        .collect()
}

#[derive(Default)]
struct EntryBuilder {
    sequence: Option<i64>,
    writings: Vec<(String, i32)>,
    readings: Vec<ImportReading>,
    senses: Vec<ImportSense>,
    writing_element_start: Option<usize>,
    reading_element_start: Option<usize>,
}

impl EntryBuilder {
    fn start_element(&mut self, name: &str) -> Result<(), AppErrorV1> {
        match name {
            "k_ele" if self.writing_element_start.is_none() => {
                if self.writings.len() == MAX_FORMS_PER_ENTRY {
                    return Err(builder_error("JMdict entry has too many written forms."));
                }
                self.writing_element_start = Some(self.writings.len());
            }
            "r_ele" if self.reading_element_start.is_none() => {
                if self.readings.len() == MAX_FORMS_PER_ENTRY {
                    return Err(builder_error("JMdict entry has too many readings."));
                }
                self.reading_element_start = Some(self.readings.len());
            }
            "sense" => {
                if self.senses.len() == MAX_SENSES_PER_ENTRY {
                    return Err(builder_error("JMdict entry has too many senses."));
                }
                self.senses.push(ImportSense {
                    glosses: Vec::new(),
                    parts_of_speech: Vec::new(),
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: Vec::new(),
                });
            }
            "k_ele" | "r_ele" => {
                return Err(builder_error(format!(
                    "JMdict contains nested {name} elements."
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn end_element(&mut self, name: &str) -> Result<(), AppErrorV1> {
        match name {
            "k_ele" => {
                let start = self
                    .writing_element_start
                    .take()
                    .ok_or_else(|| builder_error("Closed k_ele without opening it."))?;
                if self.writings.len() != start + 1 {
                    return Err(builder_error("Every k_ele must contain exactly one keb."));
                }
            }
            "r_ele" => {
                let start = self
                    .reading_element_start
                    .take()
                    .ok_or_else(|| builder_error("Closed r_ele without opening it."))?;
                if self.readings.len() != start + 1 {
                    return Err(builder_error("Every r_ele must contain exactly one reb."));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn text(&mut self, stack: &[String], value: String) -> Result<(), AppErrorV1> {
        if value.len() > MAX_XML_TOKEN_BYTES {
            return Err(builder_error("JMdict field exceeds the safety limit."));
        }
        let tag = stack.last().map(String::as_str).unwrap_or_default();
        let parent = stack
            .iter()
            .rev()
            .nth(1)
            .map(String::as_str)
            .unwrap_or_default();
        match (parent, tag) {
            ("entry", "ent_seq") => {
                if self.sequence.is_some() {
                    return Err(builder_error("JMdict entry repeated ent_seq."));
                }
                self.sequence = Some(
                    value
                        .parse()
                        .map_err(|_| builder_error("JMdict ent_seq is not an integer."))?,
                );
            }
            ("k_ele", "keb") => {
                let start = self
                    .writing_element_start
                    .ok_or_else(|| builder_error("keb appeared outside k_ele."))?;
                if self.writings.len() != start {
                    return Err(builder_error("k_ele repeated keb."));
                }
                self.writings.push((value, 0));
            }
            ("r_ele", "reb") => {
                let start = self
                    .reading_element_start
                    .ok_or_else(|| builder_error("reb appeared outside r_ele."))?;
                if self.readings.len() != start {
                    return Err(builder_error("r_ele repeated reb."));
                }
                self.readings.push(ImportReading {
                    text: value,
                    priority: 0,
                    restrictions: Vec::new(),
                });
            }
            ("k_ele", "ke_pri") => {
                let start = self
                    .writing_element_start
                    .ok_or_else(|| builder_error("ke_pri appeared outside k_ele."))?;
                let (_, priority) = self
                    .writings
                    .get_mut(start)
                    .ok_or_else(|| builder_error("ke_pri appeared before keb."))?;
                *priority = (*priority).max(priority_score(&value));
            }
            ("r_ele", "re_pri") => {
                let start = self
                    .reading_element_start
                    .ok_or_else(|| builder_error("re_pri appeared outside r_ele."))?;
                let reading = self
                    .readings
                    .get_mut(start)
                    .ok_or_else(|| builder_error("re_pri appeared before reb."))?;
                reading.priority = reading.priority.max(priority_score(&value));
            }
            ("r_ele", "re_restr") => {
                let start = self
                    .reading_element_start
                    .ok_or_else(|| builder_error("re_restr appeared outside r_ele."))?;
                let reading = self
                    .readings
                    .get_mut(start)
                    .ok_or_else(|| builder_error("re_restr appeared before reb."))?;
                push_bounded(&mut reading.restrictions, value, "reading restrictions")?;
            }
            ("sense", "gloss") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.glosses, value, "sense glosses")?;
                }
            }
            ("sense", "pos") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.parts_of_speech, value, "parts of speech")?;
                }
            }
            ("sense", "stagk" | "stagr") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.restrictions, value, "sense restrictions")?;
                }
            }
            ("sense", "field") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.fields, value, "sense fields")?;
                }
            }
            ("sense", "dial") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.dialects, value, "sense dialects")?;
                }
            }
            ("sense", "misc") => {
                if let Some(sense) = self.senses.last_mut() {
                    push_bounded(&mut sense.misc, value, "sense miscellaneous tags")?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(mut self) -> Result<ImportEntry, AppErrorV1> {
        if self.writing_element_start.is_some() || self.reading_element_start.is_some() {
            return Err(builder_error("JMdict entry ended inside a form element."));
        }
        let sequence = self
            .sequence
            .ok_or_else(|| builder_error("JMdict entry omitted ent_seq."))?;
        if sequence <= 0 {
            return Err(builder_error("JMdict ent_seq must be positive."));
        }
        if self.writings.is_empty() && self.readings.is_empty() {
            return Err(builder_error("JMdict entry omitted all forms."));
        }
        self.senses.retain(|sense| !sense.glosses.is_empty());
        if self.senses.is_empty() {
            return Err(builder_error("JMdict entry omitted all English glosses."));
        }
        Ok(ImportEntry {
            sequence,
            writings: self.writings,
            readings: self.readings,
            senses: self.senses,
        })
    }
}

fn push_bounded(
    values: &mut Vec<String>,
    value: String,
    description: &str,
) -> Result<(), AppErrorV1> {
    if values.len() == MAX_VALUES_PER_FIELD {
        return Err(builder_error(format!(
            "JMdict entry has too many {description}."
        )));
    }
    values.push(value);
    Ok(())
}

fn is_english_gloss(start: &quick_xml::events::BytesStart<'_>) -> Result<bool, AppErrorV1> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(builder_error)?;
        if matches!(attribute.key.as_ref(), "xml:lang" | "lang") {
            return Ok(matches!(attribute.value.as_ref(), "eng" | "en" | "en-US"));
        }
    }
    Ok(true)
}

fn priority_score(value: &str) -> i32 {
    match value {
        "news1" | "ichi1" | "spec1" | "gai1" => 100,
        "news2" | "ichi2" | "spec2" | "gai2" => 60,
        value if value.starts_with("nf") => value
            .get(2..)
            .and_then(|number| number.parse::<i32>().ok())
            .map_or(10, |number| (50 - number).max(1)),
        _ => 10,
    }
}

fn validate_source_bounds(path: &Path) -> Result<(), AppErrorV1> {
    let metadata = path.metadata().map_err(builder_error)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
        return Err(builder_error(
            "JMdict source must be a non-empty file no larger than 1 GiB.",
        ));
    }
    // The delimiter-span guard below prevents oversized XML text/entity tokens. JSON and
    // CSV are bounded by the file-size limit above and validated by their parsers instead.
    if path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("json") || extension.eq_ignore_ascii_case("csv")
    }) {
        return Ok(());
    }
    let mut file = File::open(path).map_err(builder_error)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut span = 0_usize;
    loop {
        let read = file.read(&mut buffer).map_err(builder_error)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if matches!(*byte, b'<' | b'>') {
                span = 0;
            } else {
                span = span.saturating_add(1);
                if span > MAX_XML_TOKEN_BYTES {
                    return Err(builder_error("JMdict XML token exceeds the safety limit."));
                }
            }
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, AppErrorV1> {
    let mut file = File::open(path).map_err(builder_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(builder_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn temporary_path(output: &Path) -> PathBuf {
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    loop {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = output.with_file_name(format!(
            ".{name}.building-{}-{sequence}",
            std::process::id()
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
}

fn builder_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::DICTIONARY_UNAVAILABLE,
        "The dictionary import failed. The previous dictionary was left unchanged.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use contracts::{TokenId, TokenV1};
    use ports::DictionaryPort;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn builds_and_queries_compact_jmdict() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("JMdict.xml");
        let mut file = File::create(&source)?;
        file.write_all(r#"<JMdict><entry><ent_seq>1</ent_seq><k_ele><keb>見る</keb><ke_pri>ichi1</ke_pri></k_ele><r_ele><reb>みる</reb></r_ele><sense><pos>verb</pos><gloss>to see</gloss></sense></entry></JMdict>"#.as_bytes())?;
        drop(file);
        let checksum = sha256_file(&source)?;
        let output = directory.path().join("dictionary.sqlite");
        assert_eq!(build(&source, &checksum, "test", &output)?, 1);
        let dictionary = dictionary::SqliteDictionary::open(&output)?;
        assert_eq!(dictionary.version(), "jmdict-schema-3:test");
        Ok(())
    }

    #[test]
    fn preserves_reading_restrictions_and_only_english_glosses()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("JMdict.xml");
        fs::write(
            &source,
            r#"<JMdict><entry><ent_seq>2</ent_seq><k_ele><keb>見る</keb></k_ele><k_ele><keb>観る</keb></k_ele><r_ele><reb>みる</reb><re_restr>見る</re_restr></r_ele><sense><pos>&v1;</pos><field>&comp;</field><dial>&ksb;</dial><misc>&arch;</misc><gloss>to see</gloss><gloss xml:lang="dut">zien</gloss></sense></entry></JMdict>"#,
        )?;
        let checksum = sha256_file(&source)?;
        let output = directory.path().join("dictionary.sqlite");
        build(&source, &checksum, "test", &output)?;
        let connection = Connection::open(&output)?;
        let restrictions: String = connection.query_row(
            "SELECT restrictions_json FROM forms WHERE text = 'みる'",
            [],
            |row| row.get(0),
        )?;
        let glosses: String = connection.query_row(
            "SELECT glosses_json FROM senses WHERE entry_sequence = 2",
            [],
            |row| row.get(0),
        )?;
        let parts_of_speech: String = connection.query_row(
            "SELECT pos_json FROM senses WHERE entry_sequence = 2",
            [],
            |row| row.get(0),
        )?;
        let (fields, dialects, misc): (String, String, String) = connection.query_row(
            "SELECT fields_json, dialects_json, misc_json FROM senses WHERE entry_sequence = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(restrictions, r#"["見る"]"#);
        assert_eq!(glosses, r#"["to see"]"#);
        assert_eq!(parts_of_speech, r#"["v1"]"#);
        assert_eq!(fields, r#"["comp"]"#);
        assert_eq!(dialects, r#"["ksb"]"#);
        assert_eq!(misc, r#"["arch"]"#);
        Ok(())
    }

    #[test]
    fn malformed_form_does_not_promote_partial_output() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("JMdict.xml");
        fs::write(
            &source,
            r#"<JMdict><entry><ent_seq>3</ent_seq><r_ele><re_pri>ichi1</re_pri><reb>みる</reb></r_ele><sense><gloss>to see</gloss></sense></entry></JMdict>"#,
        )?;
        let checksum = sha256_file(&source)?;
        let output = directory.path().join("dictionary.sqlite");
        assert!(build(&source, &checksum, "test", &output).is_err());
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    fn identical_inputs_produce_identical_databases() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("JMdict.xml");
        fs::write(
            &source,
            r#"<JMdict><entry><ent_seq>4</ent_seq><r_ele><reb>ことば</reb></r_ele><sense><gloss>word</gloss></sense></entry></JMdict>"#,
        )?;
        let checksum = sha256_file(&source)?;
        let first = directory.path().join("first.sqlite");
        let second = directory.path().join("second.sqlite");
        build(&source, &checksum, "test", &first)?;
        build(&source, &checksum, "test", &second)?;
        assert_eq!(sha256_file(&first)?, sha256_file(&second)?);
        Ok(())
    }

    #[test]
    fn builds_pinned_pitch_and_jlpt_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let pitch = directory.path().join("lex.csv");
        let mut row = vec!["*"; 33];
        row[UNIDIC_SURFACE] = "心";
        row[UNIDIC_LEMMA_READING] = "ココロ";
        row[UNIDIC_LEMMA] = "心";
        row[UNIDIC_ORTH_BASE] = "心";
        row[UNIDIC_KANA] = "ココロ";
        row[UNIDIC_KANA_BASE] = "ココロ";
        row[UNIDIC_ACCENT_TYPE] = "2";
        row[31] = "1";
        row[32] = "1";
        fs::write(&pitch, format!("{}\n", row.join(",")))?;
        let pitch_checksum = sha256_file(&pitch)?;

        let jlpt = directory.path().join("jlpt");
        fs::create_dir(&jlpt)?;
        fs::write(
            jlpt.join("term_meta_bank_1.json"),
            r#"[["心","freq",{"reading":"こころ","frequency":{"value":-1,"displayValue":"N3"}}]]"#,
        )?;
        for bank in 2..=5 {
            fs::write(jlpt.join(format!("term_meta_bank_{bank}.json")), "[]")?;
        }
        let source_checksum = "b".repeat(64);
        let output = directory.path().join("lexical-metadata.sqlite");
        assert_eq!(
            build_metadata(
                &pitch,
                &pitch_checksum,
                "UniDic test",
                &jlpt,
                &source_checksum,
                "JLPT estimate test",
                &output,
            )?,
            (1, 1)
        );
        let connection = Connection::open(&output)?;
        assert_eq!(metadata::validate(&connection)?, (1, 1));
        drop(connection);
        assert!(dictionary::LexicalMetadata::open(&output).is_ok());
        Ok(())
    }

    #[test]
    fn checked_in_fixture_is_a_valid_golden_source() -> Result<(), Box<dyn std::error::Error>> {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/dictionary/testdata/sample-jmdict.xml");
        let checksum = sha256_file(&source)?;
        let directory = tempdir()?;
        let output = directory.path().join("fixture.sqlite");
        assert_eq!(build(&source, &checksum, "fixture-v1", &output)?, 2);
        let connection = Connection::open(&output)?;
        assert_eq!(schema::validate(&connection)?, 2);
        drop(connection);

        let dictionary = dictionary::SqliteDictionary::open(&output)?;
        let entries = dictionary.lookup(&TokenV1 {
            token_id: TokenId::new("golden-token"),
            surface: "見".into(),
            byte_start: 0,
            byte_end: 3,
            lemma: "見る".into(),
            reading: "ミ".into(),
            pronunciation: None,
            part_of_speech: vec!["verb".into()],
            lookup_candidate: true,
        })?;
        let sense = entries
            .first()
            .and_then(|entry| entry.senses.first())
            .ok_or("golden dictionary lookup omitted its expected sense")?;
        assert_eq!(sense.glosses, ["to see", "to watch"]);
        assert_eq!(sense.parts_of_speech, ["verb"]);
        assert_eq!(sense.restrictions, ["見る"]);
        assert_eq!(sense.fields, ["cinematography"]);
        assert_eq!(sense.dialects, ["Tokyo-ben"]);
        assert_eq!(sense.misc, ["common usage"]);
        assert!(!sense.glosses.iter().any(|gloss| gloss == "zien"));
        Ok(())
    }

    #[test]
    fn builds_pinned_simplified_json_with_expanded_tags() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempdir()?;
        let source = directory.path().join("jmdict-eng-3.6.2.json");
        fs::write(
            &source,
            r#"{
              "version":"3.6.2",
              "languages":["eng"],
              "dictDate":"2026-08-31",
              "tags":{"v1":"Ichidan verb","comp":"computing","ksb":"Kansai-ben","arch":"archaic"},
              "words":[{
                "id":"5",
                "kanji":[{"common":true,"text":"見る","tags":[]}],
                "kana":[{"common":true,"text":"みる","tags":[],"appliesToKanji":["見る"]}],
                "sense":[{
                  "partOfSpeech":["v1"],
                  "appliesToKanji":["見る"],
                  "appliesToKana":["*"],
                  "related":[],"antonym":[],
                  "field":["comp"],"dialect":["ksb"],"misc":["arch"],
                  "info":["usage note"],"languageSource":[],
                  "gloss":[{"lang":"eng","gender":null,"type":null,"text":"to see"}]
                }]
              }]
            }"#,
        )?;
        let checksum = sha256_file(&source)?;
        let output = directory.path().join("dictionary.sqlite");
        assert_eq!(
            build(&source, &checksum, "3.6.2+20260831182826", &output)?,
            1
        );
        let connection = Connection::open(&output)?;
        let values: (String, String, String, String) = connection.query_row(
            "SELECT pos_json, fields_json, dialects_json, misc_json FROM senses",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        assert_eq!(values.0, r#"["Ichidan verb"]"#);
        assert_eq!(values.1, r#"["computing"]"#);
        assert_eq!(values.2, r#"["Kansai-ben"]"#);
        assert_eq!(values.3, r#"["archaic","usage note"]"#);
        Ok(())
    }

    #[test]
    fn oversized_xml_token_is_rejected_before_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("JMdict.xml");
        let oversized = "x".repeat(MAX_XML_TOKEN_BYTES + 1);
        fs::write(&source, format!("<JMdict>{oversized}</JMdict>"))?;
        assert!(validate_source_bounds(&source).is_err());
        Ok(())
    }
}
