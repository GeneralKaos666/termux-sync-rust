use std::cmp::Reverse;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub kind: String,
    pub name: String,
    pub status: String,
    pub size: u64,
    pub duration_ms: u64,
    pub verified: Option<bool>,
    pub at: u64,
}

pub struct HistoryStore;

impl HistoryStore {
    pub fn load(path: &Path) -> Vec<HistoryEntry> {
        let Ok(text) = fs::read_to_string(path) else {
            return Vec::new();
        };
        let mut entries: Vec<HistoryEntry> = serde_json::from_str(&text).unwrap_or_default();
        entries.sort_by_key(|entry| Reverse(entry.at));
        entries.truncate(20);
        entries
    }

    pub fn save(path: &Path, entries: &[HistoryEntry]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut entries = entries.to_vec();
        entries.sort_by_key(|entry| Reverse(entry.at));
        entries.truncate(20);
        let temp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&entries)
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::write(&temp, text)?;
        fs::rename(temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(n: usize) -> HistoryEntry {
        HistoryEntry {
            kind: "backup".to_string(),
            name: format!("entry-{n}"),
            status: "succeeded".to_string(),
            size: n as u64,
            duration_ms: n as u64,
            verified: Some(true),
            at: n as u64,
        }
    }

    #[test]
    fn history_keeps_latest_twenty() {
        let dir = std::env::temp_dir().join(format!("ts-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let entries: Vec<_> = (0..25).map(sample_entry).collect();
        HistoryStore::save(&path, &entries).unwrap();
        let loaded = HistoryStore::load(&path);
        assert_eq!(loaded.len(), 20);
        assert_eq!(loaded[0].name, "entry-24");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn corrupt_history_loads_empty() {
        let dir = std::env::temp_dir().join(format!("ts-history-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(HistoryStore::load(&path).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
