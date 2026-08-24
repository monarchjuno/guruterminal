const PERFORMANCE_APIS = new Set([
  "openai-responses",
  "openai-codex-responses",
]);

const PERFORMANCE_CONTROL = Object.freeze({
  id: "performance",
  label: "Performance",
  default_choice: "standard",
  choices: Object.freeze([
    Object.freeze({
      id: "standard",
      label: "Standard",
      description: "Use the provider's standard service tier.",
    }),
    Object.freeze({
      id: "fast",
      label: "Fast",
      description: "Request the provider's priority service tier.",
    }),
  ]),
});

export function runControlsFor(model) {
  return PERFORMANCE_APIS.has(model?.api) ? [PERFORMANCE_CONTROL] : [];
}

export function applyRunOptions(model, payload, options) {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error("Pi provider request payload is invalid");
  }
  const controls = new Map(runControlsFor(model).map((control) => [control.id, control]));
  for (const [controlId, choiceId] of Object.entries(options)) {
    const control = controls.get(controlId);
    if (!control || !control.choices.some((choice) => choice.id === choiceId)) {
      throw new Error("Pi model run option is unsupported");
    }
  }

  const next = { ...payload };
  if (options.performance === "fast") next.service_tier = "priority";
  else if (options.performance === "standard") delete next.service_tier;
  return next;
}
