//! WisprSide Local Personal Memory & Aside Scratchpad (`SD-2`).
//!
//! Wraps `wispral_memory::MemoryStore` with Specta-typed DTOs and enforces
//! Constitution Principle XIV & `SI-3` fail-closed privacy invariants:
//! - `keep_audio` defaults to `false`;
//! - Corrupt/truncated `memory_privacy.json` fails closed to `KeepNothing` (never fails open);
//! - `KeepNothing` writes zero history/aside transcripts to disk or RAM;
//! - `SessionOnly` stores Aside notes strictly in process RAM (zero disk writes to `history.jsonl`);
//! - `BoundedDays` automatically purges expired history entries via `purge_expired_history`;
//! - Updating an existing snippet trigger replaces its expansion in place without shadowing.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use specta::Type;
use wispral_arabic::{NormalizationLevel, NormalizedMatchKey};
use wispral_memory::{
    HistoryRecord, MemoryPrivacyConfig, MemoryStore, RetentionPolicy, SnippetTemplate,
};

static MEMORY_ID_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_memory_id(prefix: &str, now_ms: u64) -> String {
    let seq = MEMORY_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now_ms}-{seq}")
}

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn session_aside_store() -> &'static Mutex<HashMap<PathBuf, Vec<AsideNoteDto>>> {
    static SESSION_ASIDES: OnceLock<Mutex<HashMap<PathBuf, Vec<AsideNoteDto>>>> = OnceLock::new();
    SESSION_ASIDES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRetentionMode {
    KeepNothing,
    SessionOnly,
    Bounded7Days,
    Bounded30Days,
    UserControlled,
}

impl MemoryRetentionMode {
    pub fn to_core(self) -> RetentionPolicy {
        match self {
            Self::KeepNothing => RetentionPolicy::KeepNothing,
            Self::SessionOnly => RetentionPolicy::SessionOnly,
            Self::Bounded7Days => RetentionPolicy::BoundedDays { days: 7 },
            Self::Bounded30Days => RetentionPolicy::BoundedDays { days: 30 },
            Self::UserControlled => RetentionPolicy::UserControlled,
        }
    }

