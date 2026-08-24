export const errorMessage = (cause: unknown, fallback: string) => {
  if (cause instanceof Error && cause.message) return cause.message;
  if (typeof cause === "string" && cause.trim()) return cause;
  if (
    cause !== null &&
    typeof cause === "object" &&
    "message" in cause &&
    typeof cause.message === "string" &&
    cause.message.trim()
  ) {
    return cause.message;
  }
  return fallback;
};
