import { memo, useMemo, useState } from "react";
import { CpuIcon } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type {
  ConfiguredModel,
  ModelProviderOption,
  ModelRunSelection,
} from "../../types";
import { defaultRunOptions, defaultThinkingLevel } from "../../modelSelection";
import { PromptInputButton } from "../ai-elements/prompt-input";

type Props = {
  models: ConfiguredModel[];
  providers: ModelProviderOption[];
  selection: ModelRunSelection;
  disabled?: boolean;
  onSelectionChange: (selection: ModelRunSelection) => void;
};

type ProviderModels = {
  id: string;
  label: string;
  models: ConfiguredModel[];
};

const groupCatalog = (
  models: ConfiguredModel[],
  providers: ModelProviderOption[],
): ProviderModels[] => {
  const modelsByProvider = new Map<string, ConfiguredModel[]>();
  for (const model of models) {
    const group = modelsByProvider.get(model.provider) ?? [];
    group.push(model);
    modelsByProvider.set(model.provider, group);
  }

  const groups = providers.flatMap((provider) => {
    const providerModels = modelsByProvider.get(provider.id);
    if (!providerModels?.length) return [];
    modelsByProvider.delete(provider.id);
    return [{ id: provider.id, label: provider.label, models: providerModels }];
  });

  for (const [provider, providerModels] of modelsByProvider) {
    groups.push({ id: provider, label: provider, models: providerModels });
  }
  return groups;
};

const keepMenuOpen = (event: Event) => {
  event.preventDefault();
};

export const ChatModelMenu = memo(function ChatModelMenu({
  models,
  providers,
  selection,
  disabled = false,
  onSelectionChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const selectedModel = useMemo(
    () => models.find((model) => model.id === selection.model_profile_id),
    [models, selection.model_profile_id],
  );
  const catalog = useMemo(
    () => groupCatalog(models, providers),
    [models, providers],
  );
  const selectedOptionLabels = selectedModel?.run_controls.flatMap((control) => {
    const choice = control.choices.find(
      (candidate) => candidate.id === selection.run_options[control.id],
    );
    return choice && choice.id !== control.default_choice ? [choice.label] : [];
  }) ?? [];
  const triggerLabel = selectedModel
    ? [
        selectedModel.name,
        selection.thinking_level || "Choose thinking",
        ...selectedOptionLabels,
      ].join(" · ")
    : "Choose model";

  const selectionFor = (model: ConfiguredModel): ModelRunSelection =>
    model.id === selection.model_profile_id
      ? selection
      : {
          model_profile_id: model.id,
          thinking_level: defaultThinkingLevel(model.thinking_levels),
          run_options: defaultRunOptions(model),
        };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <PromptInputButton
          className="composer-model-menu"
          aria-label="Model settings for this message"
          disabled={disabled || models.length === 0}
          title={triggerLabel}
        >
          <CpuIcon />
          <span>{triggerLabel}</span>
        </PromptInputButton>
      </DropdownMenuTrigger>
      {open ? (
        <DropdownMenuContent
          className="composer-model-menu-panel w-72 min-w-72"
          side="top"
          align="start"
          sideOffset={8}
          aria-label="Available models"
        >
          {catalog.length === 0 ? (
            <DropdownMenuItem disabled>No connected models</DropdownMenuItem>
          ) : (
            catalog.map((provider, index) => (
              <DropdownMenuRadioGroup
                key={provider.id}
                value={selection.model_profile_id}
                onValueChange={(profileId) => {
                  const model = provider.models.find((item) => item.id === profileId);
                  if (model) onSelectionChange(selectionFor(model));
                }}
              >
                {index > 0 ? <DropdownMenuSeparator /> : null}
                <DropdownMenuLabel>{provider.label}</DropdownMenuLabel>
                {provider.models.map((model) => (
                  <DropdownMenuRadioItem
                    key={model.id}
                    value={model.id}
                    onSelect={keepMenuOpen}
                  >
                    {model.name}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            ))
          )}
          {selectedModel ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Thinking</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                value={selection.thinking_level}
                onValueChange={(thinking_level) =>
                  onSelectionChange({
                    ...selection,
                    thinking_level,
                  })
                }
              >
                {selectedModel.thinking_levels.map((level) => (
                  <DropdownMenuRadioItem
                    key={level}
                    value={level}
                    onSelect={keepMenuOpen}
                  >
                    {level}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
              {selectedModel.run_controls.map((control) => (
                <DropdownMenuRadioGroup
                  key={control.id}
                  value={selection.run_options[control.id]}
                  onValueChange={(choice) =>
                    onSelectionChange({
                      ...selection,
                      run_options: {
                        ...selection.run_options,
                        [control.id]: choice,
                      },
                    })
                  }
                >
                  <DropdownMenuLabel>{control.label}</DropdownMenuLabel>
                  {control.choices.map((choice) => (
                    <DropdownMenuRadioItem
                      key={choice.id}
                      value={choice.id}
                      title={choice.description}
                      onSelect={keepMenuOpen}
                    >
                      {choice.label}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              ))}
            </>
          ) : null}
        </DropdownMenuContent>
      ) : null}
    </DropdownMenu>
  );
});
