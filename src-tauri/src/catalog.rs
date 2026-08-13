//! Language-first voice catalog (REQ-CAT-1, REQ-CAT-2, REQ-CAT-3, REQ-CAT-6).
//!
//! The catalog is a dev-time curated snapshot of the rhasspy/piper-voices
//! repository: `resources/voices-snapshot.json` holds the voice entries and
//! `resources/gender-map.json` the curated voice-to-gender mapping. Voices
//! absent from the gender map render as `unknown` (REQ-CAT-3). Both files are
//! embedded at compile time via `include_str!`, so parsing is infallible by
//! construction. A full regeneration script for the snapshot is a later
//! concern; this slice ships a curated subset.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Loaded catalog matching the design snapshot schema (design D6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Catalog {
    pub snapshot_version: String,
    pub source: String,
    pub voices: Vec<Voice>,
    pub gender_map: HashMap<String, Gender>,
}

/// A single voice entry from the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Voice {
    pub id: String,
    pub language: String,
    pub quality: String,
    pub num_speakers: u32,
    pub files: VoiceFiles,
}

/// Files that make up a voice's model artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VoiceFiles {
    pub model: ModelFile,
}

/// Download metadata for one model file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModelFile {
    pub url: String,
    pub size_bytes: u64,
    pub md5_digest: String,
}

/// Voice gender as rendered to the UI. `Unknown` covers multi-speaker voices
/// and voices absent from the curated gender map (REQ-CAT-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Unknown,
}

/// Query result: a voice together with its resolved gender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct VoiceInfo {
    pub id: String,
    pub language: String,
    pub quality: String,
    pub num_speakers: u32,
    pub gender: Gender,
}

/// Wire shape of the embedded snapshot file (no gender map inside it).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct Snapshot {
    snapshot_version: String,
    source: String,
    voices: Vec<Voice>,
}

impl Catalog {
    /// Load the embedded snapshot and gender map.
    ///
    /// Both files are compiled in via `include_str!`; parsing is infallible by
    /// construction (the data is committed with the crate).
    pub fn load() -> Self {
        let snapshot: Snapshot =
            serde_json::from_str(include_str!("../resources/voices-snapshot.json"))
                .expect("embedded voices snapshot must be valid JSON");
        let gender_map: HashMap<String, Gender> =
            serde_json::from_str(include_str!("../resources/gender-map.json"))
                .expect("embedded gender map must be valid JSON");
        Self {
            snapshot_version: snapshot.snapshot_version,
            source: snapshot.source,
            voices: snapshot.voices,
            gender_map,
        }
    }

    /// All languages present in the snapshot, sorted and deduplicated.
    pub fn languages(&self) -> Vec<String> {
        let mut languages: Vec<String> = self
            .voices
            .iter()
            .map(|voice| voice.language.clone())
            .collect();
        languages.sort();
        languages.dedup();
        languages
    }

    /// Voices for one language (language-first query, REQ-CAT-1).
    pub fn voices_for_language(&self, language: &str) -> Vec<VoiceInfo> {
        self.voices
            .iter()
            .filter(|voice| voice.language == language)
            .map(|voice| self.to_voice_info(voice))
            .collect()
    }

    /// Look up a single voice by id.
    pub fn voice(&self, id: &str) -> Option<VoiceInfo> {
        self.voices
            .iter()
            .find(|voice| voice.id == id)
            .map(|voice| self.to_voice_info(voice))
    }

    /// Resolved gender for a voice id; `Unknown` when absent from the map.
    pub fn gender_of(&self, id: &str) -> Gender {
        self.gender_map.get(id).copied().unwrap_or(Gender::Unknown)
    }

    fn to_voice_info(&self, voice: &Voice) -> VoiceInfo {
        VoiceInfo {
            id: voice.id.clone(),
            language: voice.language.clone(),
            quality: voice.quality.clone(),
            num_speakers: voice.num_speakers,
            gender: self.gender_of(&voice.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn catalog() -> Catalog {
        Catalog::load()
    }

    #[test]
    fn snapshot_has_minimum_scope() {
        let catalog = catalog();
        assert!(catalog.voices.len() >= 10, "curated subset should hold 10-15 voices");
        assert!(catalog.languages().len() >= 5, "curated subset should cover >= 5 languages");
        assert!(!catalog.snapshot_version.is_empty());
        assert!(!catalog.source.is_empty());
    }

    #[test]
    fn languages_are_sorted_and_deduplicated() {
        let languages = catalog().languages();
        let mut sorted = languages.clone();
        sorted.sort();
        assert_eq!(languages, sorted);
        let unique: HashSet<String> = languages.iter().cloned().collect();
        assert_eq!(unique.len(), languages.len());
    }

    #[test]
    fn language_filter_returns_only_that_language() {
        let catalog = catalog();
        for language in ["es_ES", "de_DE", "zh_CN"] {
            let voices = catalog.voices_for_language(language);
            assert!(!voices.is_empty(), "language {language} should have voices");
            assert!(voices.iter().all(|voice| voice.language == language));
        }
    }

    #[test]
    fn voice_absent_from_gender_map_renders_unknown() {
        let catalog = catalog();
        let libritts = catalog.voice("en_US-libritts_r-medium").expect("voice exists");
        assert_eq!(libritts.gender, Gender::Unknown);
        let faber = catalog.voice("pt_BR-faber-medium").expect("voice exists");
        assert_eq!(faber.gender, Gender::Unknown);
    }

    #[test]
    fn voice_with_gender_map_entry_reports_gender() {
        let catalog = catalog();
        let lessac = catalog.voice("en_US-lessac-medium").expect("voice exists");
        assert_eq!(lessac.gender, Gender::Female);
        let thorsten = catalog.voice("de_DE-thorsten-medium").expect("voice exists");
        assert_eq!(thorsten.gender, Gender::Male);
    }
}