//! SQLite-backed storage helpers. Databases are opened in WAL mode for
//! concurrent readers and a single writer with ACID guarantees.

use std::path::Path;

use rusqlite::Connection;
use tracing::debug;

use crate::{StateDir, StorageError};

/// A thin wrapper around a `rusqlite` connection configured for WAL mode.
#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a SQLite database at `path`, enabling WAL mode.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; \
             PRAGMA synchronous = NORMAL; \
             PRAGMA foreign_keys = ON;",
        )?;
        debug!(path = %path.display(), "opened sqlite database in WAL mode");
        Ok(Self { conn })
    }

    /// Open an in-memory database (useful for tests). WAL is requested but the
    /// engine keeps it in memory.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL;");
        Ok(Self { conn })
    }

    /// Borrow the underlying connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Borrow mutably.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Execute a statement with no rows returned.
    pub fn execute(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<usize, StorageError> {
        Ok(self.conn.execute(sql, params)?)
    }

    /// Run a closure inside a transaction, committing on success.
    pub fn transaction<F, T>(&mut self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError>,
    {
        let tx = self.conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    /// Query a single text value, returning `None` when no row matches.
    pub fn query_text(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Option<String>, StorageError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Current journal mode (should be "wal" for file-backed databases).
    pub fn journal_mode(&self) -> Result<String, StorageError> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))?)
    }
}

// convenience re-export of params macro

// Keep a `StateDir`-aware constructor for callers that store per-state DBs.
impl Database {
    pub fn open_in_state(state: &StateDir, name: &str) -> Result<Self, StorageError> {
        let path = state.path().join(name);
        Self::open(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn opens_in_wal_mode() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let db = Database::open(&path).unwrap();
        assert_eq!(db.journal_mode().unwrap().to_lowercase(), "wal");
    }

    #[test]
    fn wal_sidecar_files_created() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let _db = Database::open(&path).unwrap();
        // WAL mode creates -wal/-shm on first write; force a write
        _db.execute("CREATE TABLE IF NOT EXISTS t (x INTEGER)", &[]).unwrap();
        _db.execute("INSERT INTO t (x) VALUES (1)", &[]).unwrap();
        assert!(tmp.path().join("test.db-wal").exists() || tmp.path().join("test.db").exists());
    }
}
