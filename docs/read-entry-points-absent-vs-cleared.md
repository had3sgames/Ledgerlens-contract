# Read entry points: never-submitted vs cleared (issue #1396)

Two admin operations remove data, and they remove *different* things:

- `clear_score` removes the latest `Score` entry, the wallet's pair-list entry
  and the score-entry index entry.
- `clear_score_history` removes only the `ScoreHistory` ring.

Neither touches the lifetime records: the per-(wallet, pair) submission
counter, the last-submit timestamp, the submission provenance, or the
per-pair total. So a wallet whose score was cleared is **deliberately
distinguishable** from one that never submitted, but only through those
lifetime records. Every other read must look identical.

Executable form: `contracts/ledgerlens-score/src/test_absent_vs_cleared.rs`.
Deletion semantics, audit events and approval policy are in
`docs/privacy-model.md` and `test_deletion_policy.rs`; they are not repeated.

**Adding a read entry point?** Add a row here, a field to `Observed` in the
test file, and the expected value in each state test.

| Entry point | Never submitted | After `clear_score_history` | After `clear_score` | After both | Intent |
|---|---|---|---|---|---|
| `get_score` | `ScoreNotFound` | latest score | `ScoreNotFound` | `ScoreNotFound` | live-score read; cleared == absent |
| `get_score_exists` | `false` | `true` | `false` | `false` | live-score read |
| `get_score_opt` | `None` | `Some` | `None` | `None` | live-score read |
| `get_score_history` | empty | empty | **retained** | empty | history is independent of the latest score |
| `get_score_variance` | `0` | `0` | **retained** | `0` | derived from history |
| `get_score_count` | `0` | lifetime count | **lifetime count** | **lifetime count** | monotonic lifetime counter; never reset by clears |
| `get_pair_score_count` | `0` | lifetime count | lifetime count | lifetime count | monotonic per-pair total |
| `get_wallet_pair_list` / `get_wallet_pair_count` | empty / `0` | listed / `1` | empty / `0` | empty / `0` | follows the live score |
| `get_score_age` | `0` | time since last submit | **time since last submit** | **time since last submit** | last-submit timestamp survives clears; `0` means "never submitted" only |
| `get_submission_provenance` | `ScoreNotFound` | present | **present** | **present** | records the last accepted submission, not the live score |
| `get_aggregate_score` | `ScoreNotFound` | aggregate | `ScoreNotFound` | `ScoreNotFound` | follows the live score |
| `query_risk_gate` | `false` (fail closed) | evaluated | `false` | `false` | cleared == absent == fail closed |
| `assign_risk_cluster` | `ScoreNotFound` | cluster | `ScoreNotFound` | `ScoreNotFound` | follows the live score |

**Bold** cells are where "cleared" differs from "never submitted". They are
intended behavior that was previously undocumented; no entry point was found
to conflate the two states incorrectly, so no code change was needed.

Consumers that must treat a cleared wallet as unknown should use
`get_score_exists` / `get_score_opt`, **not** `get_score_count` or
`get_score_age`, which report lifetime facts.
