# Architecture

Guru Terminal is one local-first desktop product. The app is the only user-facing authority.

## Trust

| Component | Role |
| --- | --- |
| React | Display state and submit typed user intent |
| Rust host | Guru isolation, SQLite, Memory writes, credentials, process leases, Tool receipts, and generic MCP authority |
| `guruterminal-core` | Scan, validate, and retrieve one Guru workspace |
| Pi | Disposable model reasoning for one Chat turn |
| OpenBB MCP sidecar | Guru-scoped warm read-only financial, economic, and filings Tools discovered through the generic MCP host |
| Finance worker | Deterministic financial calculations with no network access |
| Compute worker | Turn-local agent-authored Python or JavaScript, no host access |

React cannot touch the shell, arbitrary files, credentials, or Memory. Rust is the only writer of canonical state.

The workers ship only as private app sidecars; none is a separately supported
product or public CLI. Their versions and checksums come from manifests,
lockfiles, and staging scripts. Rust verifies each staged executable before
launch.

Bundled and signed sidecar dependencies are trusted product code. OpenBB's
exact-host guard covers its audited Python HTTP clients; it is policy
enforcement and hardening, not an OS sandbox against hostile native code or raw
sockets. Secrets, canonical writes, Tool visibility, result bounds, and
process lifetime remain outside the sidecar under Rust authority.

## Where data lives

- Memory is Markdown in `guruterminal/{wiki,lens,evidence,decision}/`.
- SQLite owns Gurus, Chat transcripts, current Artifacts, Skill revisions, and short-lived Chat/Memory finalization journals. A chart Artifact stores its actual columns and rows with an immutable lineage receipt. Creation maps a delivered result explicitly or supplies inline data; revision after `artifact.read` reuses the stored dataset unless a new dataset is supplied.
- Each Chat turn has an in-memory `RunResultRegistry`. A successfully delivered read Tool result receives a run-local `result_ref` with producer, request and response digests, retrieval time, warnings, payload, and upstream lineage. Failed, cancelled, write, admin, and control calls are not registered. The registry is discarded when the turn ends.
- Workbench file reads, lists, finds, and searches are brokered read results. Skill-file reads, capability discovery/loading, finance source catalog lookup, MCP administration, and `run_results_list` are execution/control metadata and are not result data.
- Each Guru workspace versions Memory Markdown with git. Rust commits after every canonical write. The agent has a read-only prior-version tool and no Git authority. The user can revert an applied Wiki or Lens write; Rust restores the previous bytes and commits again.
- Connector secrets stay in the OS keyring. Rust reports readiness, never the secret.
- Wiki and Lens are editable. Evidence and Decision contents are immutable.

## Chat

Chat is the only model-consuming session. The SQLite transcript is the conversation record. Each turn starts one disposable Pi process. Pi JSONL is cache, not authority. Process reuse is deliberately not a latency shortcut: the broker token, Tool policy, credentials, host context, Skills, working directory, and session identity are fixed at launch for one turn. A future warm Pi pool requires an idle-deny broker with an attested activate/drain/deactivate boundary before it can preserve this isolation.

The broker has the first pool prerequisite: a non-serializable process-lifetime
identity can restart sequential turn brokers at the same private endpoint and
token, while policy, executor, and transaction cardinality are rebuilt for
every activation. Concurrent identity use and stale endpoints fail closed, and
the identity remains leased until handlers drain and the endpoint disappears.
Chat does not retain Pi yet; this boundary must be integrated with idle-state,
session-cursor, Skill-lifetime, credential, and connector-authority attestation
before any process enters a pool.

### Latency boundaries

The host performs one bounded `knowledge context` preflight per turn. That one
catalog scan supplies validation, health, revision, learned-record metadata,
and the charter instead of launching and scanning once for each field. Pi
startup sends its independent control requests as one pipeline and validates
the complete response set under one deadline. The app-owned Pi transport uses
automatic provider transport selection; only the exact previous app-owned
settings file is upgraded.

Provider text is provisional until Pi closes the assistant turn. Rust emits
draft start/delta/end events while the model is writing, but sends canonical
answer text only after the turn capture validates the matching message end.
The React transport coalesces adjacent draft deltas to a 32 ms render cadence.
Progress starts from a full snapshot, then uses monotonic sequence patches that
upsert or remove only changed lifecycle items; a terminal full snapshot can
resynchronize a missed patch. No full timeline is serialized per token or Tool
step, and unchanged message cards do not rerender when a sibling message
updates.

