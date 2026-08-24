export const compactDate = (value: string) =>
  new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
  }).format(new Date(value));

export const compactTime = (value: string) =>
  new Intl.DateTimeFormat("en-US", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));

export const kindLabel = {
  Wiki: "Wiki",
  Lens: "Lens",
  Evidence: "Evidence",
  Decision: "Decision",
} as const;
