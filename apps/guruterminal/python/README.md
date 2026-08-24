# Guru Terminal Finance Worker

This directory is an isolated Python 3.12 worker for deterministic, read-only
financial calculations. It is not a financial data provider, application
backend, HTTP server, Python REPL, script runner, package installer, broker, or
owner of Guru Terminal memory.

The calculation surface covers decimal ratios and growth, valuation and DCF
sensitivity, point-in-time filtering and aggregation, series and risk metrics,
capital-cost and enterprise-value bridges, currency conversion, and IRR. DCF
accepts explicit annual cash flows, discount and terminal assumptions, net
debt, shares, and currency, then returns the full enterprise-to-equity bridge
plus a high-terminal-value warning. Every input must carry host-issued source
provenance; the worker verifies arithmetic, not the truth of an assumption.

The worker performs no network requests and has no live market-data tool.
Provider data is obtained through the app's capability runtimes and passed to
this worker only as explicit calculation inputs.

The native app supervises the worker as a persistent subprocess. Messages are
newline-delimited JSON-RPC 2.0 objects over stdin/stdout. The worker writes
protocol messages only to stdout; diagnostics go to stderr.

## Development

```bash
uv sync --locked
uv run --locked pytest
uv run --locked guruterminal-finance
```

The first request must be `system.handshake`. It returns protocol version `1`,
the worker and Python versions, the SHA-256 digest of `uv.lock`, and the closed
list of tool names. `tools.list` returns their full schemas. A client must
complete the handshake before listing or invoking tools. Tool calls use
`tools.call` and must include an aware ISO-8601
`data_cutoff` plus source provenance. A client cancels work with
`system.cancel` and a `request_id`; it may send that method as a request or a
notification.
Calls may complete out of order, so callers must correlate them by request ID.

Example request sequence:

```json
{"jsonrpc":"2.0","id":"hello","method":"system.handshake","params":{"protocol_version":"1","client":{"name":"guruterminal","version":"1.0.0"}}}
{"jsonrpc":"2.0","id":"calc-1","method":"tools.call","params":{"name":"percentage_change","arguments":{"start":"80","end":"100","precision":2},"context":{"data_cutoff":"2025-01-01T00:00:00Z","timeout_ms":30000,"sources":[{"source_id":"filing-1","provider":"fixture","as_of":"2024-09-30T00:00:00Z","available_at":"2024-11-01T00:00:00Z","retrieved_at":"2025-01-02T00:00:00Z"}]}}}
```

Numeric results are decimal strings rather than binary floating-point values.
Every result includes the data cutoff, only the sources actually used, a
canonical input digest, and worker/tool versions.

## Frozen worker

Build the release worker as a PyInstaller one-directory bundle:

```bash
uv run --locked python build_worker.py
```

The output is `dist/guruterminal-finance/`. The executable and its adjacent
`_internal` directory are one artifact and must remain together. The native app
should register the executable as a sidecar and bundle the support directory as
a resource. Build separately for each target architecture. Product builds sign
all nested native code before signing and notarizing the outer app bundle.

The worker deliberately uses `onedir`, not `onefile`: it does not unpack native
libraries into a temporary directory at runtime.
