use std::path::{Path, PathBuf};

use contracts::AppErrorV1;
use rusqlite::{OptionalExtension, params};
use serde::de::DeserializeOwned;

use crate::db::{Storage, storage_error, unix_timestamp};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentMedia {
    pub source_fingerprint: String,
    pub path: PathBuf,
    pub display_name: String,
}

impl Storage {
    pub fn consent_recent(
        &self,
        source_fingerprint: &str,
        path: &Path,
        display_name: &str,
        consented: bool,
    ) -> Result<(), AppErrorV1> {
        validate_fingerprint(source_fingerprint)?;
        if !consented {
            return Err(storage_error(
                "Recent media access requires explicit consent.",
            ));
        }
        if display_name.is_empty() || display_name.len() > 1_024 {
            return Err(storage_error("Recent media display name was invalid."));
        }
        let now = unix_timestamp();
        self.connection().execute(
            "INSERT INTO recents(source_fingerprint, path, display_name, consented_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(source_fingerprint) DO UPDATE SET path=excluded.path, display_name=excluded.display_name, last_opened_at=excluded.last_opened_at",
            params![source_fingerprint, path.as_os_str().to_string_lossy(), display_name, now],
        ).map_err(storage_error)?;
        Ok(())
    }

    pub fn recent(&self, fingerprint: &str) -> Result<Option<RecentMedia>, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        self.connection().query_row(
            "SELECT source_fingerprint, path, display_name FROM recents WHERE source_fingerprint = ?1",
            [fingerprint],
            |row| Ok(RecentMedia { source_fingerprint: row.get(0)?, path: PathBuf::from(row.get::<_, String>(1)?), display_name: row.get(2)? }),
        ).optional().map_err(storage_error)
    }

    pub fn recent_media(&self, limit: usize) -> Result<Vec<RecentMedia>, AppErrorV1> {
        let limit = limit.min(100);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT source_fingerprint, path, display_name FROM recents ORDER BY last_opened_at DESC, source_fingerprint ASC LIMIT ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([limit as i64], |row| {
                Ok(RecentMedia {
                    source_fingerprint: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    display_name: row.get(2)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    pub fn bind_subtitle<T: serde::Serialize>(
        &self,
        fingerprint: &str,
        source_version: &str,
        binding: &T,
    ) -> Result<(), AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        if source_version.is_empty() || source_version.len() > 256 {
            return Err(storage_error("Subtitle source version was invalid."));
        }
        let json = serde_json::to_string(binding).map_err(storage_error)?;
        if json.len() > 1024 * 1024 {
            return Err(storage_error("Subtitle binding exceeded 1 MiB."));
        }
        self.connection().execute(
            "INSERT INTO subtitle_bindings(source_fingerprint, subtitle_source_version, binding_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_fingerprint) DO UPDATE SET subtitle_source_version=excluded.subtitle_source_version, binding_json=excluded.binding_json, updated_at=excluded.updated_at",
            params![fingerprint, source_version, json, unix_timestamp()],
        ).map_err(storage_error)?;
        Ok(())
    }

    pub fn subtitle_binding<T: DeserializeOwned>(
        &self,
        fingerprint: &str,
    ) -> Result<Option<(String, T)>, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        let stored: Option<(String, String)> = self
            .connection()
            .query_row(
                "SELECT subtitle_source_version, binding_json FROM subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        stored
            .map(|(version, json)| {
                serde_json::from_str(&json)
                    .map(|binding| (version, binding))
                    .map_err(storage_error)
            })
            .transpose()
    }

    pub fn remove_subtitle_binding(&self, fingerprint: &str) -> Result<bool, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        self.connection()
            .execute(
                "DELETE FROM subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
            )
            .map(|changed| changed > 0)
            .map_err(storage_error)
    }

    pub fn bind_translation_subtitle<T: serde::Serialize>(
        &self,
        fingerprint: &str,
        source_version: &str,
        binding: &T,
    ) -> Result<(), AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        if source_version.is_empty() || source_version.len() > 256 {
            return Err(storage_error(
                "Translation subtitle source version was invalid.",
            ));
        }
        let json = serde_json::to_string(binding).map_err(storage_error)?;
        if json.len() > 1024 * 1024 {
            return Err(storage_error(
                "Translation subtitle binding exceeded 1 MiB.",
            ));
        }
        self.connection().execute(
            "INSERT INTO translation_subtitle_bindings(source_fingerprint, subtitle_source_version, binding_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_fingerprint) DO UPDATE SET subtitle_source_version=excluded.subtitle_source_version, binding_json=excluded.binding_json, updated_at=excluded.updated_at",
            params![fingerprint, source_version, json, unix_timestamp()],
        ).map_err(storage_error)?;
        Ok(())
    }

    pub fn translation_subtitle_binding<T: DeserializeOwned>(
        &self,
        fingerprint: &str,
    ) -> Result<Option<(String, T)>, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        let stored: Option<(String, String)> = self
            .connection()
            .query_row(
                "SELECT subtitle_source_version, binding_json FROM translation_subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        stored
            .map(|(version, json)| {
                serde_json::from_str(&json)
                    .map(|binding| (version, binding))
                    .map_err(storage_error)
            })
            .transpose()
    }

    pub fn remove_translation_subtitle_binding(
        &self,
        fingerprint: &str,
    ) -> Result<bool, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        self.connection()
            .execute(
                "DELETE FROM translation_subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
            )
            .map(|changed| changed > 0)
            .map_err(storage_error)
    }

    pub fn revoke_recent(&self, fingerprint: &str) -> Result<bool, AppErrorV1> {
        validate_fingerprint(fingerprint)?;
        let mut connection = self.connection();
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM translation_subtitle_bindings WHERE source_fingerprint=?1",
                [fingerprint],
            )
            .map_err(storage_error)?;
        let changed = transaction
            .execute(
                "DELETE FROM recents WHERE source_fingerprint=?1",
                [fingerprint],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(changed > 0)
    }
}

