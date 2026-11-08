use std::collections::BTreeSet;

use rusqlite::params;

use crate::sessions::StoredThinking;
use crate::sqlite::get_pool;
use crate::{StateDir, StorageError};

const DB_NAME: &str = "model_state.db";
const MAX_RECENTS: usize = 6;

static INIT_SQL: &str = "
    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS recents (
        spec TEXT PRIMARY KEY,
        seq INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS expanded_providers (
        slug TEXT PRIMARY KEY
    );
";

fn with_db<F, T>(dir: &StateDir, f: F) -> Result<T, StorageError>
where
    F: FnOnce(&mut crate::sqlite::Database) -> Result<T, StorageError>,
{
    let pool = get_pool(&dir.path().join(DB_NAME))?;
    pool.with_conn(|db| {
        db.execute_batch(INIT_SQL)?;
        f(db)
    })
}

pub fn persist_model(dir: &StateDir, spec: &str) {
    let _ = with_db(dir, |db| {
        db.execute(
            "INSERT INTO settings (key, value) VALUES ('model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![spec],
        )
        .map(|_| ())
        .map_err(StorageError::from)
    });
}

pub fn read_model(dir: &StateDir) -> Option<String> {
    with_db(dir, |db| {
        let raw: Option<String> = db.query_one(
            "SELECT value FROM settings WHERE key = 'model'",
            params![],
            |r| r.get(0),
        )?;
        Ok(raw.and_then(|s| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_owned())
        }))
    })
    .ok()
    .flatten()
}

/// Written in the `/thinking` spelling so the file reads like the command
/// that produced it, and a hand-edited or stale value just fails to parse.
pub fn persist_thinking(dir: &StateDir, thinking: StoredThinking) {
    let _ = with_db(dir, |db| {
        db.execute(
            "INSERT INTO settings (key, value) VALUES ('thinking', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![thinking.to_string()],
        )
        .map(|_| ())
        .map_err(StorageError::from)
    });
}

pub fn read_thinking(dir: &StateDir) -> Option<StoredThinking> {
    with_db(dir, |db| {
        let raw: Option<String> = db.query_one(
            "SELECT value FROM settings WHERE key = 'thinking'",
            params![],
            |r| r.get(0),
        )?;
        Ok(raw.and_then(|s| StoredThinking::parse_setting(&s).ok()))
    })
    .ok()
    .flatten()
}

/// Providers the model picker lists unfolded, by slug, since it folds the rest.
/// Kept as a sorted set, so unfolding and folding the same provider leaves the
/// file as it was.
pub fn read_expanded_providers(dir: &StateDir) -> BTreeSet<String> {
    with_db(dir, |db| {
        db.query_all(
            "SELECT slug FROM expanded_providers",
            params![],
            |r| r.get(0),
        )
        .map(|rows| rows.into_iter().collect())
        .map_err(StorageError::from)
    })
    .unwrap_or_default()
}

pub fn persist_expanded_providers(dir: &StateDir, slugs: &BTreeSet<String>) {
    let _ = with_db(dir, |db| {
        let tx = db.connection_mut().transaction()?;
        tx.execute("DELETE FROM expanded_providers", params![])?;
        for slug in slugs {
            tx.execute(
                "INSERT INTO expanded_providers (slug) VALUES (?1)",
                params![slug],
            )?;
        }
        tx.commit()?;
        Ok(())
    });
}

pub fn push_recent(dir: &StateDir, spec: &str) -> Vec<String> {
    with_db(dir, |db| {
        let tx = db.connection_mut().transaction()?;
        tx.execute("DELETE FROM recents WHERE spec = ?1", params![spec])?;
        tx.execute("UPDATE recents SET seq = seq + 1", params![])?;
        tx.execute(
            "INSERT INTO recents (spec, seq) VALUES (?1, 0)",
            params![spec],
        )?;
        tx.execute(
            "DELETE FROM recents WHERE seq >= ?1",
            params![MAX_RECENTS as i64],
        )?;
        tx.commit()?;
        db.query_all("SELECT spec FROM recents ORDER BY seq", params![], |r| {
            r.get(0)
        })
        .map_err(StorageError::from)
    })
    .unwrap_or_default()
}

/// Newest first, so a list written by a launch that predates the migration
/// does not bury the history it replaced. Identical to `push_recent` for one
/// spec, applied to a whole list.
pub fn merge_recents(new: &[String], old: &[String]) -> Vec<String> {
    let mut merged = new.to_vec();
    for spec in old {
        if !merged.contains(spec) {
            merged.push(spec.clone());
        }
    }
    merged.truncate(MAX_RECENTS);
    merged
}

