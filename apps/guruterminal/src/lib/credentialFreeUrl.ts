/**
 * Parses a URL and accepts it only when it is a credential-free HTTP(S)
 * address. Returns null for anything else. Rust enforces the same boundary;
 * this helper exists for early UI feedback and the mock bridge.
 */
export const httpAddressError =
  "Only http and https addresses without a password can be opened.";

export function parseCredentialFreeHttpUrl(candidate: string): URL | null {
  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    return null;
  }
  if (
    !["http:", "https:"].includes(parsed.protocol) ||
    !parsed.host ||
    parsed.username ||
    parsed.password
  ) {
    return null;
  }
  return parsed;
}