fn validate_fingerprint(fingerprint: &str) -> Result<(), AppErrorV1> {
    if fingerprint.is_empty()
        || fingerprint.len() > 256
        || fingerprint.chars().any(char::is_control)
    {
        return Err(storage_error("Source fingerprint was invalid."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Binding {
        language: String,
    }

    #[test]
    fn recents_require_consent_and_can_be_revoked() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let path = Path::new("C:/日本語/video.mkv");
        assert!(
            storage
                .consent_recent("fingerprint", path, "video.mkv", false)
                .is_err()
        );
        assert!(storage.recent("fingerprint")?.is_none());
        storage.consent_recent("fingerprint", path, "video.mkv", true)?;
        assert_eq!(
            storage.recent("fingerprint")?.map(|item| item.path),
            Some(path.into())
        );
        assert!(storage.revoke_recent("fingerprint")?);
        assert!(storage.recent("fingerprint")?.is_none());
        Ok(())
    }

    #[test]
    fn subtitle_binding_round_trips_with_version() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let binding = Binding {
            language: "ja".into(),
        };
        storage.bind_subtitle("fingerprint", "subtitle-v1", &binding)?;
        assert_eq!(
            storage.subtitle_binding::<Binding>("fingerprint")?,
            Some(("subtitle-v1".into(), binding))
        );
        Ok(())
    }

    #[test]
    fn translation_binding_is_independent_and_removable() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let japanese = Binding {
            language: "ja".into(),
        };
        let english = Binding {
            language: "en".into(),
        };
        storage.bind_subtitle("fingerprint", "jp-v1", &japanese)?;
        storage.bind_translation_subtitle("fingerprint", "en-v1", &english)?;
        assert_eq!(
            storage.subtitle_binding::<Binding>("fingerprint")?,
            Some(("jp-v1".into(), japanese))
        );
        assert_eq!(
            storage.translation_subtitle_binding::<Binding>("fingerprint")?,
            Some(("en-v1".into(), english))
        );
        assert!(storage.remove_translation_subtitle_binding("fingerprint")?);
        assert!(
            storage
                .translation_subtitle_binding::<Binding>("fingerprint")?
                .is_none()
        );
        assert!(
            storage
                .subtitle_binding::<Binding>("fingerprint")?
                .is_some()
        );
        assert!(storage.remove_subtitle_binding("fingerprint")?);
        assert!(
            storage
                .subtitle_binding::<Binding>("fingerprint")?
                .is_none()
        );
        Ok(())
    }
}
