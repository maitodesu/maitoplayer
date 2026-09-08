use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::Path,
};

use contracts::{
    AppErrorV1, DictionaryEntryId, DictionaryEntrySummaryV1, DictionarySenseV1, TokenV1,
};
use parking_lot::Mutex;
use ports::DictionaryPort;
use rusqlite::{Connection, OpenFlags, params};

use crate::schema::{SCHEMA_VERSION, db_error, validate};

const MAX_RESULTS: usize = 20;
const MAX_MATCHES_PER_CANDIDATE: usize = 256;
const MAX_LOOKUP_CACHE_ENTRIES: usize = 2_048;
const MAX_LOOKUP_CACHE_WEIGHT: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct SqliteDictionary {
    connection: Mutex<Connection>,
    cache: Mutex<LookupCache>,
    version: String,
}

impl SqliteDictionary {
    pub fn open(path: &Path) -> Result<Self, AppErrorV1> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db_error)?;
        connection
            .pragma_update(None, "query_only", "ON")
            .map_err(db_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(db_error)?;
        validate(&connection)?;
        let source_version: String = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'source_version'",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            cache: Mutex::new(LookupCache::default()),
            version: format!("jmdict-schema-{SCHEMA_VERSION}:{source_version}"),
        })
    }

    #[cfg(test)]
    fn cache_metrics(&self) -> (usize, usize) {
        let cache = self.cache.lock();
        (cache.entries.len(), cache.weight)
    }
}

impl DictionaryPort for SqliteDictionary {
    fn lookup(&self, token: &TokenV1) -> Result<Vec<DictionaryEntrySummaryV1>, AppErrorV1> {
        if !token.lookup_candidate {
            return Ok(Vec::new());
        }
        let cache_key = LookupCacheKey::from(token);
        if let Some(cached) = self.cache.lock().get(&cache_key) {
            return Ok(cached);
        }
        let candidates = candidate_values(token);
        let connection = self.connection.lock();
        let mut entries: BTreeMap<i64, EntryAccumulator> = BTreeMap::new();
        for candidate in &candidates {
            let mut statement = connection
                .prepare_cached(
                    "SELECT entry_sequence, kind, priority FROM forms WHERE text = ?1
                 ORDER BY priority DESC, entry_sequence LIMIT ?2",
                )
                .map_err(db_error)?;
            let rows = statement
                .query_map(
                    params![candidate.value, MAX_MATCHES_PER_CANDIDATE as i64],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i32>(2)?,
                        ))
                    },
                )
                .map_err(db_error)?;
            for row in rows {
                let (sequence, kind, priority) = row.map_err(db_error)?;
                let score = candidate.base_score + priority.clamp(0, 100);
                let accumulator = entries.entry(sequence).or_default();
                if score > accumulator.score {
                    accumulator.score = score;
                    accumulator.match_reason =
                        format!("{}_{}:{}", candidate.match_kind, kind, candidate.value);
                    accumulator.matched_form.clone_from(&candidate.value);
                    accumulator.matched_kind.clone_from(&kind);
                }
            }
        }
        let mut results = Vec::with_capacity(entries.len().min(MAX_RESULTS));
        for (sequence, accumulator) in entries {
            let entry = load_entry(&connection, sequence, accumulator)?;
            if !entry.senses.is_empty() {
                results.push(entry);
            }
        }
        results.sort_by(|left, right| {
            right
                .priority_score
                .cmp(&left.priority_score)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        results.truncate(MAX_RESULTS);
        drop(connection);
        self.cache.lock().insert(cache_key, results.clone());
        Ok(results)
    }

    fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LookupCacheKey {
    surface: String,
    lemma: String,
    reading: String,
}

impl From<&TokenV1> for LookupCacheKey {
    fn from(token: &TokenV1) -> Self {
        Self {
            surface: token.surface.clone(),
            lemma: token.lemma.clone(),
            reading: token.reading.clone(),
        }
    }
}

impl LookupCacheKey {
    fn weight(&self) -> usize {
        self.surface.len() + self.lemma.len() + self.reading.len() + 64
    }
}

#[derive(Debug)]
struct CachedLookup {
    results: Vec<DictionaryEntrySummaryV1>,
    weight: usize,
}

#[derive(Debug, Default)]
struct LookupCache {
    entries: HashMap<LookupCacheKey, CachedLookup>,
    insertion_order: VecDeque<LookupCacheKey>,
    weight: usize,
}

impl LookupCache {
    fn get(&self, key: &LookupCacheKey) -> Option<Vec<DictionaryEntrySummaryV1>> {
        self.entries.get(key).map(|entry| entry.results.clone())
    }

    fn insert(&mut self, key: LookupCacheKey, results: Vec<DictionaryEntrySummaryV1>) {
        if self.entries.contains_key(&key) {
            return;
        }
        // Serialized length closely tracks the owned strings and vectors that
        // dominate these immutable summaries. Per-entry and container overhead
        // keeps the cap conservative without walking every nested string twice.
        let payload_weight = serde_json::to_vec(&results)
            .map_or(MAX_LOOKUP_CACHE_WEIGHT.saturating_add(1), |payload| {
                payload.len()
            });
        let weight = key
            .weight()
            .saturating_add(payload_weight)
            .saturating_add(results.len().saturating_mul(256))
            .saturating_add(128);
        if weight > MAX_LOOKUP_CACHE_WEIGHT {
            return;
        }
        while self.entries.len() >= MAX_LOOKUP_CACHE_ENTRIES
            || self.weight.saturating_add(weight) > MAX_LOOKUP_CACHE_WEIGHT
        {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.weight = self.weight.saturating_sub(removed.weight);
            }
        }
        self.insertion_order.push_back(key.clone());
        self.entries.insert(key, CachedLookup { results, weight });
        self.weight = self.weight.saturating_add(weight);
    }
}

