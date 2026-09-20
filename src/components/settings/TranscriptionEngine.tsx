import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Dropdown, SettingContainer, SettingsGroup } from "@/components/ui";
import { Input } from "../ui/Input";
import { useSettings } from "../../hooks/useSettings";

/// Providers are shared with post-processing, but only ones reachable over HTTP
/// can serve transcription — Apple Intelligence runs natively and has no
/// endpoint.
const isHttpProvider = (baseUrl: string) => baseUrl.startsWith("http");

const LOCAL = "local";
const CLOUD = "cloud";

/**
 * Picks what actually transcribes the audio: a model on this machine, or a
 * remote OpenAI-compatible endpoint.
 *
 * Lives on the Models page rather than in Advanced so that the choice sits next
 * to the model list it governs — a cloud engine silently overriding a selected
 * local model is the kind of hidden state this selector exists to avoid.
 */
export const TranscriptionEngine: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, refreshSettings } = useSettings();
  const [savingKey, setSavingKey] = useState(false);

  const cloudEnabled = getSetting("cloud_stt_enabled") ?? false;
  const providerId = getSetting("cloud_stt_provider_id") ?? "openrouter";
  const model = getSetting("cloud_stt_model") ?? "";
  const providers = getSetting("post_process_providers") ?? [];
  const apiKeys = getSetting("post_process_api_keys") ?? {};

  const engineOptions = useMemo(
    () => [
      {
        value: LOCAL,
        label: t("settings.transcriptionEngine.engine.local"),
        description: t("settings.transcriptionEngine.engine.localDescription"),
      },
      {
        value: CLOUD,
        label: t("settings.transcriptionEngine.engine.cloud"),
        description: t("settings.transcriptionEngine.engine.cloudDescription"),
      },
    ],
    [t],
  );

  const providerOptions = useMemo(
    () =>
      providers
        .filter((provider) => isHttpProvider(provider.base_url))
        .map((provider) => ({
          value: provider.id,
          label: provider.label,
          description: provider.base_url,
        })),
    [providers],
  );

  const apiKey = apiKeys[providerId] ?? "";

  const handleApiKeyChange = async (value: string) => {
    setSavingKey(true);
    try {
      await commands.changePostProcessApiKeySetting(providerId, value);
      await refreshSettings();
    } finally {
      setSavingKey(false);
    }
  };

  return (
    <SettingsGroup>
      <SettingContainer
        title={t("settings.transcriptionEngine.engine.title")}
        description={t("settings.transcriptionEngine.engine.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <Dropdown
          options={engineOptions}
          selectedValue={cloudEnabled ? CLOUD : LOCAL}
          onSelect={(value) =>
            updateSetting("cloud_stt_enabled", value === CLOUD)
          }
        />
      </SettingContainer>

      {cloudEnabled && (
        <>
          <SettingContainer
            title={t("settings.transcriptionEngine.provider.title")}
            description={t("settings.transcriptionEngine.provider.description")}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <Dropdown
              options={providerOptions}
              selectedValue={providerId}
              onSelect={(value) =>
                updateSetting("cloud_stt_provider_id", value)
              }
            />
          </SettingContainer>

          <SettingContainer
            title={t("settings.transcriptionEngine.apiKey.title")}
            description={t("settings.transcriptionEngine.apiKey.description")}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <Input
              type="password"
              value={apiKey}
              disabled={savingKey}
              placeholder="sk-..."
              onChange={(event) => handleApiKeyChange(event.target.value)}
            />
          </SettingContainer>

          <SettingContainer
            title={t("settings.transcriptionEngine.model.title")}
            description={t("settings.transcriptionEngine.model.description")}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <Input
              value={model}
              placeholder="openai/gpt-transcribe"
              onChange={(event) =>
                updateSetting("cloud_stt_model", event.target.value)
              }
            />
          </SettingContainer>
        </>
      )}
    </SettingsGroup>
  );
});
