//! WisprSide Application-Aware Profiles (`SD-2`).
//!
//! Classifies the bound foreground executable (`TargetBinding.executable_basename`)
//! into one of 8 canonical application categories (`Email`, `Chat`, `Browser`,
//! `Terminal`, `Ide`, `Document`, `Notes`, `BusinessApp`), resolves per-category
//! tone/formatting/mode defaults, and enforces the non-negotiable Terminal safety
//! floor (`auto_submit = false` unconditionally in terminals).

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Email,
    Chat,
    Browser,
    Terminal,
    Ide,
    Document,
    Notes,
    BusinessApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ToneStyle {
    Professional,
    ConciseConversational,
    TechnicalCodeAware,
    StructuredProse,
    VerbatimClean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AppProfile {
    pub category: AppCategory,
    pub tone: ToneStyle,
    pub preserve_code_identifiers: bool,
    pub smart_list_formatting: bool,
    pub allow_auto_submit: bool,
    pub preferred_mode: String,
}

pub fn classify_executable(executable_basename: &str) -> AppCategory {
    let lower = executable_basename
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);

    match stem {
        "windowsterminal"
        | "wt"
        | "powershell"
        | "pwsh"
        | "cmd"
        | "conhost"
        | "openconsole"
        | "wsl"
        | "wslhost"
        | "git-bash"
        | "terminal"
        | "alacritty"
        | "wezterm"
        | "wezterm-gui"
        | "kitty"
        | "ghostty"
        | "iterm2"
        | "warp"
        | "bash"
        | "zsh"
        | "fish"
        | "mintty"
        | "gnome-terminal"
        | "gnome-terminal-server"
        | "kgx"
        | "ptyxis"
        | "konsole"
        | "xfce4-terminal"
        | "tilix"
        | "terminator"
        | "xterm"
        | "urxvt"
        | "foot"
        | "rio"
        | "hyper"
        | "tabby" => AppCategory::Terminal,
        "code" | "code - insiders" | "cursor" | "windsurf" | "antigravity" | "zed" | "idea64"
        | "pycharm64" | "webstorm64" | "rustrover64" | "clion64" | "devenv" | "nvim" | "vim"
        | "sublime_text" => AppCategory::Ide,
        "slack" | "teams" | "ms-teams" | "msteams" | "discord" | "telegram" | "whatsapp"
        | "signal" | "mattermost" | "element" => AppCategory::Chat,
        "outlook" | "olk" | "thunderbird" | "spark" | "superhuman" | "mimestream" => {
            AppCategory::Email
        }
        "chrome" | "msedge" | "firefox" | "brave" | "arc" | "vivaldi" | "safari" | "zen" => {
            AppCategory::Browser
        }
        "obsidian" | "notion" | "logseq" | "onenote" | "bear" | "joplin" | "anytype" => {
            AppCategory::Notes
        }
        "winword" | "soffice" | "pages" | "scrivener" | "typora" | "wordpad" | "notepad" => {
            AppCategory::Document
        }
        _ => AppCategory::BusinessApp,
    }
}

pub fn default_profile_for(category: AppCategory) -> AppProfile {
    match category {
        AppCategory::Terminal => AppProfile {
            category,
            tone: ToneStyle::TechnicalCodeAware,
            preserve_code_identifiers: true,
            smart_list_formatting: false,
            allow_auto_submit: false, // Hard safety invariant: never auto-execute shell lines!
            preferred_mode: "dictation".into(),
        },
        AppCategory::Ide => AppProfile {
            category,
            tone: ToneStyle::TechnicalCodeAware,
            preserve_code_identifiers: true,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "dictation".into(),
        },
        AppCategory::Email => AppProfile {
            category,
            tone: ToneStyle::Professional,
            preserve_code_identifiers: true,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "compose".into(),
        },
        AppCategory::Chat => AppProfile {
            category,
            tone: ToneStyle::ConciseConversational,
            preserve_code_identifiers: true,
            smart_list_formatting: false,
            allow_auto_submit: true,
            preferred_mode: "dictation".into(),
        },
        AppCategory::Browser => AppProfile {
            category,
            tone: ToneStyle::VerbatimClean,
            preserve_code_identifiers: true,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "dictation".into(),
        },
        AppCategory::Notes => AppProfile {
            category,
            tone: ToneStyle::StructuredProse,
            preserve_code_identifiers: true,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "compose".into(),
        },
        AppCategory::Document => AppProfile {
            category,
            tone: ToneStyle::StructuredProse,
            preserve_code_identifiers: false,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "compose".into(),
        },
        AppCategory::BusinessApp => AppProfile {
            category,
            tone: ToneStyle::Professional,
            preserve_code_identifiers: true,
            smart_list_formatting: true,
            allow_auto_submit: true,
            preferred_mode: "dictation".into(),
        },
    }
}

/// Resolves whether auto-submit (`Enter`) may fire after delivery.
/// Unconditionally returns `false` for `AppCategory::Terminal` regardless of user toggle.
pub fn effective_auto_submit(category: AppCategory, user_auto_submit: bool) -> bool {
    if category == AppCategory::Terminal {
        return false;
    }
    user_auto_submit && default_profile_for(category).allow_auto_submit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_all_eight_categories_and_blocks_terminal_auto_submit() {
        for term in [
            "WindowsTerminal.exe",
            "pwsh.exe",
            "wsl.exe",
            "OpenConsole.exe",
            "Terminal",
            "gnome-terminal-server",
            "konsole",
        ] {
            assert_eq!(classify_executable(term), AppCategory::Terminal, "{term}");
        }
        assert_eq!(classify_executable("Code.exe"), AppCategory::Ide);
        assert_eq!(classify_executable("cursor.exe"), AppCategory::Ide);
        assert_eq!(classify_executable("slack.exe"), AppCategory::Chat);
        assert_eq!(classify_executable("msteams.exe"), AppCategory::Chat);
        assert_eq!(classify_executable("OUTLOOK.EXE"), AppCategory::Email);
        assert_eq!(classify_executable("msedge.exe"), AppCategory::Browser);
        assert_eq!(classify_executable("Obsidian.exe"), AppCategory::Notes);
        assert_eq!(classify_executable("WINWORD.EXE"), AppCategory::Document);
        assert_eq!(classify_executable("sapgui.exe"), AppCategory::BusinessApp);

        // Terminal NEVER auto-submits even if user enabled auto_submit globally
        assert!(!effective_auto_submit(AppCategory::Terminal, true));
        // Non-terminal categories honor explicit user opt-in (`auto_submit == true`)
        assert!(effective_auto_submit(AppCategory::Chat, true));
        assert!(effective_auto_submit(AppCategory::Ide, true));
        assert!(effective_auto_submit(AppCategory::BusinessApp, true));
        assert!(!effective_auto_submit(AppCategory::Chat, false));
        assert!(!effective_auto_submit(AppCategory::Ide, false));
    }
}