A successful run emits setup, first-text, generation, and total milliseconds;
cold/warm session-cache status; and input, output, cache-read, and cache-write
token counts. Generation-finished is distinct from durable completion, so
persistence and Memory finalization time remains visible in the total. The
desktop shows first-text and total latency on the completed message, with the
full breakdown in its tooltip.

Provider model discovery uses a five-minute, 32-entry LRU keyed by a
secret-free credential-authority generation. Configure, connect, disconnect,
API-key rotation, and OAuth refresh-authority rotation invalidate the entry;
ordinary OAuth access-token refresh does not. A catalog render reads and
validates the bounded Pi auth file once, even when thousands of models are
present. Per-Guru workbench mutation locks retain only weak registry entries,
so visiting new Gurus cannot grow that registry for the life of the app.

The initial Pi Tool surface contains bounded reads and capability discovery,
not mutation schemas. Workbench authoring, Markdown publishing, Evidence and
Decision creation, and optional Memory learning are deterministic built-in
components loaded only when the agent discovers and selects them. This removes
roughly 4–5 KB of JSON Schema from an ordinary first model request without
granting new authority; Rust still owns the same per-turn allowlist. Capability
search requires a non-empty query and returns components in stable ID order.

`Use memory` and `Update memory` are independent and default on. With
`evidence_create`, the agent writes a readable markdown body and cites
delivered current-turn `result_ref` values. Rust verifies those receipts and
writes a human-readable `# Sources` section. An explicit `decision_submit`
becomes Decision even when `Update memory` is off.

`chart_publish` is provider-neutral. The agent either maps a result's row and
column pointers or supplies explicit inline rows and columns with optional
upstream result references. Rust checks types, bounds, and reference integrity;
it does not infer a provider response shape.

A non-abstain Decision may cite only Evidence created in that turn. Existing
Wiki and Lens records must have been exact-read in the turn before their IDs
can be listed as used context. Evidence and Decision are applied only after the
turn completes successfully, together with any chart and Memory changes.
Before changing Memory, Rust persists a finalization intent containing the
bounded exact change set, before/after hashes, and prior Git state. The commit OID is recorded before
HEAD moves; Chat and Artifacts then finalize atomically in SQLite. A crash
leaves the Guru quarantined until the intent is idempotently completed or the
Git index, HEAD, and exact Markdown set are compensated.

The broker commits a staged result only after the client acknowledges the
response, then sends a commit barrier. If that final barrier is lost or
malformed, the outcome is indeterminate rather than an ordinary Tool error;
the disposable agent process exits and the whole turn is aborted, making the
turn-local registry unreachable and preventing partial canonical writes.

Selecting `$wiki` or `$lens` turns both Memory switches on for that turn. When Use memory is on, Chat also injects the reserved charter Lens (`lens:charter`) as standing philosophy, still dated and untrusted. After the answer commits, Rust may apply Wiki or Lens changes from the same turn. A later relevant turn must retrieve and use that state. A saved record that never changes later work is not the product.

A new Guru starts with credential-free Tools on: deterministic finance
calculations, keyless OpenBB providers, World Bank, public web, and sandboxed
compute. OpenBB data providers that need credentials have separate Marketplace
entries and stay unavailable to a Guru until the user configures and enables
them. Native OpenDART, KRX, and Korea Investment connectors remain separate.

Loading the `mcp/openbb` capability lazily starts or reuses a Guru-scoped OpenBB
stdio process. Sequential Chat turns that share the same Guru and
provider/credential authority reuse one process. Concurrent turns of the same
Guru do not share a process, idle processes expire after a few minutes, and the
idle pool is capped. Before reuse the host resets the process to its admin-only
control surface so activation does not leak across turns. Provider credentials
travel through a private bootstrap channel and never through Tool arguments,
process arguments, general environment variables, receipts, or results.
Within a turn, different MCP servers may execute concurrently. Inventory,
control, and Tool calls for the same server remain serialized. A server reset
runs in a background quarantine and the process enters the idle pool only after
the reset succeeds; a terminal failure discards only that server slot.

Self-improvement changes Guru Memory only. It does not change model weights, Skills, Tool bindings, or the finance floor.

## Runs

At most four Chat runs may be live at once. Each Chat has one writer. Memory apply is exclusive for that Guru and does not consume a model slot. Switching Guru or leaving Chat is not Stop.

The desktop app is the user-facing authority. Bundled MCP and native connectors
run behind the same Guru-scoped Rust authority; none may write canonical
storage directly or bypass UI approval.
