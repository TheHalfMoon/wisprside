import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type AgentCapabilityDescriptor,
  type AgentPromptEnvelope,
  type AgentTargetId,
  type AppCategory,
  type AppProfile,
  type MemoryRetentionMode,
  type ModeExecutionResult,
  type PersonalMemorySnapshot,
  type WritingMode,
} from "@/bindings";
import { SettingsGroup } from "../ui/SettingsGroup";
import { SettingContainer } from "../ui/SettingContainer";
import { Button } from "../ui/Button";

const WRITING_MODES: WritingMode[] = [
  "dictation",
  "compose",
  "edit",
  "translate",
  "command",
  "aside",
];

const ARROW = " \u2192 ";

export const IntelligenceModes: React.FC = () => {
  const { t } = useTranslation();
  const [selectedMode, setSelectedMode] = useState<WritingMode>("dictation");
  const [selectedCategory, setSelectedCategory] = useState<AppCategory>("ide");
  const [spokenInput, setSpokenInput] = useState(
    "افتح مشروع Ecra وسوّ code review للـ pull request الأخير، وبعدها شغّل الاختبارات.",
  );
  const [selectionInput, setSelectionInput] = useState(
    "Please do not merge #160 without very careful --release checks.",
  );
  const [targetLang, setTargetLang] = useState("en");
  const [modeResult, setModeResult] = useState<ModeExecutionResult | null>(
    null,
  );
  const [modeError, setModeError] = useState<string | null>(null);

  const [profiles, setProfiles] = useState<AppProfile[]>([]);
  const [agents, setAgents] = useState<AgentCapabilityDescriptor[]>([]);
  const [agentTarget, setAgentTarget] = useState<AgentTargetId>("antigravity");
  const [agentPrompt, setAgentPrompt] = useState(
    "inspect target binding in paste_tx",
  );
  const [confirmHighRisk, setConfirmHighRisk] = useState(false);
  const [agentEnvelope, setAgentEnvelope] =
    useState<AgentPromptEnvelope | null>(null);
  const [agentError, setAgentError] = useState<string | null>(null);

  const [memory, setMemory] = useState<PersonalMemorySnapshot | null>(null);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [snippetTrigger, setSnippetTrigger] = useState("");
  const [snippetExpansion, setSnippetExpansion] = useState("");

  useEffect(() => {
    void (async () => {
      try {
        const [s, p, a, m] = await Promise.all([
          commands.getAppSettings(),
          commands.listAppProfilesCmd(),
          commands.listAgentCapabilitiesCmd(),
          commands.getMemoryBundleCmd(),
        ]);
        if (s.status === "ok") {
          if (s.data.active_writing_mode) {
            setSelectedMode(s.data.active_writing_mode);
          }
          if (s.data.translation_target_language) {
            setTargetLang(s.data.translation_target_language);
          }
        }
        setProfiles(p);
        setAgents(a);
        if (m.status === "ok") {
          setMemory(m.data);
        } else {
          setMemoryError(m.error);
        }
      } catch {
        // Browser-only dev mode outside Tauri IPC
      }
    })();
  }, []);

  const handleSelectMode = async (mode: WritingMode) => {
    setSelectedMode(mode);
    setModeError(null);
    try {
      await commands.setActiveWritingModeCmd(mode, targetLang);
    } catch {
      // Ignore outside Tauri runtime
    }
  };

  const handleTargetLangChange = async (lang: string) => {
    setTargetLang(lang);
    try {
      await commands.setActiveWritingModeCmd(selectedMode, lang);
    } catch {
      // Ignore outside Tauri runtime
    }
  };

  const activeTone =
    profiles.find((p) => p.category === selectedCategory)?.tone ??
    "technical_code_aware";

  const selectedAgentInfo = agents.find((a) => a.id === agentTarget) ?? null;

  const handleRunMode = async () => {
    setModeError(null);
    const res = await commands.executeWritingModeCmd({
      mode: selectedMode,
      spoken_transcript: spokenInput,
      selected_text: selectedMode === "edit" ? selectionInput : null,
      target_language: selectedMode === "translate" ? targetLang : null,
      app_category: selectedCategory,
      tone: activeTone,
    });
    if (res.status === "ok") {
      setModeResult(res.data);
      if (selectedMode === "aside") {
        const m = await commands.getMemoryBundleCmd();
        if (m.status === "ok") setMemory(m.data);
      }
    } else {
      setModeError(res.error);
    }
  };

  const handlePreviewAgent = async () => {
    setAgentError(null);
    const res = await commands.prepareAgentDispatchCmd(
      agentTarget,
      agentPrompt,
      null,
      confirmHighRisk,
    );
    if (res.status === "ok") {
      setAgentEnvelope(res.data);
    } else {
      setAgentError(res.error);
    }
  };

  const handleRetentionChange = async (retention: MemoryRetentionMode) => {
    setMemoryError(null);
    const res = await commands.setMemoryPrivacyCmd(
      retention,
      retention === "keep_nothing" ? false : (memory?.keep_audio ?? false),
    );
    if (res.status === "ok") {
      setMemory(res.data);
    } else {
      setMemoryError(res.error);
    }
  };

  const handleKeepAudioChange = async (keepAudio: boolean) => {
    setMemoryError(null);
    const retention = memory?.retention_mode ?? "bounded7_days";
    const res = await commands.setMemoryPrivacyCmd(retention, keepAudio);
    if (res.status === "ok") {
      setMemory(res.data);
    } else {
      setMemoryError(res.error);
    }
  };

  const handleAddSnippet = async () => {
    if (!snippetTrigger.trim() || !snippetExpansion.trim()) return;
    setMemoryError(null);
    const res = await commands.upsertMemorySnippetCmd(
      snippetTrigger,
      snippetExpansion,
    );
    if (res.status === "ok") {
      setMemory(res.data);
      setSnippetTrigger("");
      setSnippetExpansion("");
    } else {
      setMemoryError(res.error);
    }
  };

  const handleWipeMemory = async () => {
    setMemoryError(null);
    const res = await commands.wipePersonalMemoryCmd();
    if (res.status === "ok") {
      setMemory(res.data);
    } else {
      setMemoryError(res.error);
    }
  };

  return (
    <div
      className="max-w-3xl w-full mx-auto space-y-6"
      data-testid="wisprside-intelligence-panel"
    >
      <SettingsGroup title={t("settings.intelligence.modes.title")}>
        <SettingContainer
          title={t("settings.intelligence.modes.selectorTitle")}
          description={t("settings.intelligence.modes.selectorDescription")}
          grouped={true}
          layout="stacked"
        >
          <div className="flex flex-wrap gap-2">
            {WRITING_MODES.map((m) => (
              <Button
                key={m}
                data-testid={`mode-btn-${m}`}
                variant={selectedMode === m ? "primary" : "secondary"}
                size="sm"
                onClick={() => void handleSelectMode(m)}
              >
                {t(`settings.intelligence.modes.items.${m}`)}
              </Button>
            ))}
          </div>
          <div className="mt-3 space-y-2">
            <select
              aria-label={t("settings.intelligence.profiles.title")}
              className="w-full rounded border border-line bg-transparent px-2 py-1 text-sm"
              value={selectedCategory}
              onChange={(e) =>
                setSelectedCategory(e.target.value as AppCategory)
              }
            >
              <option value="ide">
                {t("settings.intelligence.profiles.ide")}
              </option>
              <option value="terminal">
                {t("settings.intelligence.profiles.terminal")}
              </option>
              <option value="email">
                {t("settings.intelligence.profiles.email")}
              </option>
              <option value="chat">
                {t("settings.intelligence.profiles.chat")}
              </option>
              <option value="notes">
                {t("settings.intelligence.profiles.notes")}
              </option>
              <option value="document">
                {t("settings.intelligence.profiles.document")}
              </option>
              <option value="browser">
                {t("settings.intelligence.profiles.browser")}
              </option>
              <option value="business_app">
                {t("settings.intelligence.profiles.business_app")}
              </option>
            </select>
            <textarea
              aria-label={t("settings.intelligence.modes.spokenLabel")}
              dir="auto"
              rows={2}
              className="w-full rounded border border-line bg-transparent p-2 text-sm"
              value={spokenInput}
              onChange={(e) => setSpokenInput(e.target.value)}
            />
            {selectedMode === "edit" && (
              <textarea
                aria-label={t("settings.intelligence.modes.selectionLabel")}
                dir="auto"
                rows={2}
                className="w-full rounded border border-line bg-transparent p-2 text-sm"
                value={selectionInput}
                onChange={(e) => setSelectionInput(e.target.value)}
              />
            )}
            {selectedMode === "translate" && (
              <select
                aria-label={t("settings.intelligence.modes.targetLanguage")}
                className="rounded border border-line bg-transparent px-2 py-1 text-sm"
                value={targetLang}
                onChange={(e) => void handleTargetLangChange(e.target.value)}
              >
                <option value="en">
                  {t("settings.intelligence.modes.langEn")}
                </option>
                <option value="ar">
                  {t("settings.intelligence.modes.langAr")}
                </option>
              </select>
            )}
            <div>
              <Button
                data-testid="run-mode-preview-btn"
                variant="primary"
                size="sm"
                onClick={handleRunMode}
              >
                {t("settings.intelligence.modes.runPreview")}
              </Button>
            </div>
            {modeError && <p className="text-xs text-error">{modeError}</p>}
            {modeResult && (
              <div
                data-testid="mode-preview-output"
                dir="auto"
                className="rounded border border-line p-2 text-sm font-mono whitespace-pre-wrap space-y-1"
              >
                <div className="flex flex-wrap gap-2 text-xs text-ink-3">
                  <span>
                    {modeResult.delivers_to_target
                      ? t("settings.intelligence.modes.deliversBadge")
                      : t("settings.intelligence.modes.noDeliveryBadge")}
                  </span>
                  {modeResult.requires_preview_confirmation && (
                    <span>
                      {t("settings.intelligence.modes.previewRequiredBadge")}
                    </span>
                  )}
                  {modeResult.safety_notes.map((n) => (
                    <span key={n}>{n}</span>
                  ))}
                </div>
                {modeResult.bidi_display_text && (
                  <div>{modeResult.bidi_display_text}</div>
                )}
                {modeResult.structured_command && (
                  <div data-testid="mode-structured-command">
                    {JSON.stringify(modeResult.structured_command)}
                  </div>
                )}
              </div>
            )}
          </div>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("settings.intelligence.profiles.title")}>
        <SettingContainer
          title={t("settings.intelligence.profiles.safetyTitle")}
          description={t("settings.intelligence.profiles.safetyDescription")}
          grouped={true}
          layout="stacked"
        >
          <div className="grid grid-cols-2 gap-2 text-xs">
            {profiles.map((p) => (
              <div
                key={p.category}
                data-testid={`profile-card-${p.category}`}
                className="rounded border border-line p-2 flex justify-between items-center"
              >
                <span className="font-medium">
                  {t(`settings.intelligence.profiles.${p.category}`)}
                </span>
                <span className="text-ink-3">
                  {p.allow_auto_submit
                    ? t("settings.intelligence.profiles.autoSubmitAllowed")
                    : t("settings.intelligence.profiles.autoSubmitBlocked")}
                </span>
              </div>
            ))}
          </div>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("settings.intelligence.agents.title")}>
        <SettingContainer
          title={t("settings.intelligence.agents.dispatchTitle")}
          description={t("settings.intelligence.agents.dispatchDescription")}
          grouped={true}
          layout="stacked"
        >
          <div className="space-y-2">
            <select
              aria-label={t("settings.intelligence.agents.title")}
              className="w-full rounded border border-line bg-transparent px-2 py-1 text-sm"
              value={agentTarget}
              onChange={(e) => setAgentTarget(e.target.value as AgentTargetId)}
            >
              {agents.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.display_name}
                </option>
              ))}
            </select>
            {selectedAgentInfo && (
              <div
                data-testid="agent-capability-meta"
                className="text-xs text-ink-3 space-y-0.5"
              >
                <div>{selectedAgentInfo.transport}</div>
                <div>{selectedAgentInfo.qualification_note}</div>
              </div>
            )}
            <input
              aria-label={t("settings.intelligence.agents.promptLabel")}
              dir="auto"
              className="w-full rounded border border-line bg-transparent px-2 py-1 text-sm"
              value={agentPrompt}
              onChange={(e) => setAgentPrompt(e.target.value)}
            />
            <label className="flex items-center gap-2 text-xs">
              <input
                type="checkbox"
                data-testid="agent-confirm-high-risk"
                checked={confirmHighRisk}
                onChange={(e) => setConfirmHighRisk(e.target.checked)}
              />
              <span>{t("settings.intelligence.agents.confirmHighRisk")}</span>
            </label>
            <Button
              data-testid="inspect-agent-envelope-btn"
              variant="secondary"
              size="sm"
              onClick={handlePreviewAgent}
            >
              {t("settings.intelligence.agents.inspectEnvelope")}
            </Button>
            {agentError && <p className="text-xs text-error">{agentError}</p>}
            {agentEnvelope && (
              <div
                data-testid="agent-envelope-output"
                className="rounded border border-line p-2 text-xs font-mono space-y-1"
              >
                <div>{agentEnvelope.policy_decision}</div>
                <div>{agentEnvelope.inferred_action_class}</div>
                <div>{agentEnvelope.sanitized_prompt}</div>
                {agentEnvelope.acp_jsonrpc_frame && (
                  <div className="break-all">
                    {agentEnvelope.acp_jsonrpc_frame}
                  </div>
                )}
              </div>
            )}
          </div>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("settings.intelligence.memory.title")}>
        <SettingContainer
          title={t("settings.intelligence.memory.privacyTitle")}
          description={t("settings.intelligence.memory.privacyDescription")}
          grouped={true}
          layout="stacked"
        >
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-4">
              <select
                aria-label={t("settings.intelligence.memory.privacyTitle")}
                data-testid="memory-retention-select"
                className="rounded border border-line bg-transparent px-2 py-1 text-sm"
                value={memory?.retention_mode ?? "bounded7_days"}
                onChange={(e) =>
                  void handleRetentionChange(
                    e.target.value as MemoryRetentionMode,
                  )
                }
              >
                <option value="keep_nothing">
                  {t("settings.intelligence.memory.keepNothing")}
                </option>
                <option value="session_only">
                  {t("settings.intelligence.memory.sessionOnly")}
                </option>
                <option value="bounded7_days">
                  {t("settings.intelligence.memory.bounded7Days")}
                </option>
                <option value="bounded30_days">
                  {t("settings.intelligence.memory.bounded30Days")}
                </option>
                <option value="user_controlled">
                  {t("settings.intelligence.memory.userControlled")}
                </option>
              </select>
              <label className="flex items-center gap-2 text-xs">
                <input
                  type="checkbox"
                  data-testid="memory-keep-audio-checkbox"
                  disabled={memory?.retention_mode === "keep_nothing"}
                  checked={memory?.keep_audio ?? false}
                  onChange={(e) => void handleKeepAudioChange(e.target.checked)}
                />
                <span>{t("settings.intelligence.memory.keepAudioLabel")}</span>
              </label>
            </div>
            <div className="flex gap-2">
              <input
                aria-label={t(
                  "settings.intelligence.memory.triggerPlaceholder",
                )}
                data-testid="snippet-trigger-input"
                dir="auto"
                placeholder={t(
                  "settings.intelligence.memory.triggerPlaceholder",
                )}
                className="flex-1 rounded border border-line bg-transparent px-2 py-1 text-sm"
                value={snippetTrigger}
                onChange={(e) => setSnippetTrigger(e.target.value)}
              />
              <input
                aria-label={t(
                  "settings.intelligence.memory.expansionPlaceholder",
                )}
                data-testid="snippet-expansion-input"
                dir="auto"
                placeholder={t(
                  "settings.intelligence.memory.expansionPlaceholder",
                )}
                className="flex-1 rounded border border-line bg-transparent px-2 py-1 text-sm"
                value={snippetExpansion}
                onChange={(e) => setSnippetExpansion(e.target.value)}
              />
              <Button
                data-testid="save-snippet-btn"
                variant="secondary"
                size="sm"
                onClick={handleAddSnippet}
              >
                {t("settings.intelligence.memory.addSnippet")}
              </Button>
            </div>
            {memoryError && <p className="text-xs text-error">{memoryError}</p>}
            <div
              className="space-y-1 text-xs"
              data-testid="memory-snippets-list"
            >
              <div className="font-medium">
                {t("settings.intelligence.memory.snippetsHeading")}
              </div>
              {memory && memory.snippets.length > 0 ? (
                memory.snippets.map((s) => (
                  <div
                    key={s.id}
                    dir="auto"
                    className="rounded border border-line px-2 py-1"
                  >
                    {`${s.trigger}${ARROW}${s.expansion}`}
                  </div>
                ))
              ) : (
                <div className="text-ink-3">
                  {t("settings.intelligence.memory.emptySnippets")}
                </div>
              )}
            </div>
            <div className="space-y-1 text-xs" data-testid="memory-aside-list">
              <div className="font-medium">
                {t("settings.intelligence.memory.asideHeading")}
              </div>
              {memory && memory.aside_notes.length > 0 ? (
                memory.aside_notes.map((n) => (
                  <div
                    key={n.id}
                    dir="auto"
                    className="rounded border border-line px-2 py-1"
                  >
                    {n.text}
                  </div>
                ))
              ) : (
                <div className="text-ink-3">
                  {t("settings.intelligence.memory.emptyAside")}
                </div>
              )}
            </div>
            <div>
              <Button
                data-testid="wipe-memory-btn"
                variant="secondary"
                size="sm"
                onClick={handleWipeMemory}
              >
                {t("settings.intelligence.memory.wipeAll")}
              </Button>
            </div>
          </div>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
