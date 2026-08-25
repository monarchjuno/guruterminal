# Knowledge and Memory

Current Markdown kinds:

| Kind | Role |
| --- | --- |
| Wiki | Reusable facts about companies, markets, and concepts |
| Lens | How this Guru invests: lessons, limits, what would prove it wrong |
| Evidence | Dated claims selected from exact current-turn results, with host-recorded source receipts |
| Decision | A point-in-time judgment the Guru can learn from without rewriting history |

Decision and Evidence are experience. Wiki and Lens are learned state that later
Chat must use. One reserved Lens page (`lens:charter`) is the Guru's standing
investment philosophy: when Use memory is on, Chat injects that page into the
turn. A raw Tool result is not itself Evidence. The agent creates Evidence by
selecting the exact values used by a claim from a delivered current-turn
`result_ref`; Rust validates the JSON Pointers and records the immutable source
receipt.

## Learning

Self-improvement is the product. It is retrieval-driven change in how this Guru researches and reasons, not model-weight training.

1. New claims need Evidence created from delivered results in the current turn. Recall and review may start from Memory.
2. Exact-read a record before relying on it. Current evidence wins conflicts.
3. With `Update memory` on, justified reusable deltas become Wiki or Lens in the same Chat turn. A Decision is optional.
4. A later relevant turn retrieves and applies that Wiki or Lens. Retrieval is hybrid: lexical search first, then offline static embeddings when that pass is empty. Exact-read is content authority. It is not ticker-only and not a live embedding API.

A stored page or a right/wrong review that does not change later work is not learning.

`Use memory` controls retrieval. `Update memory` authorizes Wiki or Lens writes.
Both default on. Retrieval off is a complete research baseline. A Wiki or Lens
patch may cite Evidence created in the turn or Memory that was exact-read in
the turn. Explicit Evidence and Decision still persist when `Update memory` is
off.

`$wiki` and `$lens` are Chat Skills that lock both Memory switches on. They accelerate the same loop; they are not another mode.

## Decisions

A Decision records the conclusion, rationale, uncertainty, and the inputs actually used. Relationships: `uses` (Wiki/Lens), `supports` (Evidence), `updates` / `contradicts` (earlier Decisions). The original is never mutated. Lessons go into Wiki or Lens.

For a non-abstain judgment, `decision_submit` accepts only Evidence IDs created
in the same turn; a raw `result_ref` or an older Evidence record is not a
Decision citation. An abstain Decision may omit Evidence when it records the
reason and uncertainty. Exact-read Wiki and Lens IDs may be listed as used
context. An explicit submission becomes canonical Decision even when `Update
memory` is off.

## Workspace

Users may revise Wiki and Lens, including the charter Lens, in Memory. New Wiki and Lens pages are written from Chat. Evidence and Decision content stays immutable. The Guru workspace keeps Memory files in git; Rust commits after every canonical write. A later Chat turn can read a prior version. The user can revert an applied Wiki or Lens write to those prior bytes. Search finds pages; exact read is content authority. Record IDs are topic slugs without time strings. Time belongs in `as_of` or `period`.

### Format compatibility

`workspace.json` schema v1 is the public v0.0.1 workspace contract. It stays
strict and append-only until a replacement is explicitly shipped. A future
schema change must include a Rust-owned, idempotent workspace migration that
first creates a Git recovery point, preserves the user-owned Markdown bytes,
and verifies the migrated workspace before normal commands use it. Never
silently recreate a user workspace or make an unknown schema permissive merely
to open it.
