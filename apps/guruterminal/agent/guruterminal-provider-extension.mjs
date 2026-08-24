import {
  closeSync,
  constants,
  fstatSync,
  fsyncSync,
  openSync,
  readFileSync,
  writeSync,
} from "node:fs";
import { isAbsolute, join } from "node:path";
import { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai";
import { runControlsFor } from "./model-run-controls.mjs";
import {
  assertSearchResultSafe,
  isNativeSearchProvider,
  NativeSearchError,
  runAnthropicSearchThroughPi,
  runCodexSearchThroughPi,
  runNativeSearch,
  toSearchErrorResult,
} from "./guruterminal-native-search.mjs";

const PROTOCOL = "guruterminal-provider/1";
const MAX_RESULT_BYTES = 512 * 1024;
const MAX_REQUEST_BYTES = 64 * 1024;
const RESULT_FILE = process.env.GURUTERMINAL_PROVIDER_RESULT_FILE;
const REQUEST_FILE = process.env.GURUTERMINAL_PROVIDER_REQUEST_FILE;
const PROVIDER_API_KEY = process.env.GURUTERMINAL_PROVIDER_API_KEY;
delete process.env.GURUTERMINAL_PROVIDER_RESULT_FILE;
delete process.env.GURUTERMINAL_PROVIDER_REQUEST_FILE;
delete process.env.GURUTERMINAL_PROVIDER_API_KEY;

async function credentialRuntime() {
  const agentDir = process.env.PI_CODING_AGENT_DIR;
  if (!agentDir || !isAbsolute(agentDir)) {
    throw new Error("Pi credential storage is unavailable");
  }
  return ModelRuntime.create({
    authPath: join(agentDir, "auth.json"),
    modelsPath: null,
    refreshOnCreate: false,
  });
}

function writeResult(value) {
  if (!RESULT_FILE) throw new Error("Guru Terminal provider result file is unavailable");
  const encoded = Buffer.from(JSON.stringify({ protocol: PROTOCOL, ...value }), "utf8");
  if (encoded.length === 0 || encoded.length > MAX_RESULT_BYTES) {
    throw new Error("Guru Terminal provider result exceeded its size limit");
  }

  let descriptor;
  try {
    descriptor = openSync(
      RESULT_FILE,
      constants.O_WRONLY | constants.O_TRUNC | (constants.O_NOFOLLOW ?? 0),
    );
    const metadata = fstatSync(descriptor);
    if (!metadata.isFile()) throw new Error("Guru Terminal provider result file is invalid");
    writeExactSync(descriptor, encoded);
    fsyncSync(descriptor);
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

export function writeExactSync(descriptor, encoded, write = writeSync) {
  let offset = 0;
  while (offset < encoded.length) {
    const written = write(descriptor, encoded, offset, encoded.length - offset, null);
    if (!Number.isInteger(written) || written <= 0) {
      throw new Error("Guru Terminal provider result write made no progress");
    }
    offset += written;
  }
}

function emit(ctx, event) {
  ctx.ui.notify(`${PROTOCOL}:${JSON.stringify(event)}`, "info");
}

function modelsFor(ctx, providerId) {
  return ctx.modelRegistry
    .getAvailable()
    .filter((model) => model.provider === providerId)
    .map((model) => ({
      id: model.id,
      name: model.name || model.id,
      reasoning: Boolean(model.reasoning),
      context_window: model.contextWindow,
      max_tokens: model.maxTokens,
      input: model.input ?? ["text"],
      thinking_levels: getSupportedThinkingLevels(model),
      thinking_level_map: model.thinkingLevelMap ?? {},
      run_controls: runControlsFor(model),
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}

function validateProviderId(providerId) {
  if (!providerId || !/^[a-z0-9-]{1,64}$/.test(providerId)) {
    throw new Error("Provider ID is invalid");
  }
  return providerId;
}

function waitForBrowserCallback(signal) {
  if (!signal || typeof signal.addEventListener !== "function") {
    throw new Error("Pi browser login cancellation is unavailable");
  }
  const reason = () =>
    signal.reason instanceof Error
      ? signal.reason
      : new Error("Browser authorization completed");
  if (signal.aborted) return Promise.reject(reason());
  return new Promise((_, reject) => {
    signal.addEventListener("abort", () => reject(reason()), { once: true });
  });
}

function readSearchRequest() {
  if (!REQUEST_FILE) throw new Error("Guru Terminal provider request file is unavailable");
  const encoded = readFileSync(REQUEST_FILE);
  if (encoded.length === 0 || encoded.length > MAX_REQUEST_BYTES) {
    throw new Error("Guru Terminal provider request exceeded its size limit");
  }
  const request = JSON.parse(encoded.toString("utf8"));
  const allowedKeys = new Set([
    "protocol",
    "type",
    "provider",
    "query",
    "limit",
    "recency",
    "include_domains",
    "exclude_domains",
  ]);
  if (
    !request ||
    typeof request !== "object" ||
    Array.isArray(request) ||
    Object.keys(request).some((key) => !allowedKeys.has(key))
  ) {
    throw new Error("Guru Terminal provider search request is invalid");
  }
  if (request.protocol !== PROTOCOL || request.type !== "search") {
    throw new Error("Guru Terminal provider search request is invalid");
  }
  if (!isNativeSearchProvider(request.provider)) {
    throw new Error("Search provider is not allowlisted");
  }
  if (
    typeof request.query !== "string" ||
    request.query.trim().length === 0 ||
    request.query.length > 4_096
  ) {
    throw new Error("Search query is invalid");
  }
  const limit = Number(request.limit ?? 5);
  if (!Number.isInteger(limit) || limit < 1 || limit > 10) {
    throw new Error("Search limit is invalid");
  }
  if (
    request.recency !== undefined &&
    !["day", "week", "month", "year"].includes(request.recency)
  ) {
    throw new Error("Search recency is invalid");
  }
  for (const domains of [request.include_domains, request.exclude_domains]) {
    if (
      domains !== undefined &&
      (!Array.isArray(domains) ||
        domains.length > 10 ||
        domains.some(
          (domain) =>
            typeof domain !== "string" ||
            domain.length < 1 ||
            domain.length > 253 ||
            !/^[a-z0-9.-]+$/u.test(domain),
        ))
    ) {
      throw new Error("Search domains are invalid");
    }
  }
  return {
    provider: request.provider,
    query: request.query,
    limit,
    recency: typeof request.recency === "string" ? request.recency : undefined,
    include_domains: Array.isArray(request.include_domains) ? request.include_domains : undefined,
    exclude_domains: Array.isArray(request.exclude_domains) ? request.exclude_domains : undefined,
  };
}

export async function resolveSearchAuth(ctx, providerId) {
  const provider = ctx.modelRegistry.getProvider(providerId);
  if (!provider) throw new NativeSearchError("unavailable", "Pi provider is unavailable");
  let auth;
  try {
    if (typeof ctx.modelRegistry.getProviderAuth === "function") {
      auth = await ctx.modelRegistry.getProviderAuth(providerId);
    } else {
      const runtime = await credentialRuntime();
      runtime.registerNativeProvider(provider);
      auth = await runtime.getAuth(providerId);
    }
  } catch {
    throw new NativeSearchError("unavailable", "provider credentials are unavailable");
  }
  if (providerId === "xai" && auth?.source === "OAuth") {
    throw new NativeSearchError(
      "unavailable",
      "xAI native web search requires an xAI API key",
    );
  }
  if (!auth?.auth) throw new NativeSearchError("unavailable", "provider credentials are unavailable");
  return {
    apiKey: auth.auth.apiKey,
    headers: auth.auth.headers ?? {},
    baseUrl: auth.auth.baseUrl,
  };
}

function credentialCommand(args) {
  const parts = args.trim().split(/\s+/u);
  if (parts.length !== 2 || !["set", "clear"].includes(parts[1])) {
    throw new Error("Credential command is invalid");
  }
  return {
    providerId: validateProviderId(parts[0]),
    operation: parts[1],
  };
}

function writeCredentialUpdated(providerId) {
  writeResult({
    type: "credential_updated",
    provider: providerId,
    models: [],
  });
}

function oauthPrompt(prompt) {
  if (prompt.type === "select") {
    const browser = prompt.options.find((option) => option.id === "browser");
    if (!browser) throw new Error("Pi browser login is unavailable");
    return browser.id;
  }
  if (prompt.type === "manual_code") return waitForBrowserCallback(prompt.signal);
  throw new Error("Unexpected OAuth prompt");
}

function oauthNotify(ctx, event) {
  if (event.type === "auth_url") {
    emit(ctx, {
      type: "authorization_url",
      url: event.url,
      instructions: event.instructions,
    });
    return;
  }
  if (event.type === "device_code") {
    if (typeof event.verificationUri !== "string" || event.verificationUri.length === 0) {
      throw new Error("Pi device login is missing a verification URL");
    }
    emit(ctx, {
      type: "authorization_url",
      url: event.verificationUri,
    });
    return;
  }
  if (event.type === "progress" || event.type === "info") {
    emit(ctx, { type: "progress", message: event.message });
  }
}

export default function guruTerminalProviderExtension(pi) {
  pi.registerCommand("guruterminal-provider-models", {
    description: "Return Pi's bundled model catalog for one provider",
    handler: async (args, ctx) => {
      const providerId = validateProviderId(args.trim());
      const provider = ctx.modelRegistry.getProvider(providerId);
      if (!provider) throw new Error("Pi provider is unavailable");
      writeResult({ type: "models", provider: providerId, models: modelsFor(ctx, providerId) });
    },
  });

  pi.registerCommand("guruterminal-provider-api-key", {
    description: "Persist or clear one API-key credential through Pi's credential store",
    handler: async (args, ctx) => {
      const { providerId, operation } = credentialCommand(args);
      const provider = ctx.modelRegistry.getProvider(providerId);
      if (!provider?.auth?.apiKey) {
        throw new Error("Pi provider does not support API-key authentication");
      }
      const runtime = await credentialRuntime();
      runtime.registerNativeProvider(provider);
      if (operation === "clear") {
        await runtime.logout(providerId);
      } else {
        if (
          !PROVIDER_API_KEY ||
          Buffer.byteLength(PROVIDER_API_KEY, "utf8") > 8 * 1024 ||
          /[\u0000-\u001f\u007f]/u.test(PROVIDER_API_KEY)
        ) {
          throw new Error("Provider API key is invalid");
        }
        await runtime.login(providerId, "api_key", {
          prompt: async (prompt) => {
            if (prompt.type !== "secret" && prompt.type !== "text") {
              throw new Error("Pi provider requested unsupported API-key input");
            }
            return PROVIDER_API_KEY;
          },
          notify: () => undefined,
        });
      }
      writeCredentialUpdated(providerId);
    },
  });

  pi.registerCommand("guruterminal-provider-login", {
    description: "Connect one OAuth provider through Pi",
    handler: async (args, ctx) => {
      const providerId = validateProviderId(args.trim());
      const provider = ctx.modelRegistry.getProvider(providerId);
      if (!provider?.auth?.oauth) throw new Error("Pi provider does not support OAuth");

      const controller = new AbortController();
      const runtime = await credentialRuntime();
      runtime.registerNativeProvider(provider);
      await runtime.login(providerId, "oauth", {
        signal: controller.signal,
        prompt: oauthPrompt,
        notify: (event) => oauthNotify(ctx, event),
      });

      writeCredentialUpdated(providerId);
      emit(ctx, { type: "connected", message: `${provider.name} is connected.` });
    },
  });

  pi.registerCommand("guruterminal-provider-search", {
    description: "Run one bounded model-native web search through the current Pi credential store",
    handler: async (_args, ctx) => {
      const request = readSearchRequest();
      const secrets = [];
      try {
        const availableModelIds = (ctx.modelRegistry.getAvailable?.() ?? [])
          .filter((model) => model.provider === request.provider)
          .map((model) => model.id);
        const result = await runNativeSearch(request, {
          availableModelIds,
          codexTransport:
            request.provider === "openai-codex"
              ? (search) => runCodexSearchThroughPi(ctx, search)
              : undefined,
          anthropicTransport:
            request.provider === "anthropic" &&
            typeof ctx.modelRegistry.find === "function" &&
            typeof ctx.modelRegistry.complete === "function"
              ? (search) => runAnthropicSearchThroughPi(ctx, search)
              : undefined,
          resolveAuth: async (providerId) => {
            const auth = await resolveSearchAuth(ctx, providerId);
            if (auth.apiKey) secrets.push(auth.apiKey);
            for (const value of Object.values(auth.headers ?? {})) {
              if (typeof value === "string" && value.length > 8) secrets.push(value);
            }
            return auth;
          },
        });
        assertSearchResultSafe(result, secrets);
        writeResult(result);
      } catch (error) {
        const failure = toSearchErrorResult(request.provider, error);
        assertSearchResultSafe(failure, secrets);
        writeResult(failure);
      }
    },
  });

  pi.registerCommand("guruterminal-provider-logout", {
    description: "Clear one saved Pi credential for any cataloged provider",
    handler: async (args, ctx) => {
      const providerId = validateProviderId(args.trim());
      const provider = ctx.modelRegistry.getProvider(providerId);
      if (!provider) throw new Error("Pi provider is unavailable");
      const runtime = await credentialRuntime();
      runtime.registerNativeProvider(provider);
      await runtime.logout(providerId);
      writeCredentialUpdated(providerId);
    },
  });
}
