# Bundled capabilities

Marketplace is the official plugin marketplace shipped with the app. It does
not install packages. Agents controls which configured capabilities each Guru
may run. Community plugins and Memory libraries (shared Wiki and Lens packs)
are coming soon in the UI only.

Credentials stay in the OS keyring. None of those values is a Tool argument,
process argument, receipt, or result. A configured connector is still off for
a Guru until the user enables it in Agents.

Skills are reviewed workflows (`research`, `wiki`, `lens`, `decision`), not
learned expertise. Hidden analysis skills (`valuation`, `filings`,
`comparison`) load for Chat when they match the work. They are not user-visible
or mentionable. Wiki and Lens learning happens through Memory, not by rewriting
Skills.

| Runtime | Catalog entries | Agent Tools |
| --- | --- | --- |
| Compute | `guruterminal.compute-python` | `compute_run` |
| Deterministic finance | `guruterminal.finance-core` | `finance_sources`, `finance_calculate` |
| OpenBB MCP | `openbb.platform` and enabled OpenBB provider entries | Dynamically discovered `mcp__openbb__*` read Tools |
| Native macro | `world-bank.indicators` | `finance_macro_data` |
| Native Korea markets | `krx.market-data`, `koreainvestment.market-data` | `finance_market_data` |
| Native Korean filings | `opendart.disclosures` | `finance_company_data`, `finance_filings`, `finance_resolve_entity` |
| Web | `community.web-research` | `web_search`, `web_fetch` |

The official source lives in
`apps/guruterminal/marketplace/marketplace.json` plus
`apps/guruterminal/marketplace/plugins/<name>/`. Each plugin has
`.guruterminal-plugin/plugin.json` and capability connectors at the plugin
root. The loader flattens those connectors into capabilities. Entries declare
a plugin, runtime kind, providers, credential mapping, network hosts, and
verification probe. Marketplace changes configuration and permission; the
OpenBB runtime and provider packages ship in the app.

`openbb.platform` contains every provider marked keyless by the pinned runtime
manifest and is enabled for a new Guru. SEC is included there, while its
separate `sec.edgar` entry collects the non-secret contact email required by
SEC network policy; SEC calls fail closed until that setup is present. FRED,
Alpha Vantage, and other credentialed OpenBB vendors keep their own Marketplace
entries so credentials, terms, and network access remain explicit. All enabled
OpenBB entries share one Guru-scoped `mcp/openbb` component. The agent loads it
with `capability_load`, after which the host exposes only allowed read-only
Tools under `mcp__openbb__*`. Sequential turns of the same Guru and authority
reuse the warm process; the host resets it to the admin-only control surface
before the next load so activation does not leak. Concurrent turns do not share
one process. The process is not app-global and is not tied to one Chat thread.

OpenBB Tool names and response shapes are discovered at runtime. When a Tool
supports several providers, the agent selects one explicitly. Rust filters the
provider choices to the current Guru's enabled entries and rejects a returned
provider that does not match the request or captured permissions. File writes,
package or Skill installation, configuration mutation, and trading Tools are
not exposed.

OpenBB runs in a private process owned by the Guru-scoped warm pool. Rust
supplies only the current Guru's active provider credentials through a private
bootstrap channel and gives the process private configuration and cache paths
that outlive a single turn. Provider credentials follow candidate, verified,
and active states; verification uses a short-lived read-only probe.

The retained native World Bank, OpenDART, KRX, and Korea Investment connectors
use the same result lifecycle as OpenBB, web, compute, and deterministic
finance. Any successfully delivered read result can be selected into Evidence
or mapped into a chart; provider identity is receipt metadata, not an
eligibility class.

`finance_sources`, capability discovery/loading, MCP administration, Skill
instruction reads, and `run_results_list` are control metadata rather than
data reads, so they do not receive `result_ref` values.

The deterministic finance worker has no network access. Compute has no network,
host filesystem, or Memory access. Korea Investment is read-only; orders are
not in its inventory. Web search routing is a Marketplace setting rather than
a model-selected provider.

### Maintaining the Korea Investment read catalog

`marketplace/kis-read-api-v1.json` is a checked-in, deterministic projection of
the pinned `koreainvestment/open-trading-api` revision. It is not refreshed at
runtime. When reviewing an upstream API update, use a clean checkout at the
commit pinned in `scripts/generate-kis-read-api.py`, then first prove that the
current file still matches:

```sh
python3 apps/guruterminal/scripts/generate-kis-read-api.py \
  --source /absolute/path/to/open-trading-api --check
```

After an intentional, reviewed pin update, run the same command without
`--check` and commit the generated catalog with its connector tests. The
generator rejects a dirty or wrong upstream revision, every non-GET operation,
and any unexpected change to the reviewed write/read inventory.
