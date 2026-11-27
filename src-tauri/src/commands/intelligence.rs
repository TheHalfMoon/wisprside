//! Tauri Specta commands for WisprSide Intelligence (`SD-2`):
//! Writing Modes, App Profiles, Personal Memory, and Structured Agent Control.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::agent_control::{
    list_agent_capabilities, prepare_agent_dispatch, AgentCapabilityDescriptor,
    AgentPromptEnvelope, AgentTargetId,
};
use crate::managers::history::HistoryManager;
use crate::personal_memory::{
    apply_personal_memory_expansions, load_privacy_config, next_memory_id, record_aside_note,
    save_privacy_config, snapshot_personal_memory, upsert_personal_snippet, wipe_personal_memory,
    MemoryRetentionMode, PersonalMemorySnapshot,
};
use crate::profiles::{classify_executable, default_profile_for, AppCategory, AppProfile};
use crate::settings::{get_settings, write_settings};
use crate::writing_modes::{
    execute_writing_mode, ModeExecutionRequest, ModeExecutionResult, WritingMode,
};

pub(crate) fn wispral_home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = crate::portable::app_data_dir(app)
        .map_err(|e| format!("Failed to resolve WisprSide data dir: {e}"))?;
    Ok(base.join("wispral_home"))
}

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Validates a workspace root directory against relative paths, `..` traversal,
/// symlinks, and Windows NTFS junctions / mount-point reparse points across every ancestor.
pub fn validate_workspace_root_dir(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("workspace root is empty".into());
    }
    let p = PathBuf::from(trimmed);
    if !p.is_absolute() {
        return Err("workspace root must be an absolute path".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("workspace root must not contain '..' segments".into());
    }
    check_directory_and_ancestors(&p)?;
    Ok(p)
}

