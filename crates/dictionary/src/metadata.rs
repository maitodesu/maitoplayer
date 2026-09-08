use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::Path,
    sync::Arc,
};

use contracts::{
    AppErrorV1, DictionaryEntrySummaryV1, PitchAccentV1, PitchLevelV1, TokenV1, error_codes,
};
use parking_lot::Mutex;
use ports::DictionaryPort;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};

pub const SCHEMA_VERSION: u32 = 1;
const APPLICATION_ID: i64 = 0x4d4c_4d44;
const MAX_CANDIDATES: usize = 16;
const MAX_SOURCE_ROWS: i64 = 64;
const MAX_PATTERNS: usize = 3;
const MAX_ENRICHMENT_CACHE_ENTRIES: usize = 1_024;
const MAX_ENRICHMENT_CACHE_WEIGHT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct LexicalMetadata {
    connection: Mutex<Connection>,
    pitch_source: String,
    jlpt_source: String,
    version: String,
}

impl LexicalMetadata {
    pub fn open(path: &Path) -> Result<Self, AppErrorV1> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(metadata_error)?;
        connection
            .pragma_update(None, "query_only", "ON")
            .map_err(metadata_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(metadata_error)?;
        validate_runtime(&connection)?;
        let pitch_source = metadata_value(&connection, "pitch_source_version")?;
        let jlpt_source = metadata_value(&connection, "jlpt_source_version")?;
        Ok(Self {
            connection: Mutex::new(connection),
            version: format!(
                "lexical-metadata-schema-{SCHEMA_VERSION}:{pitch_source}+{jlpt_source}"
            ),
            pitch_source,
            jlpt_source,
        })
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    fn enrich(&self, token: &TokenV1, entry: &mut DictionaryEntrySummaryV1) {
        let spellings = spelling_candidates(token, entry);
        let readings = reading_candidates(token, entry);
        if spellings.is_empty() || readings.is_empty() {
            return;
        }
        let connection = self.connection.lock();
        if let Ok(patterns) = lookup_pitch(&connection, &spellings, &readings, &self.pitch_source) {
            entry.pitch_accents = patterns;
        }
        if let Ok(Some(level)) = lookup_jlpt(&connection, &spellings, &readings) {
            entry.jlpt_level = Some(level);
            entry.jlpt_source = Some(self.jlpt_source.clone());
        }
    }
}

pub struct EnrichedDictionary {
    base: Arc<dyn DictionaryPort>,
    metadata: Option<LexicalMetadata>,
    cache: Mutex<EnrichmentCache>,
    version: String,
}

impl std::fmt::Debug for EnrichedDictionary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnrichedDictionary")
            .field("version", &self.version)
            .field("metadata_available", &self.metadata.is_some())
            .finish_non_exhaustive()
    }
}

impl EnrichedDictionary {
    #[must_use]
    pub fn new(base: Arc<dyn DictionaryPort>, metadata: Option<LexicalMetadata>) -> Self {
        let version = metadata.as_ref().map_or_else(
            || base.version().to_owned(),
            |metadata| format!("{}+{}", base.version(), metadata.version()),
        );
        Self {
            base,
            metadata,
            cache: Mutex::new(EnrichmentCache::default()),
            version,
        }
    }
}

impl DictionaryPort for EnrichedDictionary {
    fn lookup(&self, token: &TokenV1) -> Result<Vec<DictionaryEntrySummaryV1>, AppErrorV1> {
        let cache_key = EnrichmentCacheKey::from(token);
        if let Some(cached) = self.cache.lock().get(&cache_key) {
            return Ok(cached);
        }
        let mut entries = self.base.lookup(token)?;
        if let Some(metadata) = &self.metadata {
            for entry in entries.iter_mut().take(3) {
                metadata.enrich(token, entry);
            }
        }
        self.cache.lock().insert(cache_key, entries.clone());
        Ok(entries)
    }

    fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EnrichmentCacheKey {
    surface: String,
    lemma: String,
    reading: String,
    pronunciation: Option<String>,
}

impl From<&TokenV1> for EnrichmentCacheKey {
    fn from(token: &TokenV1) -> Self {
        Self {
            surface: token.surface.clone(),
            lemma: token.lemma.clone(),
            reading: token.reading.clone(),
            pronunciation: token.pronunciation.clone(),
        }
    }
}

#[derive(Debug)]
struct CachedEnrichment {
    entries: Vec<DictionaryEntrySummaryV1>,
    weight: usize,
}

#[derive(Debug, Default)]
struct EnrichmentCache {
    entries: HashMap<EnrichmentCacheKey, CachedEnrichment>,
    insertion_order: VecDeque<EnrichmentCacheKey>,
    weight: usize,
}

impl EnrichmentCache {
    fn get(&self, key: &EnrichmentCacheKey) -> Option<Vec<DictionaryEntrySummaryV1>> {
        self.entries.get(key).map(|cached| cached.entries.clone())
    }

    fn insert(&mut self, key: EnrichmentCacheKey, entries: Vec<DictionaryEntrySummaryV1>) {
        if self.entries.contains_key(&key) {
            return;
        }
        let weight = serde_json::to_vec(&entries)
            .map_or(MAX_ENRICHMENT_CACHE_WEIGHT.saturating_add(1), |payload| {
                payload.len().saturating_add(256)
            });
        if weight > MAX_ENRICHMENT_CACHE_WEIGHT {
            return;
        }
        while self.entries.len() >= MAX_ENRICHMENT_CACHE_ENTRIES
            || self.weight.saturating_add(weight) > MAX_ENRICHMENT_CACHE_WEIGHT
        {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.weight = self.weight.saturating_sub(removed.weight);
            }
        }
        self.insertion_order.push_back(key.clone());
        self.entries
            .insert(key, CachedEnrichment { entries, weight });
        self.weight = self.weight.saturating_add(weight);
    }
}

pub fn create(
    connection: &Connection,
    pitch_source_sha256: &str,
    pitch_source_version: &str,
    jlpt_source_sha256: &str,
    jlpt_source_version: &str,
) -> Result<(), AppErrorV1> {
    for checksum in [pitch_source_sha256, jlpt_source_sha256] {
        if checksum.len() != 64 || !checksum.bytes().all(|value| value.is_ascii_hexdigit()) {
            return Err(metadata_error("metadata source checksum is malformed"));
        }
    }
    for version in [pitch_source_version, jlpt_source_version] {
        if version.is_empty() || version.len() > 128 {
            return Err(metadata_error("metadata source version is malformed"));
        }
    }
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA application_id = 1296846148;
             PRAGMA user_version = 1;
             CREATE TABLE metadata (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             ) STRICT;
             CREATE TABLE pitch_patterns (
               spelling TEXT NOT NULL,
               reading TEXT NOT NULL,
               accent_type INTEGER NOT NULL CHECK(accent_type BETWEEN 0 AND 64),
               PRIMARY KEY(spelling, reading, accent_type)
             ) STRICT;
             CREATE INDEX pitch_patterns_spelling_idx ON pitch_patterns(spelling);
             CREATE TABLE jlpt_levels (
               spelling TEXT NOT NULL,
               reading TEXT NOT NULL,
               level INTEGER NOT NULL CHECK(level BETWEEN 1 AND 5),
               PRIMARY KEY(spelling, reading, level)
             ) STRICT;
             CREATE INDEX jlpt_levels_spelling_idx ON jlpt_levels(spelling);",
        )
        .map_err(metadata_error)?;
    for (key, value) in [
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("pitch_source_sha256", pitch_source_sha256.to_ascii_uppercase()),
        ("pitch_source_version", pitch_source_version.to_owned()),
        ("jlpt_source_sha256", jlpt_source_sha256.to_ascii_uppercase()),
        ("jlpt_source_version", jlpt_source_version.to_owned()),
        (
            "attribution",
            "Pitch accent: NINJAL UniDic; JLPT estimates: stephenmk/yomitan-jlpt-vocab and Jonathan Waller. See bundled notices."
                .to_owned(),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(metadata_error)?;
    }
    Ok(())
}

