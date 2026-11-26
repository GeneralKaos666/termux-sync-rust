use std::cmp::Reverse;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One completed backup or restore kept for the history overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Operation kind, `backup` or `restore`.
    pub kind: String,
    /// Backup name the operation targeted.
    pub name: String,
    /// Terminal status string, for example `succeeded` or `failed`.
    pub status: String,
    /// Bytes transferred, zero when unknown.
    pub size: u64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Verification result, `None` when verification did not run.
    pub verified: Option<bool>,
    /// Unix timestamp when the operation finished.
    pub at: u64,
}

/// Loads and saves [`HistoryEntry`] lists in a private JSON file.
pub struct HistoryStore;

impl HistoryStore {
    /// Reads at most the 20 newest entries; a missing file is an empty list.
    pub fn load(path: &Path) -> io::Result<Vec<HistoryEntry>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut entries: Vec<HistoryEntry> = serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        entries.sort_by_key(|entry| Reverse(entry.at));
        entries.truncate(20);
        Ok(entries)
    }

    /// Writes at most the 20 newest entries atomically with mode `0600`.
    ///
    /// Entries are sorted in place, newest first.
    pub fn save(path: &Path, entries: &mut [HistoryEntry]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        entries.sort_by_key(|entry| Reverse(entry.at));
        let keep = entries.len().min(20);
        let temp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&entries[..keep])
            .map_err(|error| io::Error::other(error.to_string()))?;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut handle = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        handle.write_all(text.as_bytes())?;
        fs::rename(temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.json");
        HistoryStore::save(&path, &mut []).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

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
        let mut entries: Vec<_> = (0..25).map(sample_entry).collect();
        HistoryStore::save(&path, &mut entries).unwrap();
        let loaded = HistoryStore::load(&path).unwrap();
        assert_eq!(loaded.len(), 20);
        assert_eq!(loaded[0].name, "entry-24");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn corrupt_history_is_reported() {
        let dir = std::env::temp_dir().join(format!("ts-history-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        std::fs::write(&path, "{not json").unwrap();
        let error = HistoryStore::load(&path).expect_err("corrupt history must be reported");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(dir).ok();
    }
}