    /// Maps `wispral_memory::RetentionPolicy` into WisprSide's UI presets
    /// (arbitrary `BoundedDays` values bucket into `<= 7` -> `Bounded7Days` and `> 7` -> `Bounded30Days`).
    pub fn from_core(policy: RetentionPolicy) -> Self {
        match policy {
            RetentionPolicy::KeepNothing => Self::KeepNothing,
            RetentionPolicy::SessionOnly => Self::SessionOnly,
            RetentionPolicy::BoundedDays { days } if days <= 7 => Self::Bounded7Days,
            RetentionPolicy::BoundedDays { .. } => Self::Bounded30Days,
            RetentionPolicy::UserControlled => Self::UserControlled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PersonalSnippetDto {
    pub id: String,
    pub trigger: String,
    pub expansion: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AsideNoteDto {
    pub id: String,
    pub text: String,
    pub app_name: Option<String>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PersonalMemorySnapshot {
    pub keep_audio: bool,
    pub retention_mode: MemoryRetentionMode,
    pub snippets: Vec<PersonalSnippetDto>,
    pub aside_notes: Vec<AsideNoteDto>,
}

fn privacy_config_path(wispral_home: &Path) -> PathBuf {
    wispral_home.join("memory_privacy.json")
}

fn fail_closed_privacy() -> MemoryPrivacyConfig {
    MemoryPrivacyConfig {
        keep_audio: false,
        retention: RetentionPolicy::KeepNothing,
    }
}

/// Loads `memory_privacy.json`. Returns `Default` (`keep_audio = false`, 7-day bounded)
/// ONLY when the file does not exist yet; fails closed (`KeepNothing`, `keep_audio = false`)
/// if an existing privacy file cannot be read or parsed (`SI-3`).
pub fn load_privacy_config(wispral_home: &Path) -> MemoryPrivacyConfig {
    let p = privacy_config_path(wispral_home);
    match std::fs::read_to_string(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => MemoryPrivacyConfig::default(),
        Ok(raw) => serde_json::from_str::<MemoryPrivacyConfig>(&raw)
            .unwrap_or_else(|_| fail_closed_privacy()),
        Err(_) => fail_closed_privacy(),
    }
}

/// Atomically writes `memory_privacy.json` (`tmp` + `sync_all` + `rename`) and enforces
/// retention transitions immediately.
pub fn save_privacy_config(wispral_home: &Path, cfg: &MemoryPrivacyConfig) -> Result<(), String> {
    std::fs::create_dir_all(wispral_home)
        .map_err(|e| format!("failed to create wispral_home: {e}"))?;
    let mut normalized = cfg.clone();
    if normalized.retention == RetentionPolicy::KeepNothing {
        normalized.keep_audio = false;
    }
    let json = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
    let target = privacy_config_path(wispral_home);
    let seq = MEMORY_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = wispral_home.join(format!(
        "memory_privacy.json.tmp.{}.{}",
        std::process::id(),
        seq
    ));
    {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("failed to create temp privacy config: {e}"))?;
        f.write_all(json.as_bytes())
            .map_err(|e| format!("failed to write temp privacy config: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("failed to fsync temp privacy config: {e}"))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("failed to atomically replace memory privacy config: {e}")
    })?;

    if normalized.retention == RetentionPolicy::KeepNothing {
        if let Ok(mut map) = session_aside_store().lock() {
            map.remove(wispral_home);
        }
    }
    if let Ok(store) = MemoryStore::open(wispral_home, normalized.clone()) {
        let _ = store.purge_expired_history(now_epoch_ms());
    }
    Ok(())
}

pub fn open_personal_store(wispral_home: &Path) -> Result<MemoryStore, String> {
    let privacy = load_privacy_config(wispral_home);
    let store = MemoryStore::open(wispral_home, privacy).map_err(|e| e.to_string())?;
    let _ = store.purge_expired_history(now_epoch_ms());
    Ok(store)
}

/// Applies personal snippets and correction rules, trimming trailing STT sentence punctuation
/// (`"توقيع البريد."` -> `"توقيع البريد"`) when matching snippet triggers.
pub fn apply_personal_memory_expansions(wispral_home: &Path, input: &str) -> String {
    let Ok(store) = open_personal_store(wispral_home) else {
        return input.to_owned();
    };
    let trimmed_punct = input
        .trim()
        .trim_end_matches(['.', '!', '?', '؟', '،', ';', '؛'])
        .trim();
    if !trimmed_punct.is_empty() {
        if let Ok(bundle) = store.export_bundle() {
            let input_key =
                NormalizedMatchKey::from_surface(trimmed_punct, NormalizationLevel::FullMatchFold);
            for snip in &bundle.snippets {
                let trig_key = NormalizedMatchKey::from_surface(
                    &snip.trigger,
                    NormalizationLevel::FullMatchFold,
                );
                if input_key == trig_key {
                    return snip.expansion.clone();
                }
            }
        }
    }
    store
        .apply_personalizations(input)
        .unwrap_or_else(|_| input.to_owned())
}

pub fn snapshot_personal_memory_at(
    wispral_home: &Path,
    now_ms: u64,
) -> Result<PersonalMemorySnapshot, String> {
    let privacy = load_privacy_config(wispral_home);
    let store = MemoryStore::open(wispral_home, privacy).map_err(|e| e.to_string())?;
    let _ = store.purge_expired_history(now_ms);
    let bundle = store.export_bundle().map_err(|e| e.to_string())?;
    let snippets = bundle
        .snippets
        .into_iter()
        .map(|s| PersonalSnippetDto {
            id: s.id,
            trigger: s.trigger,
            expansion: s.expansion,
            created_at_ms: s.created_at_ms,
        })
        .collect();
    let mut aside_notes: Vec<AsideNoteDto> =
        if bundle.privacy.retention == RetentionPolicy::KeepNothing {
            Vec::new()
        } else {
            bundle
                .history
                .into_iter()
                .filter(|h| h.mode == "aside")
                .map(|h| AsideNoteDto {
                    id: h.id,
                    text: h.text,
                    app_name: h.app_name,
                    created_at_ms: h.created_at_ms,
                })
                .collect()
        };
    if bundle.privacy.retention != RetentionPolicy::KeepNothing {
        if let Ok(map) = session_aside_store().lock() {
            if let Some(in_mem) = map.get(wispral_home) {
                aside_notes.extend(in_mem.iter().cloned());
            }
        }
    }
    Ok(PersonalMemorySnapshot {
        keep_audio: bundle.privacy.keep_audio,
        retention_mode: MemoryRetentionMode::from_core(bundle.privacy.retention),
        snippets,
        aside_notes,
    })
}

pub fn snapshot_personal_memory(wispral_home: &Path) -> Result<PersonalMemorySnapshot, String> {
    snapshot_personal_memory_at(wispral_home, now_epoch_ms())
}

pub fn upsert_personal_snippet(
    wispral_home: &Path,
    id: String,
    trigger: String,
    expansion: String,
    now_ms: u64,
) -> Result<(), String> {
    let trig = trigger.trim();
    let exp = expansion.trim();
    if trig.is_empty() || exp.is_empty() {
        return Err("snippet trigger and expansion must not be empty".into());
    }
    let store = open_personal_store(wispral_home)?;
    let trig_key = NormalizedMatchKey::from_surface(trig, NormalizationLevel::FullMatchFold);
    let existing = store.export_bundle().map_err(|e| e.to_string())?.snippets;
    let mut canonical_id = id;
    let mut matched_first = false;
    for s in existing {
        if NormalizedMatchKey::from_surface(&s.trigger, NormalizationLevel::FullMatchFold)
            == trig_key
        {
            if !matched_first {
                canonical_id = s.id;
                matched_first = true;
            } else {
                let _ = store.delete_entry(&s.id);
            }
        }
    }
    store
        .upsert_snippet(SnippetTemplate {
            id: canonical_id,
            trigger: trig.to_owned(),
            expansion: exp.to_owned(),
            created_at_ms: now_ms,
        })
        .map_err(|e| e.to_string())
}

pub fn record_aside_note(
    wispral_home: &Path,
    id: String,
    text: String,
    app_name: Option<String>,
    now_ms: u64,
) -> Result<bool, String> {
    let clean = text.trim();
    if clean.is_empty() {
        return Err("aside note is empty".into());
    }
    let privacy = load_privacy_config(wispral_home);
    match privacy.retention {
        RetentionPolicy::KeepNothing => Ok(false),
        RetentionPolicy::SessionOnly => {
            let mut map = session_aside_store()
                .lock()
                .map_err(|_| "session aside store lock poisoned".to_owned())?;
            map.entry(wispral_home.to_path_buf())
                .or_default()
                .push(AsideNoteDto {
                    id,
                    text: clean.to_owned(),
                    app_name,
                    created_at_ms: now_ms,
                });
            Ok(true)
        }
        RetentionPolicy::BoundedDays { .. } | RetentionPolicy::UserControlled => {
            let store = MemoryStore::open(wispral_home, privacy).map_err(|e| e.to_string())?;
            let _ = store.purge_expired_history(now_ms);
            store
                .record_history(HistoryRecord {
                    id,
                    mode: "aside".into(),
                    text: clean.to_owned(),
                    app_name,
                    audio_ref: None,
                    created_at_ms: now_ms,
                })
                .map_err(|e| e.to_string())
        }
    }
}

pub fn wipe_personal_memory(wispral_home: &Path) -> Result<(), String> {
    if let Ok(mut map) = session_aside_store().lock() {
        map.remove(wispral_home);
    }
    let store = open_personal_store(wispral_home)?;
    store.wipe_all().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn personal_memory_defaults_keep_audio_false_respects_keep_nothing_session_only_ttl_and_wipes()
    {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("wispral_home");

        let snap0 = snapshot_personal_memory(&home).unwrap();
        assert!(!snap0.keep_audio);
        assert_eq!(snap0.retention_mode, MemoryRetentionMode::Bounded7Days);

        // Upserting same trigger updates in place without shadowing!
        upsert_personal_snippet(
            &home,
            next_memory_id("sn", 1000),
            "توقيع".into(),
            "النسخة القديمة".into(),
            1000,
        )
        .unwrap();
        upsert_personal_snippet(
            &home,
            next_memory_id("sn", 1001),
            "توقيع".into(),
            "مع خالص التحية — WisprSide".into(),
            1001,
        )
        .unwrap();
        assert_eq!(
            apply_personal_memory_expansions(&home, "توقيع."),
            "مع خالص التحية — WisprSide"
        );

        assert!(record_aside_note(
            &home,
            "as-1".into(),
            "benchmark Gulf dialect".into(),
            Some("Code.exe".into()),
            1001
        )
        .unwrap());

        let snap1 = snapshot_personal_memory_at(&home, 2_000).unwrap();
        assert_eq!(snap1.snippets.len(), 1);
        assert_eq!(snap1.aside_notes.len(), 1);

        // TTL expiry after 8 days purges disk aside note
        let snap_expired = snapshot_personal_memory_at(&home, 1001 + 8 * 86_400_000).unwrap();
        assert!(snap_expired.aside_notes.is_empty());

        // SessionOnly keeps note in RAM with zero disk entries in history.jsonl
        save_privacy_config(
            &home,
            &MemoryPrivacyConfig {
                keep_audio: false,
                retention: MemoryRetentionMode::SessionOnly.to_core(),
            },
        )
        .unwrap();
        assert!(record_aside_note(
            &home,
            "as-ram".into(),
            "ram-only thought".into(),
            None,
            5_000
        )
        .unwrap());
        let snap_ram = snapshot_personal_memory_at(&home, 5_001).unwrap();
        assert_eq!(snap_ram.aside_notes.len(), 1);
        let disk_hist =
            std::fs::read_to_string(home.join("memory").join("history.jsonl")).unwrap_or_default();
        assert!(!disk_hist.contains("ram-only thought"));

        // Corrupt privacy file fails closed to KeepNothing (`SI-3`)
        std::fs::write(home.join("memory_privacy.json"), "{corrupt-json").unwrap();
        let corrupt_cfg = load_privacy_config(&home);
        assert_eq!(corrupt_cfg.retention, RetentionPolicy::KeepNothing);
        assert!(!corrupt_cfg.keep_audio);
        assert!(!record_aside_note(
            &home,
            "as-blocked".into(),
            "must not save".into(),
            None,
            6_000
        )
        .unwrap());

        // Restore clean KeepNothing & wipe clears everything
        save_privacy_config(
            &home,
            &MemoryPrivacyConfig {
                keep_audio: false,
                retention: MemoryRetentionMode::KeepNothing.to_core(),
            },
        )
        .unwrap();
        wipe_personal_memory(&home).unwrap();
        let snap2 = snapshot_personal_memory(&home).unwrap();
        assert!(snap2.snippets.is_empty());
        assert!(snap2.aside_notes.is_empty());
    }
}