fn check_directory_and_ancestors(path: &Path) -> Result<(), String> {
    let leaf_meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("invalid workspace root {}: {e}", path.display()))?;
    if !leaf_meta.is_dir() {
        return Err("workspace root must be a regular directory".into());
    }
    for anc in path.ancestors() {
        if anc.as_os_str().is_empty() {
            break;
        }
        let meta = match std::fs::symlink_metadata(anc) {
            Ok(m) => m,
            Err(_) if anc.parent().is_none() => break,
            Err(e) => return Err(format!("cannot inspect ancestor {}: {e}", anc.display())),
        };
        if meta.file_type().is_symlink() {
            return Err(format!(
                "workspace root rejected: symlink in path ({})",
                anc.display()
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            // Drive roots (e.g. `D:\`) are not junctions unless mounted; skip root drive letter check only if `anc.parent().is_none()`.
            if anc.parent().is_some()
                && (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
            {
                return Err(format!(
                    "workspace root rejected: reparse point or junction in path ({})",
                    anc.display()
                ));
            }
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn list_app_profiles_cmd() -> Vec<AppProfile> {
    [
        AppCategory::Terminal,
        AppCategory::Ide,
        AppCategory::Email,
        AppCategory::Chat,
        AppCategory::Browser,
        AppCategory::Notes,
        AppCategory::Document,
        AppCategory::BusinessApp,
    ]
    .into_iter()
    .map(default_profile_for)
    .collect()
}

#[tauri::command]
#[specta::specta]
pub fn get_app_profile_cmd(executable_basename: String) -> AppProfile {
    default_profile_for(classify_executable(&executable_basename))
}

#[tauri::command]
#[specta::specta]
pub fn set_active_writing_mode_cmd(
    app: AppHandle,
    mode: WritingMode,
    target_language: Option<String>,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.active_writing_mode = mode;
    if let Some(lang) = target_language {
        let trimmed = lang.trim();
        if !trimmed.is_empty() {
            settings.translation_target_language = trimmed.to_owned();
        }
    }
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn execute_writing_mode_cmd(
    app: AppHandle,
    mut request: ModeExecutionRequest,
) -> Result<ModeExecutionResult, String> {
    let home = wispral_home_dir(&app)?;
    request.spoken_transcript = apply_personal_memory_expansions(&home, &request.spoken_transcript);
    let result = execute_writing_mode(&request)?;
    if result.mode == WritingMode::Aside && !result.output_text.is_empty() {
        let now = now_epoch_ms();
        let id = next_memory_id("aside", now);
        let _ = record_aside_note(&home, id, result.output_text.clone(), None, now);
    }
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub fn list_agent_capabilities_cmd() -> Vec<AgentCapabilityDescriptor> {
    list_agent_capabilities()
}

#[tauri::command]
#[specta::specta]
pub fn prepare_agent_dispatch_cmd(
    target: AgentTargetId,
    spoken_prompt: String,
    workspace_root: Option<String>,
    user_confirmed_high_risk: bool,
) -> Result<AgentPromptEnvelope, String> {
    let validated_root = match workspace_root {
        Some(raw) if !raw.trim().is_empty() => Some(validate_workspace_root_dir(&raw)?),
        _ => None,
    };
    prepare_agent_dispatch(
        target,
        &spoken_prompt,
        validated_root.as_deref(),
        user_confirmed_high_risk,
    )
}

#[tauri::command]
#[specta::specta]
pub fn get_memory_bundle_cmd(app: AppHandle) -> Result<PersonalMemorySnapshot, String> {
    let home = wispral_home_dir(&app)?;
    snapshot_personal_memory(&home)
}

#[tauri::command]
#[specta::specta]
pub fn set_memory_privacy_cmd(
    app: AppHandle,
    retention_mode: MemoryRetentionMode,
    keep_audio: bool,
) -> Result<PersonalMemorySnapshot, String> {
    let home = wispral_home_dir(&app)?;
    let mut cfg = load_privacy_config(&home);
    cfg.keep_audio = keep_audio;
    cfg.retention = retention_mode.to_core();
    save_privacy_config(&home, &cfg)?;
    snapshot_personal_memory(&home)
}

#[tauri::command]
#[specta::specta]
pub fn upsert_memory_snippet_cmd(
    app: AppHandle,
    trigger: String,
    expansion: String,
) -> Result<PersonalMemorySnapshot, String> {
    let home = wispral_home_dir(&app)?;
    let now = now_epoch_ms();
    let id = next_memory_id("snip", now);
    upsert_personal_snippet(&home, id, trigger, expansion, now)?;
    snapshot_personal_memory(&home)
}

#[tauri::command]
#[specta::specta]
pub fn wipe_personal_memory_cmd(app: AppHandle) -> Result<PersonalMemorySnapshot, String> {
    let home = wispral_home_dir(&app)?;
    wipe_personal_memory(&home)?;
    if let Some(hm) = app.try_state::<Arc<HistoryManager>>() {
        let _ = hm.wipe_all_entries();
    }
    snapshot_personal_memory(&home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn workspace_root_validator_rejects_relative_parent_and_reparse_ancestors() {
        assert!(validate_workspace_root_dir("relative/dir").is_err());
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("real_sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(validate_workspace_root_dir(sub.to_str().unwrap()).is_ok());

        let with_parent = sub.join("..").join("real_sub");
        assert!(validate_workspace_root_dir(with_parent.to_str().unwrap()).is_err());

        #[cfg(windows)]
        {
            let junc_parent = dir.path().join("junction_parent");
            let status = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    junc_parent.to_str().unwrap(),
                    sub.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(status.status.success());
            let nested = junc_parent.join("child");
            std::fs::create_dir_all(&sub.join("child")).unwrap();
            let err = validate_workspace_root_dir(nested.to_str().unwrap()).unwrap_err();
            assert!(
                err.contains("reparse point") || err.contains("symlink"),
                "unexpected error: {err}"
            );
            let _ = std::fs::remove_dir(&junc_parent);
        }
    }
}
