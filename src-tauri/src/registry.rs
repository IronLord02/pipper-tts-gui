//! Persistent library registry (REQ-LIB-2, REQ-PER-1, REQ-LIB-5).
//!
//! The registry is a JSON file (`library.json`) stored next to the models
//! directory (see `paths.rs`). It records every installed model so the app can
//! detect existing installs on startup and never re-downloads them (REQ-LIB-1).
//!
//! Robustness contract:
//! - Writes are atomic: the file is written to a sibling temp file and renamed
//!   over the target (rename replaces on Windows via MOVEFILE_REPLACE_EXISTING).
//! - A missing file loads as an empty registry.
//! - A corrupt file is rebuilt as an empty registry instead of failing startup
//!   (REQ-PER-1); the corrupt file is left untouched until the next save.
//! - Destructive operations back up the registry file first (REQ-LIB-5):
//!   `remove_with_backup` copies `library.json` to `library.json.bak` before
//!   mutating and re-saving.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One installed model record (REQ-LIB-2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistryEntry {
    pub id: String,
    pub path: PathBuf,
    pub md5: String,
    pub version: String,
    pub speaker_count: u32,
}

/// The persisted library registry, keyed by model id (REQ-PER-1).
///
/// `#[serde(transparent)]` flattens the map so `library.json` is a plain JSON
/// object of `{id: {path, md5, version, speaker_count}}` entries. A `BTreeMap`
/// keeps entries deterministically sorted on disk so diffs stay stable across
/// saves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Registry {
    entries: BTreeMap<String, RegistryEntry>,
}

impl Registry {
    /// Load from `path`. Missing, unreadable, or corrupt files produce an empty
    /// registry (rebuild-empty policy).
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Atomic save: write a sibling temp file, then rename over the target.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path
            .parent()
            .ok_or_else(|| format!("registry path {} has no parent", path.display()))?;
        fs::create_dir_all(dir)
            .map_err(|err| format!("create {}: {err}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|err| format!("serialize registry: {err}"))?;
        fs::write(&tmp, json)
            .map_err(|err| format!("write {}: {err}", tmp.display()))?;
        fs::rename(&tmp, path)
            .map_err(|err| format!("rename {} -> {}: {err}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Whether a model id is recorded.
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// The recorded entry for a model id, if any.
    pub fn get(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.get(id)
    }

    /// All recorded model ids, sorted.
    pub fn ids(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Number of recorded models.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry holds no models.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or replace an entry; returns the previous entry, if any.
    /// Callers must `save` to persist (save-on-change contract).
    pub fn insert(&mut self, entry: RegistryEntry) -> Option<RegistryEntry> {
        self.entries.insert(entry.id.clone(), entry)
    }

    /// Remove an entry in memory; returns it if present. Callers must `save`.
    pub fn remove(&mut self, id: &str) -> Option<RegistryEntry> {
        self.entries.remove(id)
    }

    /// Back up the registry file to `library.json.bak` before a destructive
    /// operation (REQ-LIB-5). A missing source file is an error so callers
    /// never silently skip the backup.
    pub fn backup(&self, path: &Path) -> Result<(), String> {
        let bak = path.with_extension("json.bak");
        fs::copy(path, &bak)
            .map_err(|err| format!("backup {} -> {}: {err}", path.display(), bak.display()))?;
        Ok(())
    }

    /// Remove an entry with the required backup-before-delete dance
    /// (REQ-LIB-5): back up the registry file first, then remove and save.
    pub fn remove_with_backup(&mut self, id: &str, path: &Path) -> Result<Option<RegistryEntry>, String> {
        if !self.contains(id) {
            return Ok(None);
        }
        self.backup(path)?;
        let removed = self.remove(id);
        self.save(path)?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> RegistryEntry {
        RegistryEntry {
            id: id.to_string(),
            path: PathBuf::from("C:/models").join(format!("{id}.onnx")),
            md5: "a3f1c9e2b4d5467f8a0c1d2e3f4b5a67".to_string(),
            version: "1.0".to_string(),
            speaker_count: 1,
        }
    }

    #[test]
    fn roundtrip_persists_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("library.json");
        let mut registry = Registry::default();
        registry.insert(entry("en_US-lessac-medium"));
        registry.insert(entry("en_GB-alan-medium"));
        registry.save(&path).expect("save");

        let loaded = Registry::load(&path);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains("en_US-lessac-medium"));
        assert_eq!(loaded.get("en_US-ryan-high"), None);
        let lessac = loaded.get("en_US-lessac-medium").expect("entry");
        assert_eq!(lessac.md5, "a3f1c9e2b4d5467f8a0c1d2e3f4b5a67");
        assert_eq!(lessac.speaker_count, 1);
        assert_eq!(
            lessac.path,
            PathBuf::from("C:/models").join("en_US-lessac-medium.onnx")
        );
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let registry = Registry::load(&tmp.path().join("library.json"));
        assert!(registry.is_empty());
    }

    #[test]
    fn corrupt_file_rebuilds_empty() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("library.json");
        std::fs::write(&path, b"this is not json {").expect("write corrupt file");
        let registry = Registry::load(&path);
        assert!(registry.is_empty());
    }

    #[test]
    fn delete_backs_up_registry_first() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("library.json");
        let mut registry = Registry::default();
        registry.insert(entry("en_US-lessac-medium"));
        registry.insert(entry("en_GB-alan-medium"));
        registry.save(&path).expect("save");

        let removed = registry
            .remove_with_backup("en_US-lessac-medium", &path)
            .expect("remove with backup");
        assert_eq!(removed.expect("removed entry").id, "en_US-lessac-medium");

        // The backup still holds the pre-delete state (REQ-LIB-5).
        let backup = Registry::load(&path.with_extension("json.bak"));
        assert!(backup.contains("en_US-lessac-medium"));
        assert_eq!(backup.len(), 2);

        // The live registry no longer holds the deleted entry.
        let live = Registry::load(&path);
        assert_eq!(live.len(), 1);
        assert!(!live.contains("en_US-lessac-medium"));
        assert!(live.contains("en_GB-alan-medium"));
    }

    #[test]
    fn remove_missing_entry_is_noop_without_backup() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("library.json");
        let mut registry = Registry::default();
        let removed = registry
            .remove_with_backup("nope", &path)
            .expect("missing id is a no-op");
        assert_eq!(removed, None);
        assert!(!path.with_extension("json.bak").exists());
    }
}