#[derive(Debug, Default)]
struct EntryAccumulator {
    score: i32,
    match_reason: String,
    matched_form: String,
    matched_kind: String,
}

#[derive(Debug)]
struct LookupCandidate {
    value: String,
    base_score: i32,
    match_kind: &'static str,
}

fn candidate_values(token: &TokenV1) -> Vec<LookupCandidate> {
    let mut candidates = Vec::with_capacity(6);
    let mut seen = BTreeSet::new();
    for (value, score, kind) in [
        (&token.lemma, 1_000, "exact_lemma"),
        (&token.surface, 800, "exact_surface"),
        (&token.reading, 600, "exact_reading"),
    ] {
        if !value.is_empty() && value.len() <= 1_024 && seen.insert(value.clone()) {
            candidates.push(LookupCandidate {
                value: value.clone(),
                base_score: score,
                match_kind: kind,
            });
        }
        let normalized = katakana_to_hiragana(value);
        if normalized != *value && normalized.len() <= 1_024 && seen.insert(normalized.clone()) {
            candidates.push(LookupCandidate {
                value: normalized,
                base_score: score - 25,
                match_kind: "normalized_kana",
            });
        }
    }
    candidates
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

fn load_entry(
    connection: &Connection,
    sequence: i64,
    accumulator: EntryAccumulator,
) -> Result<DictionaryEntrySummaryV1, AppErrorV1> {
    let mut forms = connection
        .prepare_cached(
            "SELECT text, kind, restrictions_json FROM forms WHERE entry_sequence = ?1
         ORDER BY kind, priority DESC, text LIMIT 32",
        )
        .map_err(db_error)?;
    let rows = forms
        .query_map([sequence], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error)?;
    let mut loaded_forms = Vec::new();
    for row in rows {
        let (text, kind, restrictions) = row.map_err(db_error)?;
        loaded_forms.push((
            text,
            kind,
            serde_json::from_str::<Vec<String>>(&restrictions).map_err(db_error)?,
        ));
    }
    let matched_reading_restrictions = loaded_forms
        .iter()
        .find(|(text, kind, _)| {
            kind == "reading"
                && text == &accumulator.matched_form
                && accumulator.matched_kind == "reading"
        })
        .map(|(_, _, restrictions)| restrictions.as_slice())
        .unwrap_or_default();
    let headwords: Vec<_> = loaded_forms
        .iter()
        .filter(|(text, kind, _)| {
            kind == "writing"
                && (matched_reading_restrictions.is_empty()
                    || matched_reading_restrictions.contains(text))
        })
        .map(|(text, _, _)| text.clone())
        .collect();
    let readings: Vec<_> = loaded_forms
        .iter()
        .filter(|(text, kind, restrictions)| {
            kind == "reading"
                && (accumulator.matched_kind != "writing"
                    || restrictions.is_empty()
                    || restrictions.contains(&accumulator.matched_form))
                && (accumulator.matched_kind != "reading"
                    || text == &accumulator.matched_form
                    || restrictions.is_empty())
        })
        .map(|(text, _, _)| text.clone())
        .collect();
    let applicable_forms: BTreeSet<_> = headwords
        .iter()
        .chain(readings.iter())
        .chain(std::iter::once(&accumulator.matched_form))
        .cloned()
        .collect();
    let mut statement = connection.prepare_cached(
        "SELECT glosses_json, pos_json, restrictions_json, fields_json, dialects_json, misc_json
         FROM senses WHERE entry_sequence = ?1 ORDER BY sense_order LIMIT 16",
    ).map_err(db_error)?;
    let rows = statement
        .query_map(params![sequence], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(db_error)?;
    let mut senses = Vec::new();
    for row in rows {
        let (glosses, parts_of_speech, restrictions, fields, dialects, misc) =
            row.map_err(db_error)?;
        let restrictions: Vec<String> = serde_json::from_str(&restrictions).map_err(db_error)?;
        if restrictions.is_empty()
            || restrictions
                .iter()
                .any(|restriction| applicable_forms.contains(restriction))
        {
            senses.push(DictionarySenseV1 {
                glosses: serde_json::from_str(&glosses).map_err(db_error)?,
                parts_of_speech: serde_json::from_str(&parts_of_speech).map_err(db_error)?,
                restrictions,
                fields: serde_json::from_str(&fields).map_err(db_error)?,
                dialects: serde_json::from_str(&dialects).map_err(db_error)?,
                misc: serde_json::from_str(&misc).map_err(db_error)?,
            });
        }
    }
    Ok(DictionaryEntrySummaryV1 {
        entry_id: DictionaryEntryId::new(format!("jmdict-{sequence}")),
        headwords,
        readings,
        senses,
        pitch_accents: Vec::new(),
        jlpt_level: None,
        jlpt_source: None,
        match_reason: accumulator.match_reason,
        priority_score: accumulator.score,
    })
}

#[cfg(test)]
mod tests {
    use contracts::TokenId;
    use tempfile::NamedTempFile;

    use crate::schema::{ImportEntry, ImportReading, ImportSense, create, insert};

    use super::*;

    const TEST_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn ranks_lemma_exact_match() -> Result<(), AppErrorV1> {
        let file = NamedTempFile::new().map_err(db_error)?;
        let connection = Connection::open(file.path()).map_err(db_error)?;
        create(&connection, TEST_SHA256, "test")?;
        insert(
            &connection,
            &ImportEntry {
                sequence: 1,
                writings: vec![("見る".into(), 50)],
                readings: vec![ImportReading {
                    text: "みる".into(),
                    priority: 0,
                    restrictions: Vec::new(),
                }],
                senses: vec![ImportSense {
                    glosses: vec!["to see".into()],
                    parts_of_speech: vec!["verb".into()],
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: Vec::new(),
                }],
            },
        )?;
        drop(connection);
        let dictionary = SqliteDictionary::open(file.path())?;
        let results = dictionary.lookup(&TokenV1 {
            token_id: TokenId::new("t"),
            surface: "見".into(),
            byte_start: 0,
            byte_end: 3,
            lemma: "見る".into(),
            reading: "ミ".into(),
            pronunciation: None,
            part_of_speech: vec!["動詞".into()],
            lookup_candidate: true,
        })?;
        assert_eq!(results[0].headwords, vec!["見る"]);
        assert_eq!(results[0].priority_score, 1_050);
        assert_eq!(results[0].match_reason, "exact_lemma_writing:見る");
        Ok(())
    }

    #[test]
    fn normalizes_katakana_readings_for_jmdict_lookup() -> Result<(), AppErrorV1> {
        let file = NamedTempFile::new().map_err(db_error)?;
        let connection = Connection::open(file.path()).map_err(db_error)?;
        create(&connection, TEST_SHA256, "test")?;
        insert(
            &connection,
            &ImportEntry {
                sequence: 2,
                writings: Vec::new(),
                readings: vec![ImportReading {
                    text: "みる".into(),
                    priority: 0,
                    restrictions: Vec::new(),
                }],
                senses: vec![ImportSense {
                    glosses: vec!["to see".into()],
                    parts_of_speech: vec!["verb".into()],
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: Vec::new(),
                }],
            },
        )?;
        drop(connection);
        let dictionary = SqliteDictionary::open(file.path())?;
        let results = dictionary.lookup(&TokenV1 {
            token_id: TokenId::new("t"),
            surface: "ミル".into(),
            byte_start: 0,
            byte_end: 6,
            lemma: "ミル".into(),
            reading: "ミル".into(),
            pronunciation: None,
            part_of_speech: vec!["verb".into()],
            lookup_candidate: true,
        })?;
        assert_eq!(results[0].readings, vec!["みる"]);
        assert_eq!(results[0].priority_score, 975);
        assert_eq!(results[0].match_reason, "normalized_kana_reading:みる");
        Ok(())
    }

    #[test]
    fn lookup_ties_are_ordered_by_stable_entry_id() -> Result<(), AppErrorV1> {
        let file = NamedTempFile::new().map_err(db_error)?;
        let connection = Connection::open(file.path()).map_err(db_error)?;
        create(&connection, TEST_SHA256, "test")?;
        for sequence in [20, 10] {
            insert(
                &connection,
                &ImportEntry {
                    sequence,
                    writings: vec![("同じ".into(), 0)],
                    readings: vec![ImportReading {
                        text: "おなじ".into(),
                        priority: 0,
                        restrictions: Vec::new(),
                    }],
                    senses: vec![ImportSense {
                        glosses: vec![sequence.to_string()],
                        parts_of_speech: vec!["adjective".into()],
                        restrictions: Vec::new(),
                        fields: Vec::new(),
                        dialects: Vec::new(),
                        misc: Vec::new(),
                    }],
                },
            )?;
        }
        drop(connection);
        let dictionary = SqliteDictionary::open(file.path())?;
        let results = dictionary.lookup(&TokenV1 {
            token_id: TokenId::new("t"),
            surface: "同じ".into(),
            byte_start: 0,
            byte_end: 6,
            lemma: "同じ".into(),
            reading: "オナジ".into(),
            pronunciation: None,
            part_of_speech: vec!["adjective".into()],
            lookup_candidate: true,
        })?;
        assert_eq!(results[0].entry_id.as_str(), "jmdict-10");
        assert_eq!(results[1].entry_id.as_str(), "jmdict-20");
        Ok(())
    }

    #[test]
    fn reading_restrictions_filter_inapplicable_forms() -> Result<(), AppErrorV1> {
        let file = NamedTempFile::new().map_err(db_error)?;
        let connection = Connection::open(file.path()).map_err(db_error)?;
        create(&connection, TEST_SHA256, "test")?;
        insert(
            &connection,
            &ImportEntry {
                sequence: 30,
                writings: vec![("見る".into(), 0), ("観る".into(), 0)],
                readings: vec![ImportReading {
                    text: "みる".into(),
                    priority: 0,
                    restrictions: vec!["見る".into()],
                }],
                senses: vec![ImportSense {
                    glosses: vec!["to see".into()],
                    parts_of_speech: vec!["verb".into()],
                    restrictions: Vec::new(),
                    fields: Vec::new(),
                    dialects: Vec::new(),
                    misc: Vec::new(),
                }],
            },
        )?;
        drop(connection);
        let dictionary = SqliteDictionary::open(file.path())?;
        let results = dictionary.lookup(&TokenV1 {
            token_id: TokenId::new("t"),
            surface: "観る".into(),
            byte_start: 0,
            byte_end: 6,
            lemma: "観る".into(),
            reading: "ミル".into(),
            pronunciation: None,
            part_of_speech: vec!["verb".into()],
            lookup_candidate: true,
        })?;
        assert!(results[0].readings.is_empty());
        Ok(())
    }

    #[test]
    fn repeated_lookups_use_a_bounded_cache() -> Result<(), AppErrorV1> {
        let file = NamedTempFile::new().map_err(db_error)?;
        let connection = Connection::open(file.path()).map_err(db_error)?;
        create(&connection, TEST_SHA256, "test")?;
        drop(connection);
        let dictionary = SqliteDictionary::open(file.path())?;

        for index in 0..=MAX_LOOKUP_CACHE_ENTRIES {
            let value = format!("missing-{index}");
            let token = TokenV1 {
                token_id: TokenId::new(format!("t-{index}")),
                surface: value.clone(),
                byte_start: 0,
                byte_end: u32::try_from(value.len()).unwrap_or(u32::MAX),
                lemma: value.clone(),
                reading: value,
                pronunciation: None,
                part_of_speech: vec!["unknown".into()],
                lookup_candidate: true,
            };
            assert!(dictionary.lookup(&token)?.is_empty());
        }

        let (entries, weight) = dictionary.cache_metrics();
        assert_eq!(entries, MAX_LOOKUP_CACHE_ENTRIES);
        assert!(weight <= MAX_LOOKUP_CACHE_WEIGHT);
        Ok(())
    }
}
