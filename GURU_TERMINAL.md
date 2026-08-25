# V1 acceptance

Targets: macOS 13+ Apple Silicon and Windows x64. Local-first, single user, supported provider API key or account sign-in. No Guru Terminal account, no trades, no telemetry, no V1 MCP endpoint.

A release is ready when a real user can do this on a clean signed install:

1. Create a Guru and connect a model. In Marketplace, confirm that Python Compute, Finance Core, OpenBB Platform, World Bank Indicators, and Web Research need no additional connector API key once Chat is connected. SEC EDGAR needs no API key but asks only for a contact email; every other card visibly identifies its own Free account or Paid requirement and is not treated as preconfigured.
2. Research in Chat with Memory on. The Guru forms a current view from sources, then uses prior Memory as dated context, including the standing charter Lens when one exists.
3. With `Update memory` on, one ordinary turn writes a justified Wiki or Lens page. No second model session.
4. A later relevant question retrieves that page and uses it. A record that only sits in history does not pass.
5. In the same Chat, after a Finance Core calculation, turn off `Use memory` and `Update memory`, ask the Guru to publish a Markdown Artifact, and verify its Preview and Source. Do not restart or open a new Chat between the calculation and Artifact.
6. The user can revert an applied Wiki or Lens write. Evidence and Decisions can be saved as experience even when `Update memory` is off. The original Decision is not rewritten.
7. Charts stay as the current Chat artifact. Restart keeps Memory; disposable run output does not become a second knowledge store.

Steps 2–4 are the product. CI scores and history diffs are not a substitute.
