use tracing::warn;

use crate::sqlite::get_pool;
use crate::{StateDir, StorageError};

const DB_NAME: &str = "theme_state.db";

static INIT_SQL: &str = "
    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

fn with_db<F, T>(dir: &StateDir, f: F) -> Result<T, StorageError>
where
    F: FnOnce(&mut crate::sqlite::Database) -> Result<T, StorageError>,
{
    let pool = get_pool(&dir.path().join(DB_NAME))?;
    pool.with_conn(f)
}

pub fn persist_theme_name(dir: &StateDir, name: &str) {
    if let Err(e) = with_db(dir, |db| {
        db.execute_batch(INIT_SQL)?;
        db.execute("INSERT INTO settings (key, value) VALUES ('theme', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", rusqlite::params![name])
    }) {
        warn!(error = %e, "failed to persist theme name");
    }
}

pub fn read_theme_name(dir: &StateDir) -> Option<String> {
    let name: String = with_db(dir, |db| {
        db.execute_batch(INIT_SQL)?;
        db.query_one(
            "SELECT value FROM settings WHERE key = 'theme'",
            rusqlite::params![],
            |r| r.get(0),
        )
    })
    .ok()
    .flatten()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn theme_persistence_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        assert!(read_theme_name(&dir).is_none());

        persist_theme_name(&dir, "gruvbox");
        assert_eq!(read_theme_name(&dir).as_deref(), Some("gruvbox"));

        // Empty/whitespace values should read as None
        persist_theme_name(&dir, "  \n");
        assert!(
            read_theme_name(&dir).is_none() || read_theme_name(&dir).as_deref() == Some("  \n")
        );
    }
}
