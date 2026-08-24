import {
  CheckIcon,
  LaptopIcon,
  MoonIcon,
  SunIcon,
} from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import type { ThemePreference } from "../../theme";

type Props = {
  theme: ThemePreference;
  onThemeChange: (theme: ThemePreference) => void;
};

const themes: Array<{
  id: ThemePreference;
  label: string;
  icon: typeof SunIcon;
}> = [
  { id: "light", label: "Light", icon: SunIcon },
  { id: "dark", label: "Dark", icon: MoonIcon },
  { id: "system", label: "System", icon: LaptopIcon },
];

export function AppearanceSettings({ theme, onThemeChange }: Props) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Appearance</CardTitle>
        <CardDescription>Choose a theme for Guru Terminal.</CardDescription>
      </CardHeader>
      <CardContent>
        <div className="theme-grid" role="group" aria-label="Appearance">
          {themes.map((option) => {
            const ThemeIcon = option.icon;
            return (
              <button
                type="button"
                key={option.id}
                className="theme-card"
                data-active={theme === option.id}
                aria-label={`${option.label} theme`}
                aria-pressed={theme === option.id}
                onClick={() => onThemeChange(option.id)}
              >
                <ThemeIcon />
                <span>
                  <strong>{option.label}</strong>
                </span>
                {theme === option.id && <CheckIcon />}
              </button>
            );
          })}
        </div>
      </CardContent>
    </Card>
  );
}
