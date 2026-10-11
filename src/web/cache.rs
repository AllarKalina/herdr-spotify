//! What Spotify last told us, kept on disk so a new daemon need not ask again.

use crate::paths::Paths;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs;

/// A cached answer with what is needed to judge whether it still holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Entry<T> {
    pub fetched_at_ms: u64,
    /// The playlist snapshot this was fetched for; empty when it has none.
    #[serde(default)]
    pub snapshot_id: String,
    pub value: T,
}

/// Reads an entry. Anything unreadable counts as not cached.
pub(super) fn read<T: DeserializeOwned>(paths: &Paths, name: &str) -> Option<Entry<T>> {
    let text = fs::read_to_string(paths.cache().join(file_name(name)?)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes an entry in one step. A failure is logged, never fatal: the cache only saves calls.
pub(super) fn write<T: Serialize>(paths: &Paths, name: &str, entry: &Entry<T>) {
    let outcome = (|| -> crate::Result<()> {
        let dir = paths.cache();
        fs::create_dir_all(&dir)?;
        let file = file_name(name).ok_or("unusable cache name")?;
        let partial = dir.join(format!("{file}.partial"));
        fs::write(&partial, serde_json::to_vec(entry)?)?;
        fs::rename(&partial, dir.join(file))?;
        Ok(())
    })();
    if let Err(error) = outcome {
        log::warn!("cache not written for {name}: {error}");
    }
}

/// Names come from Spotify IDs; anything but letters, digits and dashes is refused.
fn file_name(name: &str) -> Option<String> {
    let plain = !name.is_empty()
        && name.len() <= 80
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    plain.then(|| format!("{name}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn an_entry_round_trips_and_odd_names_are_refused() {
        let dir = PathBuf::from(format!("/tmp/hs-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = Paths::at(dir.clone()).expect("paths");
        let entry = Entry {
            fetched_at_ms: 7,
            snapshot_id: "s1".into(),
            value: vec![1, 2, 3],
        };

        write(&paths, "tracks-abc123", &entry);
        let back: Entry<Vec<i32>> = read(&paths, "tracks-abc123").expect("cached");
        assert_eq!(
            (back.fetched_at_ms, back.snapshot_id.as_str(), back.value),
            (7, "s1", vec![1, 2, 3])
        );

        write(&paths, "../escape", &entry);
        assert!(read::<Vec<i32>>(&paths, "../escape").is_none());
        assert!(!dir.join("escape.json").exists());
        assert!(read::<Vec<i32>>(&paths, "missing").is_none());
        fs::remove_dir_all(&dir).expect("clean");
    }
}