pub fn insert_pitch(
    connection: &Connection,
    spelling: &str,
    reading: &str,
    accent_type: u32,
) -> Result<(), AppErrorV1> {
    let reading = katakana_to_hiragana(reading.trim());
    let spelling = spelling.trim();
    if spelling.is_empty()
        || spelling.len() > 1_024
        || reading.is_empty()
        || reading.len() > 1_024
        || accent_type > 64
    {
        return Ok(());
    }
    connection
        .prepare_cached(
            "INSERT OR IGNORE INTO pitch_patterns(spelling, reading, accent_type)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(metadata_error)?
        .execute(params![spelling, reading, accent_type])
        .map_err(metadata_error)?;
    Ok(())
}

pub fn insert_jlpt(
    connection: &Connection,
    spelling: &str,
    reading: &str,
    level: u8,
) -> Result<(), AppErrorV1> {
    let reading = katakana_to_hiragana(reading.trim());
    let spelling = spelling.trim();
    if spelling.is_empty()
        || spelling.len() > 1_024
        || reading.is_empty()
        || reading.len() > 1_024
        || !(1..=5).contains(&level)
    {
        return Ok(());
    }
    connection
        .prepare_cached(
            "INSERT OR IGNORE INTO jlpt_levels(spelling, reading, level) VALUES (?1, ?2, ?3)",
        )
        .map_err(metadata_error)?
        .execute(params![spelling, reading, level])
        .map_err(metadata_error)?;
    Ok(())
}

pub fn validate(connection: &Connection) -> Result<(usize, usize), AppErrorV1> {
    validate_with_check(connection, "PRAGMA integrity_check")
}

fn validate_runtime(connection: &Connection) -> Result<(usize, usize), AppErrorV1> {
    validate_with_check(connection, "PRAGMA quick_check(1)")
}

fn validate_with_check(
    connection: &Connection,
    integrity_pragma: &str,
) -> Result<(usize, usize), AppErrorV1> {
    let integrity: String = connection
        .query_row(integrity_pragma, [], |row| row.get(0))
        .map_err(metadata_error)?;
    if integrity != "ok" {
        return Err(metadata_error(format!(
            "metadata integrity check: {integrity}"
        )));
    }
    let application_id: i64 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(metadata_error)?;
    let user_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(metadata_error)?;
    if application_id != APPLICATION_ID || user_version != SCHEMA_VERSION {
        return Err(metadata_error("unsupported lexical metadata schema"));
    }
    for key in [
        "schema_version",
        "pitch_source_sha256",
        "pitch_source_version",
        "jlpt_source_sha256",
        "jlpt_source_version",
        "attribution",
    ] {
        let value = metadata_value(connection, key)?;
        if value.is_empty() {
            return Err(metadata_error(format!("empty metadata value {key}")));
        }
        if key.ends_with("sha256")
            && (value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(metadata_error(format!("malformed metadata checksum {key}")));
        }
    }
    let pitch_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM pitch_patterns", [], |row| row.get(0))
        .map_err(metadata_error)?;
    let jlpt_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM jlpt_levels", [], |row| row.get(0))
        .map_err(metadata_error)?;
    if pitch_count == 0 || jlpt_count == 0 {
        return Err(metadata_error("lexical metadata tables cannot be empty"));
    }
    Ok((
        usize::try_from(pitch_count).map_err(metadata_error)?,
        usize::try_from(jlpt_count).map_err(metadata_error)?,
    ))
}

fn metadata_value(connection: &Connection, key: &str) -> Result<String, AppErrorV1> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(metadata_error)?
        .ok_or_else(|| metadata_error(format!("missing metadata value {key}")))
}

