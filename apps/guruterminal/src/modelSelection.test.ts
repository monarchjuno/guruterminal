import {
  exactSelectionFromLock,
  resolveModelRunSelection,
} from "./modelSelection";
import type { ModelCatalog } from "./types";

const catalog: ModelCatalog = {
  hidden_model_profile_ids: [],
  models: [
    {
      id: "pi/luna",
      name: "Luna",
      provider: "openai-codex",
      model: "gpt-5.6-luna",
      input: ["text"],
      reasoning: true,
      context_window: 272_000,
      max_tokens: 128_000,
      thinking_levels: ["deliberate", "max"],
      thinking_level_map: { deliberate: "d", max: "m" },
      run_controls: [],
      credential_source: "saved",
    },
  ],
  providers: [],
};

describe("Pi catalog model selection", () => {
  it("defaults only from levels present in the catalog", () => {
    expect(resolveModelRunSelection(catalog)).toEqual({
      model_profile_id: "pi/luna",
      thinking_level: "deliberate",
      run_options: {},
    });
    expect(resolveModelRunSelection(catalog, "pi/luna")).toEqual({
      model_profile_id: "pi/luna",
      thinking_level: "deliberate",
      run_options: {},
    });
  });

  it("treats a blank preferred profile as unset", () => {
    expect(resolveModelRunSelection(catalog, "")).toEqual({
      model_profile_id: "pi/luna",
      thinking_level: "deliberate",
      run_options: {},
    });
  });

  it("does not silently replace stale explicit model or thinking values", () => {
    expect(resolveModelRunSelection(catalog, "removed/profile", "max")).toEqual(
      { model_profile_id: "", thinking_level: "", run_options: {} },
    );
    expect(
      resolveModelRunSelection(catalog, "pi/luna", "removed-level"),
    ).toEqual({
      model_profile_id: "pi/luna",
      thinking_level: "",
      run_options: {},
    });
  });

  it("restores a durable lock only when profile and thinking match exactly", () => {
    const lock = {
      profile_id: "pi/luna",
      name: "Luna",
      provider: "openai-codex",
      model: "gpt-5.6-luna",
      thinking_level: "max",
      run_options: {},
    };
    expect(exactSelectionFromLock(catalog, lock)).toEqual({
      model_profile_id: "pi/luna",
      thinking_level: "max",
      run_options: {},
    });
    expect(
      exactSelectionFromLock(catalog, {
        ...lock,
        thinking_level: "unsupported",
        run_options: {},
      }),
    ).toBeNull();
    for (const changed of [
      { ...lock, name: "Renamed Luna" },
      { ...lock, provider: "different-provider" },
      { ...lock, model: "different-model" },
    ]) {
      expect(exactSelectionFromLock(catalog, changed)).toBeNull();
    }
  });

  it("omits hidden models from selection and durable-lock restoration", () => {
    const hiddenCatalog = {
      ...catalog,
      hidden_model_profile_ids: ["pi/luna"],
    };
    expect(resolveModelRunSelection(hiddenCatalog)).toEqual({
      model_profile_id: "",
      thinking_level: "",
      run_options: {},
    });
    expect(
      exactSelectionFromLock(hiddenCatalog, {
        profile_id: "pi/luna",
        name: "Luna",
        provider: "openai-codex",
        model: "gpt-5.6-luna",
        thinking_level: "max",
        run_options: {},
      }),
    ).toBeNull();
  });

  it("falls back to the first visible model when the preferred profile is hidden", () => {
    const twoModels: ModelCatalog = {
      ...catalog,
      models: [
        catalog.models[0]!,
        {
          ...catalog.models[0]!,
          id: "pi/sol",
          name: "Sol",
          model: "gpt-5.6-sol",
        },
      ],
      hidden_model_profile_ids: ["pi/luna"],
    };
    expect(resolveModelRunSelection(twoModels, "pi/luna", "max")).toEqual({
      model_profile_id: "pi/sol",
      thinking_level: "max",
      run_options: {},
    });
    expect(
      resolveModelRunSelection(twoModels, "removed/profile", "max"),
    ).toEqual({
      model_profile_id: "",
      thinking_level: "",
      run_options: {},
    });
  });
});
