# Third-party notices

## Chart rendering

Guru Terminal uses [KLineChart](https://github.com/klinecharts/KLineChart)
10.0.0, copyright (c) 2019 lihu, under the Apache License 2.0. Its upstream
NOTICE, including the TradingView Lightweight Charts attribution recorded by
KLineChart, and the Apache license text are retained with the dependency.

Guru Terminal uses [Flint](https://github.com/microsoft/flint-chart) 0.5.0,
copyright Microsoft Corporation, under the MIT License reproduced below.

Guru Terminal uses Vega 6.3.1, Vega-Lite 6.4.3, and Vega-Embed 7.1.0,
copyright the University of Washington Interactive Data Lab and contributors,
under the BSD 3-Clause License. Their complete license texts are retained with
the dependencies.

## Memory git

Guru Terminal uses [git2](https://github.com/rust-lang/git2-rs) 0.20 with a
vendored [libgit2](https://github.com/libgit2/libgit2) 1.9.7 to version Guru
Memory Markdown. git2 is available under either the MIT License or the Apache
License 2.0. libgit2 is GPLv2 with a linking exception that permits this
application to link against it. The MIT terms reproduced below apply to git2
when that option is selected.

## Pi coding agent

Guru Terminal bundles Pi coding agent 0.84.2, copyright (c) 2025 Mario Zechner,
under the MIT License:

> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## keyring

Guru Terminal uses [keyring-rs](https://github.com/open-source-cooperative/keyring-rs)
3.6.3 to store finance connector API keys in the native macOS and Windows
credential stores. keyring-rs is available under either the Apache License 2.0
or the MIT License. The applicable license texts are distributed with the
crate; the MIT terms reproduced above also apply when that option is selected.

## OpenBB financial-data runtime

Guru Terminal bundles a read-only Python sidecar built from
[OpenBB Platform](https://github.com/OpenBB-finance/OpenBB) 4.7.2 and
`openbb-mcp-server` 1.4.1. The OpenBB repository records copyright (c)
2021-2025 OpenBB Inc. and licenses all files in that repository under the GNU
Affero General Public License v3.0 (`AGPL-3.0-only`). The complete AGPLv3 terms
are distributed as Guru Terminal's top-level `LICENSE`; the sidecar's bundled
`uv.lock` identifies the exact PyPI artifacts and SHA-256 hashes.

The bundled OpenBB core, router, and analysis distributions are `openbb-core`
1.6.13, `openbb-platform-api` 1.3.6, `openbb-charting` 3.0.0,
`openbb-commodity` 1.5.2, `openbb-crypto` 1.6.2, `openbb-currency` 1.6.2,
`openbb-derivatives` 1.6.2, `openbb-econometrics` 1.7.2, `openbb-economy`
1.6.2, `openbb-equity` 1.6.2, `openbb-etf` 1.6.2, `openbb-fixedincome`
1.6.2, `openbb-index` 1.6.2, `openbb-news` 1.6.2, `openbb-quantitative`
1.6.2, `openbb-regulators` 1.6.2, and `openbb-technical` 1.6.2.

The bundled OpenBB provider extensions are `openbb-alpha-vantage` 1.6.1,
`openbb-benzinga` 1.6.1, `openbb-biztoc` 1.6.1, `openbb-bls` 1.3.1,
`openbb-cboe` 1.6.1, `openbb-cftc` 1.4.2, `openbb-congress-gov` 1.2.3,
`openbb-deribit` 1.2.1, `openbb-ecb` 1.6.1, `openbb-econdb` 1.5.1,
`openbb-us-eia` 1.3.1, `openbb-famafrench` 1.2.1,
`openbb-federal-reserve` 1.6.2, `openbb-finra` 1.6.1, `openbb-finviz`
1.5.1, `openbb-fmp` 1.6.1, `openbb-fred` 1.6.2,
`openbb-government-us` 1.6.1, `openbb-imf` 2.1.3, `openbb-intrinio`
1.6.1, `openbb-multpl` 1.3.1, `openbb-nasdaq` 1.6.3, `openbb-oecd`
1.6.1, `openbb-sec` 1.6.7, `openbb-seeking-alpha` 1.6.1,
`openbb-stockgrid` 1.6.1, `openbb-tiingo` 1.6.1, `openbb-tmx` 1.5.2,
`openbb-tradier` 1.5.1, `openbb-tradingeconomics` 1.6.1, `openbb-wsj`
1.6.1, and `openbb-yfinance` 1.6.3. Installed Core Metadata declares
`AGPL-3.0-only` for these OpenBB distributions except
`openbb-congress-gov` and `openbb-multpl`. Those two installed wheels provide
no `License`, `License-Expression`, or `License-File` field, so this notice does
not infer a separate package-level license for them; the repository-wide
OpenBB license stated above is the verified source-tree notice.

The sidecar embeds CPython 3.12 under the Python Software Foundation License
Version 2 and its historical license notices, and the PyInstaller 6.21.0
bootloader, copyright (c) 2010-2023 the PyInstaller
Development Team, 2005-2009 Giovanni Bajo, and 2002 McMillan Enterprises,
Inc., under GPLv2-or-later with PyInstaller's bootloader exception. Its MCP and
application stack includes FastMCP 3.4.7 and `fastmcp-slim` 3.4.7 under
Apache 2.0;
the Model Context Protocol Python SDK 1.29.0, copyright (c) 2024 Anthropic,
PBC, under MIT; FastAPI 0.136.3, copyright (c) 2018 Sebastián Ramírez, under
MIT; Starlette 1.6.0, copyright (c) 2018 Encode OSS Ltd, under BSD-3-Clause;
and Pydantic 2.13.4 and `pydantic-core` 2.46.4, copyright (c) 2017-present
Pydantic Services Inc. and contributors, under MIT.

Major scientific and charting dependencies include NumPy 2.5.2, copyright (c)
2005-2025 the NumPy Developers, under BSD-3-Clause with bundled components
under 0BSD, MIT, Zlib, and CC0-1.0; pandas 3.0.5 under BSD-3-Clause; SciPy
1.18.1 under BSD-3-Clause with its bundled-library notices; statsmodels 0.14.6
under BSD; scikit-learn 1.9.0, copyright (c) 2007-2026 the scikit-learn
developers, under BSD-3-Clause; Plotly 6.9.0, copyright (c) 2016-2024 Plotly
Technologies Inc., under MIT; OpenPyXL 3.1.5 under MIT; `arch` 7.2.0 and
`linearmodels` 6.1 under the University of Illinois/NCSA License; and
`exchange-calendars` 4.13.2 under Apache 2.0.

Major networking, security, parsing, and provider-client dependencies include
HTTPX 0.28.1 under BSD-3-Clause; aiohttp 3.14.3 under Apache 2.0 and MIT;
Requests 2.34.2 under Apache 2.0; cryptography 50.0.0 under Apache 2.0 or
BSD-3-Clause; certifi 2026.7.22 under MPL 2.0; curl-cffi 0.16.1 under MIT;
lxml 6.1.2 under BSD-3-Clause; Beautiful Soup 4.15.0 under MIT; yfinance 1.6.0
under Apache 2.0; `pandas-ta-openbb` 0.4.24, finvizfinance 1.3.0, and Nasdaq
Data Link 1.0.4 under MIT. The complete copyright, license, NOTICE, and
bundled-library terms in each installed distribution's `.dist-info` metadata
and license files remain applicable.

## Korea Investment Open Trading API

Guru Terminal's checked-in read-operation inventory is derived from the
[Korea Investment Open Trading API](https://github.com/koreainvestment/open-trading-api)
repository at commit `b093e42ba32d1df5f5ddad7a71cb715cbc800832`, copyright
(c) 2026 Korea Investment & Securities, under the MIT License. The MIT terms
reproduced above apply. Guru Terminal does not bundle or execute the upstream
MCP server or Python examples.

## Deno and Pyodide compute runtime

Guru Terminal bundles Deno 2.9.5, copyright the Deno authors, under the MIT
License. The MIT license text reproduced above also applies to Deno.

Guru Terminal bundles Pyodide 314.0.3 under the Mozilla Public License 2.0.
Pyodide includes CPython and other components under their respective licenses.
The MPL 2.0 license and corresponding source information are available from
the [Pyodide project](https://github.com/pyodide/pyodide); Guru Terminal does
not modify Pyodide.

## Always-on method Skills

Short valuation, comparison, historical-rule-test, and filings workflow
cards in `agent/skills/` are adapted from
[Vibe-Trading](https://github.com/HKUDS/Vibe-Trading) skills
`valuation-model`, `financial-statement`, `deep-company-series`,
`factor-research`, `research-discipline`, and `edgar-sec-filings`,
copyright (c) 2026 Vibe-Trading Contributors, under the MIT License.
The MIT terms reproduced above apply. Guru Terminal does not bundle
Vibe-Trading, its brokers, Alpha Zoo, swarms, or trade execution.

## Oh My Pi web adapters

Selected Codex, Anthropic, and xAI hosted-search request/response parsers in
`agent/guruterminal-native-search.mjs`, plus Wikipedia, Wikidata, and Crossref
URL-matcher and bounded JSON-projection patterns in
`src-tauri/src/web/official.rs`, are ported from
[Oh My Pi](https://github.com/can1357/oh-my-pi) commit
`76a294cb19bfded1e32e2111f1f729129595bf5e`, copyright (c) 2025 Mario
Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs, Inc., and contributors,
under the MIT License. The MIT terms reproduced above apply.
Guru Terminal does not bundle Oh My Pi, merge multiple official API
responses, or adopt its site-handler registry.

The offline scientific Python closure contains NumPy 2.4.3, pandas 3.0.2,
SciPy 1.18.0, statsmodels 0.14.6, scikit-learn 1.8.0, joblib 1.5.3,
packaging 26.1, patsy 1.0.2, python-dateutil 2.9.0.post0, pytz 2026.1.post1,
six 1.17.0, and threadpoolctl 3.6.0. Their BSD, MIT, Apache, PSF, Zlib, CC0,
and other applicable license notices are retained inside the bundled wheel
archives under their `.dist-info` metadata and license directories.
