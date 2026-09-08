use std::collections::{BTreeMap, BTreeSet};

use contracts::{AppErrorV1, error_codes};
use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA_VERSION: &str = "3";
const APPLICATION_ID: i64 = 0x4d49_474b;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEntry {
    pub sequence: i64,
    pub writings: Vec<(String, i32)>,
    pub readings: Vec<ImportReading>,
    pub senses: Vec<ImportSense>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReading {
    pub text: String,
    pub priority: i32,
    pub restrictions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportSense {
    pub glosses: Vec<String>,
    pub parts_of_speech: Vec<String>,
    pub restrictions: Vec<String>,
    pub fields: Vec<String>,
    pub dialects: Vec<String>,
    pub misc: Vec<String>,
}

pub fn create(
    connection: &Connection,
    source_sha256: &str,
    source_version: &str,
) -> Result<(), AppErrorV1> {
    if source_sha256.len() != 64 || !source_sha256.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(db_error("source checksum must be a 64-character SHA-256"));
    }
    if source_version.is_empty() || source_version.len() > 128 {
        return Err(db_error("source version must contain 1 to 128 bytes"));
    }
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
         PRAGMA application_id = 1296648011;
         PRAGMA user_version = 3;
         CREATE TABLE metadata (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         ) STRICT;
         CREATE TABLE entries (
           sequence INTEGER PRIMARY KEY
         ) STRICT;
         CREATE TABLE forms (
           entry_sequence INTEGER NOT NULL REFERENCES entries(sequence) ON DELETE CASCADE,
           text TEXT NOT NULL,
           kind TEXT NOT NULL CHECK(kind IN ('writing', 'reading')),
           priority INTEGER NOT NULL DEFAULT 0,
           restrictions_json TEXT NOT NULL DEFAULT '[]',
           PRIMARY KEY(entry_sequence, text, kind)
         ) STRICT;
         CREATE INDEX forms_text_idx ON forms(text);
         CREATE TABLE senses (
           entry_sequence INTEGER NOT NULL REFERENCES entries(sequence) ON DELETE CASCADE,
           sense_order INTEGER NOT NULL,
           glosses_json TEXT NOT NULL,
           pos_json TEXT NOT NULL,
           restrictions_json TEXT NOT NULL,
           fields_json TEXT NOT NULL,
           dialects_json TEXT NOT NULL,
           misc_json TEXT NOT NULL,
           PRIMARY KEY(entry_sequence, sense_order)
         ) STRICT;",
        )
        .map_err(db_error)?;
    for (key, value) in [
        ("schema_version", SCHEMA_VERSION),
        ("source_sha256", source_sha256),
        ("source_version", source_version),
        (
            "attribution",
            "JMdict by the Electronic Dictionary Research and Development Group; see EDRDG licence.",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

pub fn insert(connection: &Connection, entry: &ImportEntry) -> Result<(), AppErrorV1> {
    if entry.sequence <= 0
        || (entry.writings.is_empty() && entry.readings.is_empty())
        || entry.senses.is_empty()
    {
        return Err(db_error(
            "dictionary entry needs a positive sequence, forms, and senses",
        ));
    }
    let writings: BTreeSet<_> = entry
        .writings
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    let readings: BTreeSet<_> = entry
        .readings
        .iter()
        .map(|reading| reading.text.as_str())
        .collect();
    if writings
        .iter()
        .chain(readings.iter())
        .any(|text| text.is_empty())
    {
        return Err(db_error("dictionary forms cannot be empty"));
    }
    if entry.readings.iter().any(|reading| {
        reading
            .restrictions
            .iter()
            .any(|restriction| !writings.contains(restriction.as_str()))
    }) {
        return Err(db_error(
            "reading restriction does not name a written form in its entry",
        ));
    }
    if entry.senses.iter().any(|sense| {
        sense.glosses.is_empty()
            || sense.restrictions.iter().any(|restriction| {
                !writings.contains(restriction.as_str()) && !readings.contains(restriction.as_str())
            })
    }) {
        return Err(db_error(
            "dictionary sense is empty or has an invalid restriction",
        ));
    }
    connection
        .execute(
            "INSERT INTO entries(sequence) VALUES (?1)",
            [entry.sequence],
        )
        .map_err(db_error)?;
    for (text, priority) in consolidate_forms(&entry.writings) {
        connection.execute(
            "INSERT INTO forms(entry_sequence, text, kind, priority) VALUES (?1, ?2, 'writing', ?3)",
            params![entry.sequence, text, priority],
        ).map_err(db_error)?;
    }
    for (text, (priority, restrictions)) in consolidate_readings(&entry.readings) {
        connection
            .execute(
                "INSERT INTO forms(entry_sequence, text, kind, priority, restrictions_json)
             VALUES (?1, ?2, 'reading', ?3, ?4)",
                params![
                    entry.sequence,
                    text,
                    priority,
                    serde_json::to_string(&restrictions).map_err(json_error)?
                ],
            )
            .map_err(db_error)?;
    }
    for (index, sense) in entry.senses.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO senses(
               entry_sequence, sense_order, glosses_json, pos_json, restrictions_json,
               fields_json, dialects_json, misc_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.sequence,
                    index as i64,
                    serde_json::to_string(&sense.glosses).map_err(json_error)?,
                    serde_json::to_string(&sense.parts_of_speech).map_err(json_error)?,
                    serde_json::to_string(&sense.restrictions).map_err(json_error)?,
                    serde_json::to_string(&sense.fields).map_err(json_error)?,
                    serde_json::to_string(&sense.dialects).map_err(json_error)?,
                    serde_json::to_string(&sense.misc).map_err(json_error)?,
                ],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

pub fn validate(connection: &Connection) -> Result<usize, AppErrorV1> {
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(db_error)?;
    if integrity != "ok" {
        return Err(db_error(format!("integrity check: {integrity}")));
    }
    let application_id: i64 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(db_error)?;
    if application_id != APPLICATION_ID {
        return Err(db_error(format!(
            "unexpected dictionary application id {application_id}"
        )));
    }
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_error)?;
    if user_version.to_string() != SCHEMA_VERSION {
        return Err(db_error(format!(
            "unsupported dictionary user version {user_version}"
        )));
    }
    let version: String = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if version != SCHEMA_VERSION {
        return Err(db_error(format!("unsupported schema version {version}")));
    }
    for key in ["source_sha256", "source_version", "attribution"] {
        let value: String = connection
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .map_err(db_error)?;
        if value.is_empty() {
            return Err(db_error(format!("empty required metadata value {key}")));
        }
        if key == "source_sha256"
            && (value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(db_error("dictionary source checksum is malformed"));
        }
        if key == "source_version" && value.len() > 128 {
            return Err(db_error("dictionary source version is too long"));
        }
    }
    let foreign_key_violation: Option<i64> = connection
        .query_row(
            "SELECT rowid FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if foreign_key_violation.is_some() {
        return Err(db_error("dictionary contains a foreign-key violation"));
    }
    let invalid_json_shape: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM forms
                WHERE CASE WHEN json_valid(restrictions_json)
                           THEN json_type(restrictions_json) <> 'array'
                           ELSE 1 END) +
               (SELECT COUNT(*) FROM senses
                WHERE CASE WHEN json_valid(glosses_json)
                           THEN json_type(glosses_json) <> 'array'
                           ELSE 1 END
                   OR CASE WHEN json_valid(pos_json)
                           THEN json_type(pos_json) <> 'array'
                           ELSE 1 END
                   OR CASE WHEN json_valid(restrictions_json)
                           THEN json_type(restrictions_json) <> 'array'
                           ELSE 1 END
                   OR CASE WHEN json_valid(fields_json)
                           THEN json_type(fields_json) <> 'array'
                           ELSE 1 END
                   OR CASE WHEN json_valid(dialects_json)
                           THEN json_type(dialects_json) <> 'array'
                           ELSE 1 END
                   OR CASE WHEN json_valid(misc_json)
                           THEN json_type(misc_json) <> 'array'
                           ELSE 1 END)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if invalid_json_shape != 0 {
        return Err(db_error("dictionary JSON fields must be valid JSON arrays"));
    }
    let non_string_json_values: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM forms, json_each(forms.restrictions_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.glosses_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.pos_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.restrictions_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.fields_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.dialects_json) AS item
                WHERE item.type <> 'text') +
               (SELECT COUNT(*) FROM senses, json_each(senses.misc_json) AS item
                WHERE item.type <> 'text')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if non_string_json_values != 0 {
        return Err(db_error("dictionary JSON arrays may contain only strings"));
    }
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
        .map_err(db_error)?;
    usize::try_from(count).map_err(|_| db_error("entry count overflow"))
}

fn consolidate_forms(forms: &[(String, i32)]) -> BTreeMap<&str, i32> {
    let mut consolidated = BTreeMap::new();
    for (text, priority) in forms {
        consolidated
            .entry(text.as_str())
            .and_modify(|current: &mut i32| *current = (*current).max(*priority))
            .or_insert(*priority);
    }
    consolidated
}

fn consolidate_readings(readings: &[ImportReading]) -> BTreeMap<&str, (i32, Vec<&str>)> {
    let mut consolidated: BTreeMap<&str, (i32, Option<BTreeSet<&str>>)> = BTreeMap::new();
    for reading in readings {
        let restrictions = if reading.restrictions.is_empty() {
            None
        } else {
            Some(reading.restrictions.iter().map(String::as_str).collect())
        };
        consolidated
            .entry(&reading.text)
            .and_modify(|(priority, current_restrictions)| {
                *priority = (*priority).max(reading.priority);
                match (&mut *current_restrictions, &restrictions) {
                    (Some(current), Some(additional)) => current.extend(additional),
                    _ => *current_restrictions = None,
                }
            })
            .or_insert((reading.priority, restrictions));
    }
    consolidated
        .into_iter()
        .map(|(text, (priority, restrictions))| {
            (
                text,
                (
                    priority,
                    restrictions.map_or_else(Vec::new, |values| values.into_iter().collect()),
                ),
            )
        })
        .collect()
}

pub(crate) fn db_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::DICTIONARY_UNAVAILABLE,
        "The bundled local dictionary is unavailable or incompatible. Repair or reinstall the application.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn json_error(error: serde_json::Error) -> AppErrorV1 {
    db_error(error)
}

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;

    use super::*;

    const TEST_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn rejects_unpinned_source_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let file = NamedTempFile::new()?;
        let connection = Connection::open(file.path())?;
        assert!(create(&connection, "not-a-sha", "test").is_err());
        Ok(())
    }

    #[test]
    fn duplicate_forms_collapse_to_the_highest_priority() -> Result<(), Box<dyn std::error::Error>>
    {
        let file = NamedTempFile::new()?;
        let connection = Connection::open(file.path())?;
        create(&connection, TEST_SHA256, "test")?;
        insert(
            &connection,
            &ImportEntry {
                sequence: 1,
                writings: vec![("見る".into(), 10), ("見る".into(), 100)],
                readings: Vec::new(),
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
        let (count, priority): (i64, i32) =
            connection.query_row("SELECT COUNT(*), MAX(priority) FROM forms", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        assert_eq!((count, priority), (1, 100));
        assert_eq!(validate(&connection)?, 1);
        Ok(())
    }

    #[test]
    fn validation_rejects_non_array_or_non_string_json_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let file = NamedTempFile::new()?;
        let connection = Connection::open(file.path())?;
        create(&connection, TEST_SHA256, "test")?;
        insert(
            &connection,
            &ImportEntry {
                sequence: 1,
                writings: vec![("見る".into(), 100)],
                readings: Vec::new(),
                senses: vec![ImportSense {
                    glosses: vec!["to see".into()],
                    parts_of_speech: vec!["verb".into()],
                    restrictions: Vec::new(),
                    fields: vec!["cinematography".into()],
                    dialects: vec!["Tokyo-ben".into()],
                    misc: vec!["common usage".into()],
                }],
            },
        )?;

        for (table, column) in [
            ("forms", "restrictions_json"),
            ("senses", "glosses_json"),
            ("senses", "pos_json"),
            ("senses", "restrictions_json"),
            ("senses", "fields_json"),
            ("senses", "dialects_json"),
            ("senses", "misc_json"),
        ] {
            let update = format!("UPDATE {table} SET {column} = ?1");
            connection.execute(&update, [r#""scalar""#])?;
            let scalar_error = validate(&connection)
                .err()
                .ok_or("scalar JSON was accepted")?;
            assert!(
                scalar_error
                    .diagnostics
                    .as_deref()
                    .is_some_and(|value| value.contains("valid JSON arrays"))
            );

            connection.execute(&update, ["[1]"])?;
            let element_error = validate(&connection)
                .err()
                .ok_or("non-string JSON array element was accepted")?;
            assert!(
                element_error
                    .diagnostics
                    .as_deref()
                    .is_some_and(|value| value.contains("only strings"))
            );
            connection.execute(&update, ["[]"])?;
        }
        assert_eq!(validate(&connection)?, 1);
        Ok(())
    }
}