fn lookup_pitch(
    connection: &Connection,
    spellings: &[String],
    readings: &BTreeSet<String>,
    source: &str,
) -> Result<Vec<PitchAccentV1>, AppErrorV1> {
    let placeholders = std::iter::repeat_n("?", spellings.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT spelling, reading, accent_type FROM pitch_patterns
         WHERE spelling IN ({placeholders}) ORDER BY spelling, reading, accent_type LIMIT {MAX_SOURCE_ROWS}"
    );
    let mut statement = connection.prepare_cached(&sql).map_err(metadata_error)?;
    let rows = statement
        .query_map(params_from_iter(spellings), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(metadata_error)?;
    let spelling_set: BTreeSet<_> = spellings.iter().collect();
    let mut patterns = BTreeMap::new();
    for row in rows {
        let (spelling, reading, accent_type) = row.map_err(metadata_error)?;
        if !spelling_set.contains(&spelling) || !readings.contains(&reading) {
            continue;
        }
        let morae = split_morae(&reading);
        let Ok(mora_count) = u32::try_from(morae.len()) else {
            continue;
        };
        if mora_count == 0 || accent_type > mora_count {
            continue;
        }
        let levels = pitch_levels(morae.len(), accent_type);
        patterns
            .entry((reading.clone(), accent_type))
            .or_insert_with(|| PitchAccentV1 {
                reading,
                morae,
                levels,
                drop_after_mora: (accent_type != 0).then_some(accent_type),
                source: source.to_owned(),
            });
    }
    Ok(patterns.into_values().take(MAX_PATTERNS).collect())
}

fn lookup_jlpt(
    connection: &Connection,
    spellings: &[String],
    readings: &BTreeSet<String>,
) -> Result<Option<u8>, AppErrorV1> {
    let placeholders = std::iter::repeat_n("?", spellings.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT spelling, reading, level FROM jlpt_levels
         WHERE spelling IN ({placeholders}) ORDER BY level DESC LIMIT {MAX_SOURCE_ROWS}"
    );
    let mut statement = connection.prepare_cached(&sql).map_err(metadata_error)?;
    let rows = statement
        .query_map(params_from_iter(spellings), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u8>(2)?,
            ))
        })
        .map_err(metadata_error)?;
    let spelling_set: BTreeSet<_> = spellings.iter().collect();
    let mut level = None;
    for row in rows {
        let (spelling, reading, candidate) = row.map_err(metadata_error)?;
        if spelling_set.contains(&spelling) && readings.contains(&reading) {
            level = Some(level.map_or(candidate, |current: u8| current.max(candidate)));
        }
    }
    Ok(level)
}

fn spelling_candidates(token: &TokenV1, entry: &DictionaryEntrySummaryV1) -> Vec<String> {
    unique_candidates(
        entry
            .headwords
            .iter()
            .chain(std::iter::once(&token.lemma))
            .chain(std::iter::once(&token.surface))
            .chain(entry.readings.iter()),
        false,
    )
}

fn reading_candidates(token: &TokenV1, entry: &DictionaryEntrySummaryV1) -> BTreeSet<String> {
    unique_candidates(
        entry
            .readings
            .iter()
            .chain(std::iter::once(&token.reading))
            .chain(token.pronunciation.iter()),
        true,
    )
    .into_iter()
    .collect()
}

fn unique_candidates<'a>(
    values: impl Iterator<Item = &'a String>,
    normalize_kana: bool,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        let candidate = if normalize_kana {
            katakana_to_hiragana(value)
        } else {
            value.clone()
        };
        if !candidate.is_empty() && candidate.len() <= 1_024 && seen.insert(candidate.clone()) {
            result.push(candidate);
            if result.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    result
}

fn katakana_to_hiragana(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\u{30a1}'..='\u{30f6}' => {
                char::from_u32(u32::from(character) - 0x60).unwrap_or(character)
            }
            _ => character,
        })
        .collect()
}

fn split_morae(reading: &str) -> Vec<String> {
    let mut morae = Vec::<String>::new();
    for character in katakana_to_hiragana(reading).chars() {
        if matches!(
            character,
            'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ゕ' | 'ゖ'
        ) && let Some(previous) = morae.last_mut()
        {
            previous.push(character);
        } else if character.is_alphanumeric()
            || matches!(character, 'ー' | 'っ' | 'ん' | 'ゔ' | 'ゝ' | 'ゞ')
        {
            morae.push(character.to_string());
        }
    }
    morae
}

