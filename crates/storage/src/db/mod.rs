use std::{fs, path::Path, time::Duration};

use contracts::{AppErrorV1, error_codes};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;

const CURRENT_SCHEMA: i64 = 4;

#[derive(Debug)]
pub struct Storage {
    connection: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, AppErrorV1> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(storage_error)?;
        }
        let existed = path.exists();
        if existed && path.metadata().map_err(storage_error)?.len() == 0 {
            return Err(AppErrorV1::new(
                error_codes::STORAGE_FAILED,
                "User data appears damaged. The original database was preserved; restore a backup or export diagnostics.",
                false,
            )
            .with_diagnostics("Existing user database was empty."));
        }
        let mut connection = Connection::open(path).map_err(storage_error)?;
        configure(&connection)?;
        if existed {
            verify_integrity(&connection)?;
        }
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn in_memory() -> Result<Self, AppErrorV1> {
        let mut connection = Connection::open_in_memory().map_err(storage_error)?;
        configure(&connection)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock()
    }

    pub fn backup_to(&self, destination: &Path) -> Result<(), AppErrorV1> {
        if destination.exists() {
            return Err(storage_error(
                "The recovery backup destination already exists.",
            ));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(storage_error)?;
        }
        let destination_text = destination
            .to_str()
            .ok_or_else(|| storage_error("Backup destination was not valid Unicode."))?;
        self.connection()
            .execute("VACUUM INTO ?1", [destination_text])
            .map_err(storage_error)?;
        let backup = Connection::open(destination).map_err(storage_error)?;
        verify_integrity(&backup)
    }
}

fn configure(connection: &Connection) -> Result<(), AppErrorV1> {
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(storage_error)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
        )
        .map_err(storage_error)
}

fn verify_integrity(connection: &Connection) -> Result<(), AppErrorV1> {
    let result: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(storage_error)?;
    if result != "ok" {
        return Err(AppErrorV1::new(
            error_codes::STORAGE_FAILED,
            "User data appears damaged. The original database was preserved; restore a backup or export diagnostics.",
            false,
        ).with_diagnostics(result));
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), AppErrorV1> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version > CURRENT_SCHEMA {
        return Err(storage_error(
            "User database was created by a newer application version.",
        ));
    }
    if version == 0 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(include_str!("../../migrations/001_initial.sql"))
            .map_err(storage_error)?;
        transaction
            .pragma_update(None, "user_version", 1)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version == 1 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(include_str!("../../migrations/002_durable_workflows.sql"))
            .map_err(storage_error)?;
        transaction
            .pragma_update(None, "user_version", 2)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version == 2 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(include_str!("../../migrations/003_publish_reliability.sql"))
            .map_err(storage_error)?;
        transaction
            .pragma_update(None, "user_version", 3)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version == 3 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(include_str!(
                "../../migrations/004_translation_subtitles.sql"
            ))
            .map_err(storage_error)?;
        transaction
            .pragma_update(None, "user_version", CURRENT_SCHEMA)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    Ok(())
}

pub(crate) fn storage_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::STORAGE_FAILED,
        "User data could not be saved. Check disk space and application data permissions.",
        true,
    )
    .with_diagnostics(error.to_string())
}

pub(crate) fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_transactional_and_repeatable() -> Result<(), AppErrorV1> {
        let storage = Storage::in_memory()?;
        let version: i64 = storage
            .connection()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        assert_eq!(version, CURRENT_SCHEMA);
        Ok(())
    }

    #[test]
    fn upgrades_existing_databases_through_translation_schema() -> Result<(), AppErrorV1> {
        for starting_version in [1_i64, 2_i64, 3_i64] {
            let temporary = tempfile::tempdir().map_err(storage_error)?;
            let database = temporary
                .path()
                .join(format!("upgrade-v{starting_version}.sqlite3"));
            let connection = Connection::open(&database).map_err(storage_error)?;
            configure(&connection)?;
            connection
                .execute_batch(include_str!("../../migrations/001_initial.sql"))
                .map_err(storage_error)?;
            connection
                .pragma_update(None, "user_version", 1)
                .map_err(storage_error)?;
            if starting_version == 2 {
                connection
                    .execute_batch(include_str!("../../migrations/002_durable_workflows.sql"))
                    .map_err(storage_error)?;
                connection
                    .pragma_update(None, "user_version", 2)
                    .map_err(storage_error)?;
            }
            if starting_version == 3 {
                connection
                    .execute_batch(include_str!("../../migrations/002_durable_workflows.sql"))
                    .map_err(storage_error)?;
                connection
                    .execute_batch(include_str!("../../migrations/003_publish_reliability.sql"))
                    .map_err(storage_error)?;
                connection
                    .pragma_update(None, "user_version", 3)
                    .map_err(storage_error)?;
            }
            drop(connection);

            let storage = Storage::open(&database)?;
            let version: i64 = storage
                .connection()
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(storage_error)?;
            assert_eq!(version, CURRENT_SCHEMA);
            storage
                .connection()
                .query_row("SELECT COUNT(*) FROM publish_claims", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(storage_error)?;
            storage
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM translation_subtitle_bindings",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(storage_error)?;
        }
        Ok(())
    }

    #[test]
    fn backup_is_integral_and_never_overwrites() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(storage_error)?;
        let database = temporary.path().join("user.sqlite3");
        let backup = temporary.path().join("backup.sqlite3");
        let storage = Storage::open(&database)?;
        storage.backup_to(&backup)?;
        assert!(backup.is_file());
        assert!(storage.backup_to(&backup).is_err());
        Ok(())
    }

    #[test]
    fn corrupt_database_is_preserved() -> Result<(), AppErrorV1> {
        let temporary = tempfile::tempdir().map_err(storage_error)?;
        let database = temporary.path().join("corrupt.sqlite3");
        let original = b"this is not sqlite";
        fs::write(&database, original).map_err(storage_error)?;
        assert!(Storage::open(&database).is_err());
        assert_eq!(fs::read(&database).map_err(storage_error)?, original);
        Ok(())
    }
}
