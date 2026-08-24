# Guru Terminal OpenBB runtime

This project freezes OpenBB Platform and the official OpenBB MCP server into a
private, read-only sidecar. It is not a public CLI. Rust owns process creation,
credentials, capability checks, result limits, and shutdown.

The frozen OpenBB distribution and its pinned provider packages are trusted
product code. Exact-host checks cover the audited Python HTTP stacks and catch
undeclared provider egress, but they are not an OS network sandbox against a
compromised native dependency. HOME/cache redirection, private bootstrap,
process-tree cleanup, MCP filtering, and result bounds remain enforced at the
host boundary.

## Wire contract

The process reads one newline-terminated Guru bootstrap frame before the MCP
client sends `initialize`:

```json
{"type":"guruterminal.bootstrap","protocol_version":1,"run_id":"run-1","scratch_dir":"/private/run/directory","credentials":{},"settings":{"allowed_categories":["equity"],"enabled_provider_ids":["yfinance"],"allowed_network_hosts":["consent.yahoo.com","fc.yahoo.com","finance.yahoo.com","guce.yahoo.com","markets.businessinsider.com","query1.finance.yahoo.com","query2.finance.yahoo.com"],"provider_config":{}}}
```

The frame is at most 65,536 bytes. `scratch_dir` must already exist, be an
absolute non-symlink directory, and be private to the current user. Credential
keys are OpenBB credential names authorized by the enabled provider entries in
`runtime-manifest.json`. After the newline, stdin and stdout speak the official
MCP stdio protocol.

The wrapper points OpenBB's home, settings, caches, temporary files, Matplotlib,
Numba, and Python bytecode at the run scratch directory before importing any
OpenBB module. Credentials are installed into OpenBB's process-local user
settings service without writing `user_settings.json`; they are never accepted
in command-line arguments or environment variables. Provider configuration is
strictly mapped by the manifest. In particular, the SEC contact email replaces
all bundled SEC request identities, and Tradier's account type is limited to
`sandbox` or `live`.

Provider authority is also enforced for Tools that do not expose a top-level
`provider` input. The runtime manifest explicitly classifies the audited local
analytics Tools and maps direct provider Tools to their one implicit provider.
Unknown providerless routes are omitted. For example, the `imf_utils` surface is
not discoverable unless the Guru has enabled the `imf` provider.

The source manifest names the executable `guruterminal-openbb`. Windows staging
rewrites only the public bundle manifest to `guruterminal-openbb.exe`; the
package-internal manifest remains platform-neutral for wrapper authorization.

## Development

```sh
uv sync --locked --python 3.12
uv run --locked pytest
uv run --locked ruff check .
uv run --locked python build_sidecar.py --check
```

`build_sidecar.py` runs `openbb-build` and creates a PyInstaller one-directory
bundle named `guruterminal-openbb`. Platform staging scripts live in the parent
`scripts/` directory.

The bundle also contains `THIRD_PARTY_LICENSES/python-distributions.json` and
its referenced compliance archive. The builder resolves the locked runtime
dependency closure (including `openbb[all]`), copies every distribution's full
metadata plus package-level license/NOTICE files, adds the CPython and
PyInstaller license payloads, and verifies every archived byte by SHA-256.

## Opt-in live parity

The live audit discovers the staged runtime's current Tool inventory, chooses
public OpenBB Tools from their schemas and descriptions, and records the actual
Tool/keyless-provider pair used for each former finance capability. It accepts
only successful, non-error results whose canonical
`structuredContent.provider` matches the selected provider; it does not lock
provider response shapes or add provider-specific compatibility routes.

```sh
uv run --locked python -m guruterminal_openbb.live_parity \
  --bundle ../src-tauri/resources/pi-runtime/openbb \
  --report /tmp/guruterminal-openbb-live-parity.json \
  --summary-report test-output/live-parity-summary.json

GURUTERMINAL_OPENBB_LIVE=1 uv run --locked pytest -m live
```

The command and marked test return non-zero while any parity item is missing or
fails live validation. Network tests are skipped in the normal test suite.
