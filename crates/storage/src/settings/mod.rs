use serde::{Serialize, de::DeserializeOwned};

use contracts::AppErrorV1;
use rusqlite::{OptionalExtension, params};

use crate::db::{Storage, storage_error, unix_timestamp};

impl Storage {
    pub fn put_setting<T: Serialize>(
        &self,
        key: &str,
        version: u32,
        value: &T,
    ) -> Result<(), AppErrorV1> {
        validate_key(key)?;
        let json = serde_json::to_string(value).map_err(storage_error)?;
        if json.len() > 1024 * 1024 {
            return Err(storage_error("Setting payload exceeded 1 MiB."));
        }
        self.connection().execute(
            "INSERT INTO settings(key, value_json, settings_version, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, settings_version=excluded.settings_version, updated_at=excluded.updated_at",
            params![key, json, version, unix_timestamp()],
        ).map_err(storage_error)?;
        Ok(())
    }

    pub fn get_setting<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<(u32, T)>, AppErrorV1> {
        validate_key(key)?;
        let stored: Option<(String, u32)> = self
            .connection()
            .query_row(
                "SELECT value_json, settings_version FROM settings WHERE key = ?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        stored
            .map(|(json, version)| {
                serde_json::from_str(&json)
                    .map(|value| (version, value))
                    .map_err(storage_error)
            })
            .transpose()
    }
}

fn validate_key(key: &str) -> Result<(), AppErrorV1> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(storage_error("Setting key was invalid."));
    }
    Ok(())
}
