//! WisprSide Six Intelligent Writing Modes (`SD-2`).
//!
//! Modes:
//! 1. `Dictation` — clean, natural punctuation, Arabic/code-switch aware, verbatim intent.
//! 2. `Compose` — turns spoken rough thoughts into structured Email, Markdown, BulletList, or CommitMessage.
//! 3. `Edit` — transforms non-destructively captured UIA `selected_text` via spoken instruction,
//!    guarded by `verify_edit_safety` (protected entity & negation preservation) + preview support.
//! 4. `Translate` — offline glossary-backed EN <-> AR / multilingual translation (`wispral-speech`)
//!    preserving protected technical identifiers, CLI flags, and PR/issue numbers.
//! 5. `Command` — structured deterministic voice commands (undo, scratch, mode switch, code block,
//!    agent dispatch) with zero shell execution.
//! 6. `Aside` — non-delivering voice note / scratchpad capture (`delivers_to_target == false`).

use serde::{Deserialize, Serialize};
use specta::Type;
use wispral_arabic::{
    extract_protected_entities, tokenize_proclitic_aware, EntityKind, VerbatimTranscript,
};
use wispral_speech::OfflineGlossaryTranslator;

use crate::profiles::{AppCategory, ToneStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum WritingMode {
    #[default]
    Dictation,
    Compose,
    Edit,
    Translate,
    Command,
    Aside,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum StructuredCommand {
    ScratchLastSentence,
    NewParagraph,
    WrapCodeBlock { language: String, content: String },
    SwitchMode { target_mode: WritingMode },
    DispatchToAgent { prompt: String },
    LiteralText { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ModeExecutionRequest {
    pub mode: WritingMode,
    pub spoken_transcript: String,
    pub selected_text: Option<String>,
    pub target_language: Option<String>,
    pub app_category: AppCategory,
    pub tone: ToneStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ModeExecutionResult {
    pub mode: WritingMode,
    pub output_text: String,
    pub bidi_display_text: String,
    pub delivers_to_target: bool,
    pub requires_preview_confirmation: bool,
    pub structured_command: Option<StructuredCommand>,
    pub safety_notes: Vec<String>,
}

fn has_negation_cue(text: &str) -> bool {
    let en_neg = text.split_whitespace().any(|tok| {
        let clean = tok
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '\'' && c != '’')
            .replace('’', "'")
            .to_ascii_lowercase();
        matches!(
            clean.as_str(),
            "no" | "not"
                | "never"
                | "without"
                | "cannot"
                | "can't"
                | "won't"
                | "don't"
                | "doesn't"
                | "didn't"
                | "shouldn't"
                | "mustn't"
                | "isn't"
                | "aren't"
        )
    });
    let ar_neg = tokenize_proclitic_aware(text).iter().any(|tok| {
        for key in [tok.stem_key.as_key_str(), tok.match_key.as_key_str()] {
            if matches!(
                key,
                "لا" | "لم"
                    | "لن"
                    | "ليس"
                    | "ليست"
                    | "بدون"
                    | "غير"
                    | "ولا"
                    | "فلا"
                    | "الا"
                    | "ولم"
                    | "فلم"
                    | "ولن"
                    | "فلن"
                    | "وليس"
                    | "فليس"
                    | "وليست"
                    | "وبدون"
                    | "بغير"
                    | "وغير"
            ) {
                return true;
            }
        }
        false
    });
    en_neg || ar_neg
}

/// Verifies that an intelligent `Edit` transformation preserved every CLI flag and reference ID
/// from `original_selection` and did not silently drop a negation unless explicitly instructed.
pub fn verify_edit_safety(
    original_selection: &str,
    instruction: &str,
    candidate_output: &str,
) -> Result<(), String> {
    let orig_entities = extract_protected_entities(original_selection);
    for ent in orig_entities {
        if matches!(ent.kind, EntityKind::CliFlag | EntityKind::ReferenceId)
            && !candidate_output.contains(&ent.text)
        {
            return Err(format!(
                "edit safety guard blocked rewrite: protected entity `{}` was dropped",
                ent.text
            ));
        }
    }
    let instr_lower = instruction.to_ascii_lowercase();
    let explicit_polarity_change = instr_lower.contains("affirmative")
        || instr_lower.contains("remove negation")
        || instruction.contains("إزالة النفي");
    if !explicit_polarity_change
        && has_negation_cue(original_selection)
        && !has_negation_cue(candidate_output)
    {
        return Err(
            "edit safety guard blocked rewrite: negation polarity was dropped without instruction"
                .into(),
        );
    }
    Ok(())
}

fn format_clean_dictation(raw: &str, smart_lists: bool) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if smart_lists
        && (trimmed.contains(" أولاً ") || trimmed.to_ascii_lowercase().starts_with("bullet "))
    {
        let body = trimmed
            .strip_prefix("bullet ")
            .or_else(|| trimmed.strip_prefix("Bullet "))
            .unwrap_or(trimmed);
        return format!("- {body}");
    }
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(trimmed.len() + 2);
    for up in first.to_uppercase() {
        out.push(up);
    }
    out.push_str(chars.as_str());
    if !out.ends_with(['.', '!', '?', '؟', ':', ';', '،'])
        && !out.contains('`')
        && out.split_whitespace().count() >= 4
    {
        out.push('.');
    }
    out
}

fn compose_structured(spoken: &str, category: AppCategory, tone: ToneStyle) -> String {
    let clean = format_clean_dictation(spoken, true);
    if clean.is_empty() {
        return clean;
    }
    let parts: Vec<&str> = clean
        .split([',', '،', ';', '؛'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    match category {
        AppCategory::Email => {
            let is_ar = clean.chars().any(wispral_arabic::is_arabic_script);
            let (greeting, signoff) = match (is_ar, tone) {
                (true, ToneStyle::ConciseConversational) => ("مرحباً،", "شكراً،"),
                (true, _) => ("تحية طيبة،", "مع خالص التحية"),
                (false, ToneStyle::ConciseConversational) => ("Hi,", "Thanks,"),
                (false, _) => ("Hello,", "Best regards,"),
            };
            let body = if parts.len() >= 3 {
                parts
                    .iter()
                    .map(|p| format!("- {}", p.trim_end_matches('.')))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                clean
            };
            format!("{greeting}\n\n{body}\n\n{signoff}")
        }
        AppCategory::Notes | AppCategory::Document if parts.len() >= 2 => parts
            .iter()
            .map(|p| format!("- {}", p.trim_end_matches('.')))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => match tone {
            ToneStyle::StructuredProse if parts.len() >= 2 => parts
                .iter()
                .map(|p| format!("- {}", p.trim_end_matches('.')))
                .collect::<Vec<_>>()
                .join("\n"),
            ToneStyle::ConciseConversational => clean.trim_end_matches('.').to_owned(),
            _ => clean,
        },
    }
}

fn apply_local_edit(selection: &str, instruction: &str) -> Result<String, String> {
    let instr = instruction.trim();
    let lower = instr.to_ascii_lowercase();

    let candidate = if lower.contains("bullet") || instr.contains("نقاط") {
        selection
            .split(['.', '\n', '،', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    } else if lower.contains("uppercase") || instr.contains("حروف كبيرة") {
        selection.to_uppercase()
    } else if lower.contains("lowercase") || instr.contains("حروف صغيرة") {
        selection.to_lowercase()
    } else if lower.contains("code block") || instr.contains("كتلة كود") {
        format!("```\n{}\n```", selection.trim())
    } else if lower.contains("translate to english") || instr.contains("ترجم للإنجليزية")
    {
        OfflineGlossaryTranslator
            .translate(selection, "en")
            .map_err(|e| e.to_string())?
            .as_str()
            .to_owned()
    } else if lower.contains("translate to arabic") || instr.contains("ترجم للعربية") {
        OfflineGlossaryTranslator
            .translate(selection, "ar")
            .map_err(|e| e.to_string())?
            .as_str()
            .to_owned()
    } else if let Some(rest_lower) = lower.strip_prefix("replace ") {
        if let Some(with_idx) = rest_lower.find(" with ") {
            let after_replace = &instr["replace ".len()..];
            let from_orig = after_replace[..with_idx].trim();
            let to_orig = after_replace[with_idx + " with ".len()..]
                .trim()
                .trim_end_matches(['.', '!', '?']);
            if !from_orig.is_empty() {
                selection.replace(from_orig, to_orig)
            } else {
                format!("{} ({})", selection.trim(), instr)
            }
        } else {
            format!("{} ({})", selection.trim(), instr)
        }
    } else if lower.contains("concise") || lower.contains("shorten") || instr.contains("اختصر")
    {
        selection
            .replace("very ", "")
            .replace("really ", "")
            .replace("basically ", "")
            .replace("in order to ", "to ")
            .trim()
            .to_owned()
    } else {
        format_clean_dictation(selection, false)
    };

    verify_edit_safety(selection, instr, &candidate)?;
    Ok(candidate)
}

fn parse_voice_command(spoken: &str) -> StructuredCommand {
    let trimmed = spoken.trim();
    let unpunct = trimmed
        .trim_end_matches(['.', '!', '?', '؟', '،', ';', '؛'])
        .trim();
    let lower = unpunct.to_ascii_lowercase();

    if matches!(
        lower.as_str(),
        "scratch that" | "undo that" | "delete last sentence"
    ) || unpunct == "احذف الجملة الأخيرة"
    {
        return StructuredCommand::ScratchLastSentence;
    }
    if matches!(lower.as_str(), "new paragraph" | "next paragraph") || unpunct == "فقرة جديدة"
    {
        return StructuredCommand::NewParagraph;
    }
    if let Some(rest) = lower.strip_prefix("switch to ") {
        let mode = match rest.trim_end_matches(" mode").trim() {
            "dictation" => Some(WritingMode::Dictation),
            "compose" => Some(WritingMode::Compose),
            "edit" => Some(WritingMode::Edit),
            "translate" => Some(WritingMode::Translate),
            "aside" | "note" => Some(WritingMode::Aside),
            _ => None,
        };
        if let Some(target_mode) = mode {
            return StructuredCommand::SwitchMode { target_mode };
        }
    }
    for prefix in ["ask agent to ", "tell agent to ", "agent "] {
        if lower.starts_with(prefix) {
            let body = unpunct[prefix.len()..].trim().to_owned();
            if !body.is_empty() {
                return StructuredCommand::DispatchToAgent { prompt: body };
            }
        }
    }
    if let Some(ar_rest) = unpunct.strip_prefix("اطلب من الوكيل ") {
        let body = ar_rest.trim().to_owned();
        if !body.is_empty() {
            return StructuredCommand::DispatchToAgent { prompt: body };
        }
    }
    if lower.starts_with("code block ") {
        let code_body = trimmed["code block ".len()..].trim();
        if !code_body.is_empty() {
            return StructuredCommand::WrapCodeBlock {
                language: String::new(),
                content: code_body.to_owned(),
            };
        }
    }
    StructuredCommand::LiteralText {
        text: format_clean_dictation(trimmed, false),
    }
}

pub fn execute_writing_mode(req: &ModeExecutionRequest) -> Result<ModeExecutionResult, String> {
    match req.mode {
        WritingMode::Dictation => {
            let smart_lists = !matches!(req.app_category, AppCategory::Terminal | AppCategory::Ide);
            let out = if req.app_category == AppCategory::Terminal {
                req.spoken_transcript.trim().to_owned()
            } else {
                format_clean_dictation(&req.spoken_transcript, smart_lists)
            };
            let bidi = VerbatimTranscript::new(&out)
                .to_bidi_isolated()
                .as_str()
                .to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Dictation,
                output_text: out,
                bidi_display_text: bidi,
                delivers_to_target: true,
                requires_preview_confirmation: false,
                structured_command: None,
                safety_notes: vec![],
            })
        }
        WritingMode::Compose => {
            let out = compose_structured(&req.spoken_transcript, req.app_category, req.tone);
            let bidi = VerbatimTranscript::new(&out)
                .to_bidi_isolated()
                .as_str()
                .to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Compose,
                output_text: out,
                bidi_display_text: bidi,
                delivers_to_target: true,
                requires_preview_confirmation: false,
                structured_command: None,
                safety_notes: vec![],
            })
        }
        WritingMode::Edit => {
            let selection = req
                .selected_text
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    "Edit mode requires selected text in the active control (captured non-destructively via UIA)".to_owned()
                })?;
            let edited = apply_local_edit(selection, &req.spoken_transcript)?;
            let bidi = VerbatimTranscript::new(&edited)
                .to_bidi_isolated()
                .as_str()
                .to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Edit,
                output_text: edited,
                bidi_display_text: bidi,
                delivers_to_target: true,
                requires_preview_confirmation: true,
                structured_command: None,
                safety_notes: vec![
                    "verified_non_destructive_selection".into(),
                    "verified_entity_and_negation_preservation".into(),
                ],
            })
        }
        WritingMode::Translate => {
            let target = req.target_language.as_deref().unwrap_or("en");
            let translated = OfflineGlossaryTranslator
                .translate(&req.spoken_transcript, target)
                .map_err(|e| e.to_string())?;
            let bidi = translated.to_bidi_isolated().as_str().to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Translate,
                output_text: translated.as_str().to_owned(),
                bidi_display_text: bidi,
                delivers_to_target: true,
                requires_preview_confirmation: false,
                structured_command: None,
                safety_notes: vec!["protected_entities_preserved".into()],
            })
        }
        WritingMode::Command => {
            let cmd = parse_voice_command(&req.spoken_transcript);
            let (out, delivers) = match &cmd {
                StructuredCommand::NewParagraph => ("\n\n".to_owned(), true),
                StructuredCommand::WrapCodeBlock { language, content } => {
                    (format!("```{language}\n{content}\n```"), true)
                }
                StructuredCommand::LiteralText { text } => (text.clone(), true),
                StructuredCommand::ScratchLastSentence
                | StructuredCommand::SwitchMode { .. }
                | StructuredCommand::DispatchToAgent { .. } => (String::new(), false),
            };
            let bidi = VerbatimTranscript::new(&out)
                .to_bidi_isolated()
                .as_str()
                .to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Command,
                output_text: out,
                bidi_display_text: bidi,
                delivers_to_target: delivers,
                requires_preview_confirmation: false,
                structured_command: Some(cmd),
                safety_notes: vec!["no_shell_exec".into()],
            })
        }
        WritingMode::Aside => {
            let note = format_clean_dictation(&req.spoken_transcript, true);
            let bidi = VerbatimTranscript::new(&note)
                .to_bidi_isolated()
                .as_str()
                .to_owned();
            Ok(ModeExecutionResult {
                mode: WritingMode::Aside,
                output_text: note,
                bidi_display_text: bidi,
                delivers_to_target: false, // Never injects into active window!
                requires_preview_confirmation: false,
                structured_command: None,
                safety_notes: vec!["captured_to_aside_scratchpad_only".into()],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_writing_modes_and_edit_safety_guards_work_offline() {
        // 1. Compose (Email)
        let composed = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Compose,
            spoken_transcript: "ship WisprSide v0.10, verify target binding, update release notes"
                .into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Email,
            tone: ToneStyle::Professional,
        })
        .unwrap();
        assert!(composed.output_text.contains("Hello,"));
        assert!(composed.output_text.contains("- Ship WisprSide v0.10"));
        assert!(composed.delivers_to_target);

        // 2. Edit preserves `#160`, `--release`, and negations; blocks dropped entity
        let edited = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Edit,
            spoken_transcript: "make it concise".into(),
            selected_text: Some(
                "Please do not merge #160 without very careful --release checks.".into(),
            ),
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(edited.output_text.contains("do not"));
        assert!(edited.output_text.contains("#160"));
        assert!(edited.output_text.contains("--release"));
        assert!(edited.requires_preview_confirmation);

        assert!(verify_edit_safety(
            "Do not merge #160 with --force",
            "shorten",
            "Merge #160 with --force"
        )
        .is_err());

        // 3. Translate Arabic + English code-switching
        let tr = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Translate,
            spoken_transcript:
                "افتح مشروع Ecra وسوّ code review للـ pull request الأخير، وبعدها شغّل الاختبارات."
                    .into(),
            selected_text: None,
            target_language: Some("en".into()),
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(tr.output_text.contains("Open project Ecra"));
        assert!(tr.output_text.contains("pull request"));

        // 4. Command routes agent prompt without pasting into target
        let cmd = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Command,
            spoken_transcript: "ask agent to inspect paste_tx target binding".into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(!cmd.delivers_to_target);
        assert_eq!(
            cmd.structured_command,
            Some(StructuredCommand::DispatchToAgent {
                prompt: "inspect paste_tx target binding".into()
            })
        );

        // 5. Aside never delivers to target window
        let aside = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Aside,
            spoken_transcript: "remember to benchmark Arabic Gulf dialect tomorrow".into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Terminal,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(!aside.delivers_to_target);

        // 6. Punctuated & capitalized STT commands and proclitic/contraction negations
        let scratch = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Command,
            spoken_transcript: "Scratch that.".into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(!scratch.delivers_to_target);
        assert_eq!(
            scratch.structured_command,
            Some(StructuredCommand::ScratchLastSentence)
        );

        let cb = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Command,
            spoken_transcript: "Code block fn main() {}".into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert!(cb.delivers_to_target);
        assert!(cb.output_text.contains("```"));
        assert!(cb.output_text.contains("fn main() {}"));

        let sw = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Command,
            spoken_transcript: "Switch to compose mode.".into(),
            selected_text: None,
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert_eq!(
            sw.structured_command,
            Some(StructuredCommand::SwitchMode {
                target_mode: WritingMode::Compose
            })
        );

        let rep = execute_writing_mode(&ModeExecutionRequest {
            mode: WritingMode::Edit,
            spoken_transcript: "Replace Foo With Bar.".into(),
            selected_text: Some("Foo is ready".into()),
            target_language: None,
            app_category: AppCategory::Ide,
            tone: ToneStyle::TechnicalCodeAware,
        })
        .unwrap();
        assert_eq!(rep.output_text, "Bar is ready");

        assert!(verify_edit_safety("Please don't merge #160", "shorten", "Merge #160").is_err());
        assert!(verify_edit_safety("ولن ندمج #160", "اختصر", "ندمج #160").is_err());
    }
}
