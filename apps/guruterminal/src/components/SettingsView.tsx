import type { ThemePreference } from "../theme";
import type { SettingsSection } from "../navigation";
import type {
  ModelCatalog,
  ModelVisibilityUpdateRequest,
  ProviderConfigureRequest,
  ProviderConnectionEvent,
} from "../types";
import { AppearanceSettings } from "./settings/AppearanceSettings";
import { ModelSettingsPanel } from "./settings/ModelSettingsPanel";
import { UpdateSettingsCard } from "./settings/UpdateSettingsCard";
import type { UpdatePhase, UpdateState } from "../types";

type Props = {
  section: SettingsSection;
  catalog: ModelCatalog | null;
  theme: ThemePreference;
  loadError: string | null;
  updateResult: UpdateState | null;
  updatePhase: UpdatePhase;
  updateError: string | null;
  onThemeChange: (theme: ThemePreference) => void;
  onLoadModels: (provider: string) => Promise<ModelCatalog>;
  onConnect: (
    provider: string,
    observer: (event: ProviderConnectionEvent) => void,
  ) => Promise<ModelCatalog>;
  onCancelConnect: () => Promise<void>;
  onOpenConnectBrowser: () => Promise<void>;
  onConfigure: (request: ProviderConfigureRequest) => Promise<ModelCatalog>;
  onUpdateModelVisibility: (
    request: ModelVisibilityUpdateRequest,
  ) => Promise<ModelCatalog>;
  onDisconnect: (provider: string) => Promise<ModelCatalog>;
  onCheckForUpdates: () => Promise<void>;
  onInstallUpdate: (offerId: string) => Promise<void>;
};

export function SettingsView({
  section,
  catalog,
  theme,
  loadError,
  updateResult,
  updatePhase,
  updateError,
  onThemeChange,
  onLoadModels,
  onConnect,
  onCancelConnect,
  onOpenConnectBrowser,
  onConfigure,
  onUpdateModelVisibility,
  onDisconnect,
  onCheckForUpdates,
  onInstallUpdate,
}: Props) {
  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <div className="settings-heading">
        <div>
          <h1 id="settings-title">Settings</h1>
          <p>Providers, appearance, and updates.</p>
        </div>
      </div>

      <div className="settings-stack">
        {section === "model" ? (
          <ModelSettingsPanel
            catalog={catalog}
            loadError={loadError}
            onLoadModels={onLoadModels}
            onConnect={onConnect}
            onCancelConnect={onCancelConnect}
            onOpenConnectBrowser={onOpenConnectBrowser}
            onConfigure={onConfigure}
            onUpdateModelVisibility={onUpdateModelVisibility}
            onDisconnect={onDisconnect}
          />
        ) : null}

        {section === "appearance" ? (
          <AppearanceSettings theme={theme} onThemeChange={onThemeChange} />
        ) : null}

        {section === "updates" ? (
          <UpdateSettingsCard
            status={updateResult}
            phase={updatePhase}
            error={updateError}
            onCheck={onCheckForUpdates}
            onInstall={onInstallUpdate}
          />
        ) : null}
      </div>
    </section>
  );
}
