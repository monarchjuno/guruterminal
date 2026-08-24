import {
  BookOpenIcon,
  BotIcon,
  BrainCircuitIcon,
  MessageSquareIcon,
  PaletteIcon,
  RefreshCwIcon,
  Settings2Icon,
  StoreIcon,
} from "lucide-react";
import type { ComponentType } from "react";

export type AppTab =
  | "chat"
  | "library"
  | "agents"
  | "marketplace"
  | "settings";

export type AppTabGroup = "workspace" | "ecosystem" | "footer";
export type SettingsSection = "model" | "appearance" | "updates";

type AppTabDefinition = {
  id: AppTab;
  label: string;
  group: AppTabGroup;
  guruScoped: boolean;
  icon: ComponentType<{ className?: string }>;
};

type SettingsSectionDefinition = {
  id: SettingsSection;
  label: string;
  icon: ComponentType<{ className?: string }>;
};

const APP_TABS: readonly AppTabDefinition[] = [
  {
    id: "chat",
    label: "Chat",
    group: "workspace",
    guruScoped: true,
    icon: MessageSquareIcon,
  },
  {
    id: "library",
    label: "Memory",
    group: "workspace",
    guruScoped: true,
    icon: BookOpenIcon,
  },
  {
    id: "agents",
    label: "Agents",
    group: "ecosystem",
    guruScoped: false,
    icon: BotIcon,
  },
  {
    id: "marketplace",
    label: "Marketplace",
    group: "ecosystem",
    guruScoped: false,
    icon: StoreIcon,
  },
  {
    id: "settings",
    label: "Settings",
    group: "footer",
    guruScoped: false,
    icon: Settings2Icon,
  },
];

export const SETTINGS_SECTIONS: readonly SettingsSectionDefinition[] = [
  { id: "model", label: "Model", icon: BrainCircuitIcon },
  { id: "appearance", label: "Appearance", icon: PaletteIcon },
  { id: "updates", label: "Updates", icon: RefreshCwIcon },
];

export const appTabsInGroup = (group: AppTabGroup) =>
  APP_TABS.filter((item) => item.group === group);

export const appTabDefinition = (tab: AppTab) =>
  APP_TABS.find((item) => item.id === tab) ?? APP_TABS[0];