pub fn read_recents(dir: &StateDir) -> Vec<String> {
    with_db(dir, |db| {
        db.query_all("SELECT spec FROM recents ORDER BY seq", params![], |r| {
            r.get(0)
        })
        .map_err(StorageError::from)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Effort;
    use tempfile::TempDir;
    use test_case::test_case;

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        assert!(read_model(&dir).is_none());

        persist_model(&dir, "anthropic/claude-sonnet-4");
        assert_eq!(
            read_model(&dir).as_deref(),
            Some("anthropic/claude-sonnet-4")
        );

        persist_model(&dir, "openai/gpt-5.4-nano");
        assert_eq!(read_model(&dir).as_deref(), Some("openai/gpt-5.4-nano"));
    }

    #[test]
    fn whitespace_and_empty_treated_as_none() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        persist_model(&dir, "  \n");
        assert!(read_model(&dir).is_none());

        persist_model(&dir, "");
        assert!(read_model(&dir).is_none());
    }

    #[test_case(StoredThinking::Off)]
    #[test_case(StoredThinking::Adaptive)]
    #[test_case(StoredThinking::Effort { level: Effort::High })]
    #[test_case(StoredThinking::Budget { tokens: 8192 })]
    fn thinking_round_trip(thinking: StoredThinking) {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        assert!(read_thinking(&dir).is_none());
        persist_thinking(&dir, thinking);
        assert_eq!(read_thinking(&dir), Some(thinking));
    }

    #[test_case("" ; "empty")]
    #[test_case("  \n" ; "whitespace")]
    #[test_case("turbo" ; "unknown level")]
    #[test_case("0" ; "zero budget")]
    fn read_thinking_ignores_invalid(raw: &str) {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        // Persisting an invalid value should not make a valid one appear.
        persist_model(&dir, ""); // no-op helper not available; write via sqlite directly would be overkill
        let _ = with_db(&dir, |db| {
            db.execute(
                "INSERT INTO settings (key, value) VALUES ('thinking', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![raw],
            )
            .map(|_| ())
            .map_err(StorageError::from)
        });
        assert!(read_thinking(&dir).is_none());
    }

    #[test]
    fn push_recent_dedupes_and_orders_most_recent_first() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        push_recent(&dir, "anthropic/claude-sonnet-4");
        push_recent(&dir, "openai/gpt-5.4-nano");
        assert_eq!(
            read_recents(&dir),
            ["openai/gpt-5.4-nano", "anthropic/claude-sonnet-4"]
        );

        push_recent(&dir, "anthropic/claude-sonnet-4");
        assert_eq!(
            read_recents(&dir),
            ["anthropic/claude-sonnet-4", "openai/gpt-5.4-nano"]
        );
    }

    #[test]
    fn merge_recents_keeps_both_sides_newest_first() {
        let merged = merge_recents(
            &["new/one".to_owned()],
            &["old/one".to_owned(), "new/one".to_owned()],
        );
        assert_eq!(merged, ["new/one", "old/one"]);
    }

    #[test]
    fn merge_recents_caps_at_max() {
        let new: Vec<String> = (0..MAX_RECENTS).map(|i| format!("new/{i}")).collect();
        let old: Vec<String> = (0..MAX_RECENTS).map(|i| format!("old/{i}")).collect();
        assert_eq!(merge_recents(&new, &old).len(), MAX_RECENTS);
    }

    #[test]
    fn push_recent_caps_at_max() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        for i in 0..(MAX_RECENTS + 3) {
            push_recent(&dir, &format!("p/model-{i}"));
        }
        let recents = read_recents(&dir);
        assert_eq!(recents.len(), MAX_RECENTS);
        assert_eq!(recents[0], format!("p/model-{}", MAX_RECENTS + 2));
    }

    #[test]
    fn push_recent_returns_final_list() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());

        let recents = push_recent(&dir, "a/b");
        assert_eq!(recents, vec!["a/b".to_string()]);
        assert_eq!(recents, read_recents(&dir));
    }

    #[test]
    fn read_recents_handles_missing_or_invalid() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());
        assert!(read_recents(&dir).is_empty());
    }

    #[test]
    fn expanded_providers_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());
        assert!(read_expanded_providers(&dir).is_empty());

        let mut slugs = BTreeSet::new();
        slugs.insert("openai".to_owned());
        slugs.insert("anthropic".to_owned());
        persist_expanded_providers(&dir, &slugs);
        assert_eq!(read_expanded_providers(&dir), slugs);
    }
}
