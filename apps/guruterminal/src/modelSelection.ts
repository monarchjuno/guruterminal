import type {
  ExecutionModelLock,
  ModelCatalog,
  ModelRunSelection,
} from "./types";

const emptyModelRunSelection = (): ModelRunSelection => ({
  model_profile_id: "",
  thinking_level: "",
  run_options: {},
});

export const defaultThinkingLevel = (levels: readonly string[]) =>
  levels.includes("medium") ? "medium" : (levels[0] ?? "");

export const defaultRunOptions = (model: ModelCatalog["models"][number]) =>
  Object.fromEntries(
    model.run_controls.map((control) => [control.id, control.default_choice]),
  );

export const visibleCatalogModels = (catalog: ModelCatalog | null) => {
  if (!catalog) return [];
  const hidden = new Set(catalog.hidden_model_profile_ids);
  return catalog.models.filter((model) => !hidden.has(model.id));
};

export const resolveModelRunSelection = (
  catalog: ModelCatalog | null,
  preferredProfileId?: string,
  preferredThinkingLevel?: string,
  preferredRunOptions?: Record<string, string>,
): ModelRunSelection => {
  const models = visibleCatalogModels(catalog);
  const preferred = preferredProfileId
    ? models.find((item) => item.id === preferredProfileId)
    : models[0];
  const preferredIsHidden = Boolean(
    preferredProfileId &&
      catalog?.hidden_model_profile_ids.includes(preferredProfileId),
  );
  const model = preferred ?? (preferredIsHidden ? models[0] : undefined);
  if (!model) return emptyModelRunSelection();
  return {
    model_profile_id: model.id,
    thinking_level:
      preferredThinkingLevel === undefined
        ? defaultThinkingLevel(model.thinking_levels)
        : model.thinking_levels.includes(preferredThinkingLevel)
          ? preferredThinkingLevel
          : "",
    run_options:
      preferredRunOptions !== undefined &&
      model.run_controls.length === Object.keys(preferredRunOptions).length &&
      model.run_controls.every((control) =>
        control.choices.some(
          (choice) => choice.id === preferredRunOptions[control.id],
        ),
      )
        ? preferredRunOptions
        : defaultRunOptions(model),
  };
};

export const exactSelectionFromLock = (
  catalog: ModelCatalog | null,
  lock?: ExecutionModelLock,
): ModelRunSelection | null => {
  if (!lock) return null;
  const model = visibleCatalogModels(catalog).find(
    (item) => item.id === lock.profile_id,
  );
  if (
    !model ||
    model.name !== lock.name ||
    model.provider !== lock.provider ||
    model.model !== lock.model ||
    !model.thinking_levels.includes(lock.thinking_level)
    || model.run_controls.length !== Object.keys(lock.run_options).length
    || model.run_controls.some(
      (control) =>
        !control.choices.some(
          (choice) => choice.id === lock.run_options[control.id],
        ),
    )
  ) {
    return null;
  }
  return {
    model_profile_id: lock.profile_id,
    thinking_level: lock.thinking_level,
    run_options: lock.run_options,
  };
};
