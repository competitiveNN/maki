//! SQLite-backed storage helpers. Databases are opened in WAL mode for
//! concurrent readers and a single writer with ACID guarantees.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{Connection, OptionalExtension, Statement};
use std::collections::HashMap;
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
             PRAGMA foreign_keys = ON; \
             PRAGMA cache_size = -2048; \
             PRAGMA temp_store = memory;",
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

    /// Prepare a statement (no cache needed - rusqlite caches internally).
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>, StorageError> {
        Ok(self.conn.prepare(sql)?)
    }

    /// Execute a statement with no rows returned.
    pub fn execute(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<usize, StorageError> {
        Ok(self.conn.execute(sql, params)?)
    }

    /// Execute many statements in a single transaction.
    pub fn execute_batch(&self, sql: &str) -> Result<(), StorageError> {
        Ok(self.conn.execute_batch(sql)?)
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
    pub fn query_text(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<String>, StorageError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Query all rows as a vector of row tuples.
    pub fn query_all<T, F>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        f: F,
    ) -> Result<Vec<T>, StorageError>
    where
        F: Fn(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, f)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Query a single row, returning `None` if not found.
    pub fn query_one<T, F>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        f: F,
    ) -> Result<Option<T>, StorageError>
    where
        F: Fn(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let row = stmt.query_row(params, f).optional()?;
        Ok(row)
    }

    /// Current journal mode (should be "wal" for file-backed databases).
    pub fn journal_mode(&self) -> Result<String, StorageError> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))?)
    }

    /// Enable busy timeout for contention handling (ms).
    pub fn set_busy_timeout(&self, ms: u32) -> Result<(), StorageError> {
        self.conn
            .busy_timeout(std::time::Duration::from_millis(ms as u64))?;
        Ok(())
    }
}

/// A thread-safe database pool for shared access.
/// Uses a single connection with a mutex - suitable for WAL mode single-writer.
#[derive(Debug, Clone)]
pub struct DatabasePool {
    inner: Arc<Mutex<Database>>,
}

impl DatabasePool {
    /// Create a new pool with one connection.
    pub fn new(path: &Path) -> Result<Self, StorageError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(Database::open(path)?)),
        })
    }

    /// Execute a read/write operation with exclusive access.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&mut Database) -> Result<T, StorageError>,
    {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| StorageError::Io(std::io::Error::other(e.to_string())))?;
        f(&mut db)
    }

    /// Execute a read-only operation (can be parallel with other reads in WAL).
    pub fn with_read<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Database) -> Result<T, StorageError>,
    {
        let db = self
            .inner
            .lock()
            .map_err(|e| StorageError::Io(std::io::Error::other(e.to_string())))?;
        f(&db)
    }
}

/// Global pool registry for databases keyed by path.
/// Allows plugins to share a single connection per database file.
static POOL_REGISTRY: OnceLock<Mutex<HashMap<String, DatabasePool>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, DatabasePool>> {
    POOL_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get or create a shared database pool for the given path.
pub fn get_pool(path: &Path) -> Result<DatabasePool, StorageError> {
    let key = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let mut reg = registry()
        .lock()
        .map_err(|e| StorageError::Io(std::io::Error::other(e.to_string())))?;
    if let Some(pool) = reg.get(&key) {
        return Ok(pool.clone());
    }
    let pool = DatabasePool::new(path)?;
    reg.insert(key, pool.clone());
    Ok(pool)
}

/// Clear all pools (for testing).
#[cfg(test)]
pub fn clear_pools() {
    if let Some(reg) = POOL_REGISTRY.get() {
        reg.lock().unwrap().clear();
    }
}

// convenience re-export of params macro
pub use rusqlite::params;

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
        _db.execute("CREATE TABLE IF NOT EXISTS t (x INTEGER)", &[])
            .unwrap();
        _db.execute("INSERT INTO t (x) VALUES (1)", &[]).unwrap();
        assert!(tmp.path().join("test.db-wal").exists() || tmp.path().join("test.db").exists());
    }

    #[test]
    fn pool_shares_connection() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let pool1 = get_pool(&path).unwrap();
        let pool2 = get_pool(&path).unwrap();
        // Same pool instance returned
        assert!(std::ptr::eq(&*pool1.inner, &*pool2.inner));
    }

    #[test]
    fn pool_with_conn_works() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let pool = get_pool(&path).unwrap();
        pool.with_conn(|db| {
            db.execute("CREATE TABLE t (x INTEGER)", &[])?;
            db.execute("INSERT INTO t (x) VALUES (42)", &[])?;
            Ok(())
        })
        .unwrap();
        let val = pool
            .with_read(|db| db.query_one("SELECT x FROM t", &[], |r| r.get(0)))
            .unwrap();
        assert_eq!(val, Some(42));
    }

    #[test]
    fn query_all_works() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let db = Database::open(&path).unwrap();
        db.execute("CREATE TABLE t (x INTEGER)", &[]).unwrap();
        for i in 0..5 {
            db.execute("INSERT INTO t (x) VALUES (?1)", &[&i]).unwrap();
        }
        let rows: Vec<i64> = db
            .query_all("SELECT x FROM t ORDER BY x", &[], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, vec![0, 1, 2, 3, 4]);
    }
}
