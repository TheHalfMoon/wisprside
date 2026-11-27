//! WisprSide Structured AI Coding-Agent Voice Control (`SD-2`).
//!
//! Provides:
//! 1. Honest capability matrix across `Antigravity`, `Claude Code`, `Codex`,
//!    `OpenCode`, `Gemini CLI`, and `Cursor` (aligned with `docs/research/acp/capability-matrix.json`);
//! 2. JSON-RPC 2.0 `ACP` `stdio` message framing (`initialize`, `session/new`,
//!    `session/prompt`, `$/cancel_request`) + safe `Command::new` CLI adapters;
//! 3. Workspace vocabulary symbol/file grounding + automatic secret redaction
//!    (`wispral_context::scan_workspace_vocabulary`, `redact_sensitive_text`);
//! 4. Deterministic `wispral_policy` permission gating (`Read` / `ReversibleWrite` /
//!    `IrreversibleWrite`) so destructive actions require explicit human confirmation;
//! 5. Verified ACP 5.0 cancellation (`CancellationRecord` bound to `ProcessInstance`,
//!    releasing leases ONLY on observed process exit or worker idle report).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;
use wispral_arabic::{tokenize_proclitic_aware, NormalizationLevel, NormalizedMatchKey};
use wispral_context::{redact_sensitive_text, scan_workspace_vocabulary};
use wispral_contracts::{CancellationRecord, CancellationState, ProcessInstance};
use wispral_policy::{
    evaluate_verdict, ActionClass, CandidateDecision, EnforcedDecision, EnforcementInputs,
    LayerSource, PolicyLayer, UndecidableResolution,
};