fn pitch_levels(mora_count: usize, accent_type: u32) -> Vec<PitchLevelV1> {
    (0..mora_count)
        .map(|index| {
            let mora = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let high = if accent_type == 0 {
                mora > 1
            } else if accent_type == 1 {
                mora == 1
            } else {
                mora > 1 && mora <= accent_type
            };
            if high {
                PitchLevelV1::High
            } else {
                PitchLevelV1::Low
            }
        })
        .collect()
}

pub(crate) fn metadata_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::DICTIONARY_UNAVAILABLE,
        "Optional pitch-accent and JLPT metadata is unavailable. Definitions remain usable.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use contracts::{DictionaryEntryId, DictionarySenseV1, TokenId};
    use tempfile::NamedTempFile;

    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn token() -> TokenV1 {
        TokenV1 {
            token_id: TokenId::new("token"),
            surface: "心".into(),
            byte_start: 0,
            byte_end: 3,
            lemma: "心".into(),
            reading: "ココロ".into(),
            pronunciation: Some("ココロ".into()),
            part_of_speech: vec!["noun".into()],
            lookup_candidate: true,
        }
    }

    fn entry() -> DictionaryEntrySummaryV1 {
        DictionaryEntrySummaryV1 {
            entry_id: DictionaryEntryId::new("jmdict-1"),
            headwords: vec!["心".into()],
            readings: vec!["こころ".into()],
            senses: vec![DictionarySenseV1 {
                glosses: vec!["heart".into()],
                parts_of_speech: Vec::new(),
                restrictions: Vec::new(),
                fields: Vec::new(),
                dialects: Vec::new(),
                misc: Vec::new(),
            }],
            pitch_accents: Vec::new(),
            jlpt_level: None,
            jlpt_source: None,
            match_reason: "test".into(),
            priority_score: 1,
        }
    }

    #[test]
    fn exact_spelling_and_reading_enrich_without_cross_attaching_homophones()
    -> Result<(), Box<dyn std::error::Error>> {
        let file = NamedTempFile::new()?;
        let connection = Connection::open(file.path())?;
        create(
            &connection,
            SHA,
            "UniDic CSJ 3.1.1",
            SHA,
            "JLPT estimate test",
        )?;
        insert_pitch(&connection, "心", "こころ", 2)?;
        insert_pitch(&connection, "心", "しん", 1)?;
        insert_jlpt(&connection, "心", "こころ", 3)?;
        validate(&connection)?;
        drop(connection);

        let metadata = LexicalMetadata::open(file.path())?;
        let mut result = entry();
        metadata.enrich(&token(), &mut result);
        assert_eq!(result.pitch_accents.len(), 1);
        assert_eq!(result.pitch_accents[0].morae, ["こ", "こ", "ろ"]);
        assert_eq!(result.pitch_accents[0].drop_after_mora, Some(2));
        assert_eq!(
            result.pitch_accents[0].levels,
            [PitchLevelV1::Low, PitchLevelV1::High, PitchLevelV1::Low]
        );
        assert_eq!(result.jlpt_level, Some(3));
        Ok(())
    }

    #[test]
    fn heiban_and_odaka_remain_distinct_and_morae_join_small_kana() {
        assert_eq!(split_morae("きょうと"), ["きょ", "う", "と"]);
        assert_eq!(
            pitch_levels(3, 0),
            [PitchLevelV1::Low, PitchLevelV1::High, PitchLevelV1::High]
        );
        assert_eq!(
            pitch_levels(3, 3),
            [PitchLevelV1::Low, PitchLevelV1::High, PitchLevelV1::High]
        );
    }

    #[test]
    fn rejects_corrupt_or_empty_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let file = NamedTempFile::new()?;
        let connection = Connection::open(file.path())?;
        create(
            &connection,
            SHA,
            "UniDic CSJ 3.1.1",
            SHA,
            "JLPT estimate test",
        )?;
        assert!(validate(&connection).is_err());
        Ok(())
    }
}
