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

Chat is the only model-consuming session. The SQLite transcript is the conversation record. Each turn starts one disposable Pi process. Pi JSONL is cache, not authority.

`Use memory` and `Update memory` are independent and default on. With
`evidence_create`, the agent selects exact values from delivered current-turn
results using JSON Pointer. Rust validates the selection, copies the data into
Evidence, and writes the producer receipt. An explicit `decision_submit`
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

Self-improvement changes Guru Memory only. It does not change model weights, Skills, Tool bindings, or the finance floor.

## Runs

At most four Chat runs may be live at once. Each Chat has one writer. Memory apply is exclusive for that Guru and does not consume a model slot. Switching Guru or leaving Chat is not Stop.

The desktop app is the user-facing authority. Bundled MCP and native connectors
run behind the same Guru-scoped Rust authority; none may write canonical
storage directly or bypass UI approval.
