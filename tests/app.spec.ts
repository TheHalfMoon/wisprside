import { test, expect } from "@playwright/test";

test.describe("WisprSide App", () => {
  test("dev server responds", async ({ page }) => {
    const response = await page.goto("/");
    expect(response?.status()).toBe(200);
  });

  test("page has html structure", async ({ page }) => {
    await page.goto("/");
    const html = await page.content();
    expect(html).toContain("<html");
    expect(html).toContain("<body");
  });

  test("intelligence panel renders writing modes, profiles, agent gating, and personal memory", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      let activeMode = "dictation";
      let targetLanguage = "en";
      let retentionMode = "bounded7_days";
      let keepAudio = false;
      const snippets: Array<{
        id: string;
        trigger: string;
        expansion: string;
        created_at_ms: number;
      }> = [];
      const asideNotes: Array<{
        id: string;
        text: string;
        app_name: string | null;
        created_at_ms: number;
      }> = [];

      const profiles = [
        {
          category: "terminal",
          tone: "technical_code_aware",
          preserve_code_identifiers: true,
          smart_list_formatting: false,
          allow_auto_submit: false,
          preferred_mode: "dictation",
        },
        {
          category: "ide",
          tone: "technical_code_aware",
          preserve_code_identifiers: true,
          smart_list_formatting: true,
          allow_auto_submit: true,
          preferred_mode: "dictation",
        },
        {
          category: "email",
          tone: "professional",
          preserve_code_identifiers: true,
          smart_list_formatting: true,
          allow_auto_submit: true,
          preferred_mode: "compose",
        },
      ];

      const agents = [
        {
          id: "antigravity",
          display_name: "Antigravity",
          transport: "acp_stdio",
          default_binary: "agy",
          supports_session_resume: true,
          supports_permission_gate: true,
          supports_verified_process_cancel: true,
          qualification_note:
            "ACP JSON-RPC 2.0 stdio + Wispral Connect MCP native integration",
        },
        {
          id: "claude_code",
          display_name: "Claude Code",
          transport: "managed_cli_subprocess",
          default_binary: "claude",
          supports_session_resume: true,
          supports_permission_gate: true,
          supports_verified_process_cancel: true,
          qualification_note:
            "Managed `claude -p --output-format stream-json --` subprocess adapter",
        },
      ];

      (
        window as unknown as Record<string, unknown>
      ).__TAURI_OS_PLUGIN_INTERNALS__ = {
        arch: "x86_64",
        eol: "\n",
        exe_extension: "exe",
        family: "windows",
        os_type: "windows",
        platform: "windows",
        version: "11",
      };

      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: () => 1,
        unregisterCallback: () => {},
        invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
          switch (cmd) {
            case "plugin:os|locale":
              return "en-US";
            case "plugin:event|listen":
            case "plugin:event|unlisten":
              return 1;
            case "get_app_settings":
            case "get_default_settings":
              return {
                onboarding_completed: true,
                selected_model: "parakeet-tdt-0.6b-v3",
                app_language: "en",
                theme: "system",
                whats_new_last_seen_version: "0.10.0",
                show_whats_new_on_update: false,
                active_writing_mode: activeMode,
                translation_target_language: targetLanguage,
                bindings: {},
              };
            case "get_retired_models":
            case "get_active_notices":
            case "get_available_microphones":
            case "get_available_output_devices":
            case "get_available_models":
              return [];
            case "get_windows_microphone_permission_status":
              return { supported: true, overall_access: "allowed" };
            case "get_secure_input_status":
              return { active: false, process_name: null };
            case "initialize_enigo":
            case "initialize_shortcuts":
            case "show_main_window_command":
              return null;
            case "list_app_profiles_cmd":
              return profiles;
            case "list_agent_capabilities_cmd":
              return agents;
            case "set_active_writing_mode_cmd":
              activeMode = String(args.mode ?? activeMode);
              if (args.targetLanguage) {
                targetLanguage = String(args.targetLanguage);
              }
              return null;
            case "execute_writing_mode_cmd": {
              const req = args.request as Record<string, unknown>;
              const m = String(req.mode);
              const spoken = String(req.spoken_transcript ?? "");
              if (m === "aside") {
                asideNotes.push({
                  id: `aside-${asideNotes.length + 1}`,
                  text: spoken,
                  app_name: "Code.exe",
                  created_at_ms: 1700000000000,
                });
                return {
                  mode: "aside",
                  output_text: spoken,
                  bidi_display_text: spoken,
                  delivers_to_target: false,
                  requires_preview_confirmation: false,
                  structured_command: null,
                  safety_notes: ["captured_to_aside_scratchpad_only"],
                };
              }
              if (m === "command") {
                return {
                  mode: "command",
                  output_text: "",
                  bidi_display_text: "",
                  delivers_to_target: false,
                  requires_preview_confirmation: false,
                  structured_command: { action: "scratch_last_sentence" },
                  safety_notes: ["no_shell_exec"],
                };
              }
              return {
                mode: m,
                output_text: spoken,
                bidi_display_text: spoken,
                delivers_to_target: true,
                requires_preview_confirmation: m === "edit",
                structured_command: null,
                safety_notes: [],
              };
            }
            case "prepare_agent_dispatch_cmd": {
              const prompt = String(args.spokenPrompt ?? "");
              const confirmed = Boolean(args.userConfirmedHighRisk);
              const destructive = /delete|--force|احذف/i.test(prompt);
              return {
                target: args.target,
                sanitized_prompt: prompt.replace(
                  /ghp_[A-Za-z0-9]+/g,
                  "[REDACTED_SECRET]",
                ),
                matched_workspace_files: ["paste_tx"],
                matched_workspace_symbols: ["TargetBinding"],
                inferred_action_class: destructive
                  ? "irreversiblewrite"
                  : "read",
                policy_decision: destructive && !confirmed ? "ASK" : "ALLOW",
                acp_jsonrpc_frame:
                  args.target === "antigravity"
                    ? '{"jsonrpc":"2.0","id":"req-wisprside-1","method":"session/prompt"}'
                    : null,
                cli_argv: ["agy", "acp"],
              };
            }
            case "get_memory_bundle_cmd":
              return {
                keep_audio: keepAudio,
                retention_mode: retentionMode,
                snippets,
                aside_notes: asideNotes,
              };
            case "set_memory_privacy_cmd":
              retentionMode = String(args.retentionMode);
              keepAudio =
                retentionMode === "keep_nothing"
                  ? false
                  : Boolean(args.keepAudio);
              return {
                keep_audio: keepAudio,
                retention_mode: retentionMode,
                snippets,
                aside_notes: retentionMode === "keep_nothing" ? [] : asideNotes,
              };
            case "upsert_memory_snippet_cmd":
              snippets.splice(0, snippets.length, {
                id: "sn-1",
                trigger: String(args.trigger),
                expansion: String(args.expansion),
                created_at_ms: 1700000000000,
              });
              return {
                keep_audio: keepAudio,
                retention_mode: retentionMode,
                snippets,
                aside_notes: asideNotes,
              };
            case "wipe_personal_memory_cmd":
              snippets.length = 0;
              asideNotes.length = 0;
              return {
                keep_audio: keepAudio,
                retention_mode: retentionMode,
                snippets: [],
                aside_notes: [],
              };
            default:
              return null;
          }
        },
      };
    });

    await page.goto("/");
    await page.getByText("Intelligence").click();

    const panel = page.getByTestId("wisprside-intelligence-panel");
    await expect(panel).toBeVisible();

    // Terminal blocks auto-submit while IDE allows explicit opt-in
    await expect(page.getByTestId("profile-card-terminal")).toContainText(
      "Auto-submit blocked",
    );
    await expect(page.getByTestId("profile-card-ide")).toContainText(
      "Auto-submit opt-in",
    );

    // Command mode renders structured_command even when bidi_display_text is empty
    await page.getByTestId("mode-btn-command").click();
    await page.getByTestId("run-mode-preview-btn").click();
    await expect(page.getByTestId("mode-structured-command")).toContainText(
      "scratch_last_sentence",
    );

    // Aside mode captures to scratchpad list and never delivers to active target
    await page.getByTestId("mode-btn-aside").click();
    await page.getByTestId("run-mode-preview-btn").click();
    await expect(page.getByTestId("mode-preview-output")).toContainText(
      "Local only",
    );
    await expect(page.getByTestId("memory-aside-list")).toContainText(
      "pull request",
    );

    // Agent capability metadata + ASK -> ALLOW confirmation gate
    await expect(page.getByTestId("agent-capability-meta")).toContainText(
      "acp_stdio",
    );
    await page
      .getByLabel("Spoken agent instruction")
      .fill("delete branch with ghp_secret999");
    await page.getByTestId("inspect-agent-envelope-btn").click();
    await expect(page.getByTestId("agent-envelope-output")).toContainText(
      "ASK",
    );
    await expect(page.getByTestId("agent-envelope-output")).toContainText(
      "[REDACTED_SECRET]",
    );
    await page.getByTestId("agent-confirm-high-risk").check();
    await page.getByTestId("inspect-agent-envelope-btn").click();
    await expect(page.getByTestId("agent-envelope-output")).toContainText(
      "ALLOW",
    );

    // Personal memory: keep_audio defaults to false, saves snippet, and wipes
    await expect(
      page.getByTestId("memory-keep-audio-checkbox"),
    ).not.toBeChecked();
    await page.getByTestId("snippet-trigger-input").fill("توقيع");
    await page
      .getByTestId("snippet-expansion-input")
      .fill("مع خالص التحية — WisprSide");
    await page.getByTestId("save-snippet-btn").click();
    await expect(page.getByTestId("memory-snippets-list")).toContainText(
      "مع خالص التحية — WisprSide",
    );
    await page.getByTestId("wipe-memory-btn").click();
    await expect(page.getByTestId("memory-snippets-list")).toContainText(
      "No voice snippets saved yet.",
    );
  });
});