static ACP_REQ_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentTargetId {
    Antigravity,
    ClaudeCode,
    Codex,
    OpenCode,
    GeminiCli,
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationTransport {
    /// Speaks JSON-RPC 2.0 Agent Client Protocol (`ACP`) over `stdio`.
    AcpStdio,
    /// Invokes documented non-interactive CLI flags via `Command::new` arg array (zero shell).
    ManagedCliSubprocess,
    /// Delivers structured context-grounded prompt via target-bound IDE bridge.
    TargetBoundIdeBridge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentCapabilityDescriptor {
    pub id: AgentTargetId,
    pub display_name: String,
    pub transport: IntegrationTransport,
    pub default_binary: String,
    pub supports_session_resume: bool,
    pub supports_permission_gate: bool,
    pub supports_verified_process_cancel: bool,
    pub qualification_note: String,
}

pub fn list_agent_capabilities() -> Vec<AgentCapabilityDescriptor> {
    vec![
        AgentCapabilityDescriptor {
            id: AgentTargetId::Antigravity,
            display_name: "Antigravity".into(),
            transport: IntegrationTransport::AcpStdio,
            default_binary: "agy".into(),
            supports_session_resume: true,
            supports_permission_gate: true,
            supports_verified_process_cancel: true,
            qualification_note: "ACP JSON-RPC 2.0 stdio + Wispral Connect MCP native integration"
                .into(),
        },
        AgentCapabilityDescriptor {
            id: AgentTargetId::Codex,
            display_name: "Codex".into(),
            transport: IntegrationTransport::AcpStdio,
            default_binary: "codex-acp".into(),
            supports_session_resume: true,
            supports_permission_gate: true,
            supports_verified_process_cancel: true,
            qualification_note:
                "000A ACP v1 handshake observed (@agentclientprotocol/codex-acp); session/prompt auth-gated"
                    .into(),
        },
        AgentCapabilityDescriptor {
            id: AgentTargetId::GeminiCli,
            display_name: "Gemini CLI".into(),
            transport: IntegrationTransport::AcpStdio,
            default_binary: "gemini".into(),
            supports_session_resume: true,
            supports_permission_gate: true,
            supports_verified_process_cancel: true,
            qualification_note:
                "000A ACP v1 handshake observed (`gemini --acp`); session/prompt auth-gated".into(),
        },
        AgentCapabilityDescriptor {
            id: AgentTargetId::ClaudeCode,
            display_name: "Claude Code".into(),
            transport: IntegrationTransport::ManagedCliSubprocess,
            default_binary: "claude".into(),
            supports_session_resume: true,
            supports_permission_gate: true,
            supports_verified_process_cancel: true,
            qualification_note:
                "Managed `claude -p --output-format stream-json --` subprocess adapter".into(),
        },
        AgentCapabilityDescriptor {
            id: AgentTargetId::OpenCode,
            display_name: "OpenCode".into(),
            transport: IntegrationTransport::ManagedCliSubprocess,
            default_binary: "opencode".into(),
            supports_session_resume: true,
            supports_permission_gate: true,
            supports_verified_process_cancel: true,
            qualification_note: "Managed `opencode run --format json --` subprocess adapter"
                .into(),
        },
        AgentCapabilityDescriptor {
            id: AgentTargetId::Cursor,
            display_name: "Cursor".into(),
            transport: IntegrationTransport::TargetBoundIdeBridge,
            default_binary: "cursor".into(),
            supports_session_resume: false,
            supports_permission_gate: true,
            supports_verified_process_cancel: false,
            qualification_note: "Target-bound UIA composer delivery + Wispral Connect MCP server"
                .into(),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentPromptEnvelope {
    pub target: AgentTargetId,
    pub sanitized_prompt: String,
    pub matched_workspace_files: Vec<String>,
    pub matched_workspace_symbols: Vec<String>,
    pub inferred_action_class: String,
    pub policy_decision: String,
    pub acp_jsonrpc_frame: Option<String>,
    pub cli_argv: Vec<String>,
}

fn infer_agent_action_class(prompt: &str) -> ActionClass {
    let lower = prompt.to_ascii_lowercase();
    let en_tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|t| !t.is_empty())
        .collect();
    let has_en_destructive_tok = en_tokens.iter().any(|t| {
        matches!(
            *t,
            "delete"
                | "drop"
                | "truncate"
                | "wipe"
                | "destroy"
                | "purge"
                | "rm"
                | "rmdir"
                | "del"
                | "remove-item"
        )
    });
    let has_en_destructive_phrase = [
        "push --force",
        "push -f",
        "force push",
        "force-with-lease",
        "reset --hard",
        "clean -f",
        "clean -d",
    ]
    .iter()
    .any(|p| lower.contains(p));

    let ar_tokens = tokenize_proclitic_aware(prompt);
    let has_ar_destructive = ar_tokens.iter().any(|tok| {
        matches!(
            tok.stem_key.as_key_str(),
            "احذف" | "حذف" | "امسح" | "مسح" | "تدمير" | "فرمت" | "ازل" | "ازاله"
        ) || matches!(
            tok.match_key.as_key_str(),
            "احذف" | "حذف" | "امسح" | "مسح" | "تدمير" | "فرمت" | "ازل" | "ازاله"
        )
    }) || {
        let folded = NormalizedMatchKey::from_surface(prompt, NormalizationLevel::FullMatchFold);
        folded
            .as_key_str()
            .split_whitespace()
            .any(|w| matches!(w, "احذف" | "حذف" | "امسح" | "مسح" | "تدمير" | "فرمت"))
    };

    if has_en_destructive_tok || has_en_destructive_phrase || has_ar_destructive {
        return ActionClass::IrreversibleWrite;
    }

    let has_en_read = en_tokens.iter().any(|t| {
        matches!(
            *t,
            "inspect" | "explain" | "review" | "search" | "summarize" | "read"
        )
    });
    let has_ar_read = ar_tokens.iter().any(|tok| {
        matches!(
            tok.stem_key.as_key_str(),
            "اشرح" | "راجع" | "ابحث" | "افحص" | "اقرا"
        ) || matches!(
            tok.match_key.as_key_str(),
            "اشرح" | "راجع" | "ابحث" | "افحص" | "اقرا"
        )
    });
    if has_en_read || has_ar_read {
        ActionClass::Read
    } else {
        ActionClass::ReversibleWrite
    }
}

/// Builds a secret-redacted, workspace-grounded, policy-gated agent dispatch payload
/// without invoking a shell.
pub fn prepare_agent_dispatch(
    target: AgentTargetId,
    spoken_prompt: &str,
    workspace_root: Option<&Path>,
    user_confirmed_high_risk: bool,
) -> Result<AgentPromptEnvelope, String> {
    let sanitized = redact_sensitive_text(spoken_prompt.trim());
    if sanitized.is_empty() {
        return Err("agent prompt is empty".into());
    }

    let mut matched_files = Vec::new();
    let mut matched_symbols = Vec::new();
    if let Some(root) = workspace_root {
        let vocab = scan_workspace_vocabulary(root, 128);
        let lower = sanitized.to_ascii_lowercase();
        for f in vocab.file_stems {
            if f.len() >= 3 && lower.contains(&f.to_ascii_lowercase()) {
                matched_files.push(f);
            }
        }
        for sym in vocab.symbols {
            if sym.len() >= 4 && lower.contains(&sym.to_ascii_lowercase()) {
                matched_symbols.push(sym);
                if matched_symbols.len() >= 8 {
                    break;
                }
            }
        }
    }

    let action_class = infer_agent_action_class(&sanitized);
    let base_decision = match action_class {
        ActionClass::IrreversibleWrite => CandidateDecision::Ask,
        _ => CandidateDecision::Allow,
    };
    let verdict = evaluate_verdict(&EnforcementInputs {
        action_class,
        layers: &[PolicyLayer {
            source: LayerSource::Product,
            decision: base_decision,
        }],
        kernel_floor_ids: &[],
        rule_ids: &["wisprside_agent_voice_gate".into()],
        human_present: true,
        undecidable_resolution: UndecidableResolution::Ask,
        has_valid_confirmation: user_confirmed_high_risk,
        has_stock_voice_catalog: false,
        has_consent_evidence: false,
        has_target_binding: true,
    })
    .map_err(|e| e.to_string())?;

    let decision_str = match verdict.enforced_decision {
        EnforcedDecision::Allow => "ALLOW",
        EnforcedDecision::Ask => "ASK",
        EnforcedDecision::Deny => "DENY",
    }
    .to_owned();

    let cap = list_agent_capabilities()
        .into_iter()
        .find(|c| c.id == target)
        .ok_or_else(|| "unknown agent target".to_owned())?;

    let acp_jsonrpc_frame = if cap.transport == IntegrationTransport::AcpStdio {
        let seq = ACP_REQ_SEQ.fetch_add(1, Ordering::Relaxed);
        Some(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": format!("req-wisprside-{seq}"),
                "method": "session/prompt",
                "params": {
                    "prompt": [{ "type": "text", "text": sanitized }],
                    "context": {
                        "matchedFiles": matched_files,
                        "matchedSymbols": matched_symbols
                    }
                }
            })
            .to_string(),
        )
    } else {
        None
    };

    let cli_argv = match target {
        AgentTargetId::Antigravity => vec!["agy".into(), "acp".into()],
        AgentTargetId::Codex => vec!["codex-acp".into()],
        AgentTargetId::GeminiCli => vec!["gemini".into(), "--acp".into()],
        AgentTargetId::ClaudeCode => vec![
            "claude".into(),
            "-p".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--".into(),
            sanitized.clone(),
        ],
        AgentTargetId::OpenCode => vec![
            "opencode".into(),
            "run".into(),
            "--format".into(),
            "json".into(),
            "--".into(),
            sanitized.clone(),
        ],
        AgentTargetId::Cursor => vec!["cursor".into()],
    };

    Ok(AgentPromptEnvelope {
        target,
        sanitized_prompt: sanitized,
        matched_workspace_files: matched_files,
        matched_workspace_symbols: matched_symbols,
        inferred_action_class: format!("{action_class:?}").to_ascii_lowercase(),
        policy_decision: decision_str,
        acp_jsonrpc_frame,
        cli_argv,
    })
}

/// Builds a contract-conformant ACP 5.0 cancellation record (`cancellation.schema.json`).
/// Never releases leases on `Acknowledged` alone (`lease_released = false` until observed stop).
pub fn build_verified_cancellation_record(
    operation_id: &str,
    epoch: u64,
    pid: u32,
    start_generation: &str,
    process_exited: bool,
) -> CancellationRecord {
    if process_exited {
        CancellationRecord {
            operation_id: operation_id.to_owned(),
            epoch,
            cancel_epoch: epoch + 1,
            state: CancellationState::ObservedStopped,
            owner: "wisprside_agent_supervisor".into(),
            process_instance: Some(ProcessInstance {
                pid: pid as u64,
                start_generation: start_generation.to_owned(),
            }),
            observation: "process_exit".into(),
            effect_state: "none".into(),
            reconcile_intent_id: None,
            response: "cancelled_result".into(),
            lease_released: true,
        }
    } else {
        CancellationRecord {
            operation_id: operation_id.to_owned(),
            epoch,
            cancel_epoch: epoch + 1,
            state: CancellationState::Acknowledged,
            owner: "wisprside_agent_supervisor".into(),
            process_instance: Some(ProcessInstance {
                pid: pid as u64,
                start_generation: start_generation.to_owned(),
            }),
            observation: "none".into(),
            effect_state: "none".into(),
            reconcile_intent_id: None,
            response: "pending".into(),
            lease_released: false,
        }
    }
}

/// Serializes a [`CancellationRecord`] to strict contract JSON (omitting optional `None` fields
/// rather than emitting explicit `null`).
pub fn cancellation_record_to_json(record: &CancellationRecord) -> serde_json::Value {
    let mut val =
        serde_json::to_value(record).unwrap_or(serde_json::Value::Object(Default::default()));
    if let serde_json::Value::Object(ref mut map) = val {
        map.retain(|_, v| !v.is_null());
    }
    val
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_dispatch_redacts_secrets_gates_destructive_commands_and_validates_acp_cancel() {
        let all = list_agent_capabilities();
        assert_eq!(all.len(), 6);

        // Destructive prompt without confirmation -> ASK (not ALLOW!) + secret redacted
        let env = prepare_agent_dispatch(
            AgentTargetId::ClaudeCode,
            "delete production branch using token ghp_supersecret123456",
            None,
            false,
        )
        .unwrap();
        assert_eq!(env.policy_decision, "ASK");
        assert!(!env.sanitized_prompt.contains("ghp_supersecret"));
        assert!(env.sanitized_prompt.contains("[REDACTED_SECRET]"));
        assert!(env.cli_argv.contains(&"--".to_owned()));

        // CLI option injection (`--dangerously-skip-permissions`) is isolated behind `--`
        let flag_env = prepare_agent_dispatch(
            AgentTargetId::ClaudeCode,
            "--dangerously-skip-permissions inspect paste_tx",
            None,
            false,
        )
        .unwrap();
        let dash_pos = flag_env.cli_argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(dash_pos + 1, flag_env.cli_argv.len() - 1);

        // Variants like `git push --force` and vocalized `احذف نهائياً` trigger ASK
        for risky in [
            "git push --force origin main",
            "rm -fr ./dist",
            "احذف نهائياً",
        ] {
            let r = prepare_agent_dispatch(AgentTargetId::OpenCode, risky, None, false).unwrap();
            assert_eq!(r.policy_decision, "ASK", "failed for {risky}");
        }
        let confirmed =
            prepare_agent_dispatch(AgentTargetId::OpenCode, "احذف نهائياً", None, true).unwrap();
        assert_eq!(confirmed.policy_decision, "ALLOW");

        // Confirmed -> ALLOW; ACP target produces JSON-RPC 2.0 frame with unique IDs
        let acp1 = prepare_agent_dispatch(
            AgentTargetId::Antigravity,
            "inspect target binding in paste_tx",
            None,
            false,
        )
        .unwrap();
        let acp2 = prepare_agent_dispatch(
            AgentTargetId::Antigravity,
            "inspect target binding in paste_tx",
            None,
            false,
        )
        .unwrap();
        assert_eq!(acp1.policy_decision, "ALLOW");
        assert!(acp1
            .acp_jsonrpc_frame
            .as_deref()
            .unwrap()
            .contains("\"session/prompt\""));
        assert_ne!(acp1.acp_jsonrpc_frame, acp2.acp_jsonrpc_frame);

        // ACP 5.0 cancellation conforms to `cancellation.schema.json`
        let ack = build_verified_cancellation_record("op-agent-01", 2, 4242, "13370000", false);
        assert!(!ack.lease_released);
        wispral_contracts::validate_shape("cancellation", &cancellation_record_to_json(&ack))
            .unwrap();

        let stopped = build_verified_cancellation_record("op-agent-01", 2, 4242, "13370000", true);
        assert!(stopped.lease_released);
        wispral_contracts::validate_shape("cancellation", &cancellation_record_to_json(&stopped))
            .unwrap();
    }
}
