# Protocol v2 (pre-registered addendum)

Frozen before execution on 2026-11-17. Machine-readable: `configs/experiment-v2.json`, `configs/run-v2.json`, `configs/analysis-v2.json`. v1 remains unchanged in `docs/protocol.md`; v2 is reported separately and is never pooled with v1 or the pilot.

## Why a v2 exists

The v1 main run had 97/576 cells with unknown token usage: 81 transport failures (runaway streams killed at the call deadline) and 16 provider content rejections. Under the frozen v1 rules those totals are lower bounds, and the D−A token difference was not estimable. v2 changes only the transport limits, to test whether the failure mode is avoidable, and reruns the whole matrix.

## Changes from v1

| Setting | v1 | v2 |
| --- | --- | --- |
| Per-call model output cap | 8192 | **4096** |
| Connector deadline | 85 s | **55 s** |
| Provider stream cap | none (8 MiB envelope after completion) | **1 MiB, abortive** |
| Randomization seed | 42 | **43** |
| Transport retries | none | **none** |
| Encrypted reasoning request | always when effort was set | **omitted at effort `none`** |

The v1 transport failures were traced to requesting `reasoning.encrypted_content` while `reasoning_effort` was `none`: the provider streamed large encrypted-reasoning payloads, calls ran to the deadline, and usage was lost. v2 omits that request at `none`; a smoke run of the largest sorting task then completed in 7 calls and 21 s with complete usage instead of aborting at 55–85 s.

Everything else is frozen and identical: model `gpt-6-luna`, reasoning `none`, 48 suite-v3 tasks, four arms, three repetitions, 300,000 cumulative input tokens, 65,536 feedback bytes, 49,080-byte reads, one submission retry, 300 s wall, same oracle and verifier, same 5-point margin and analysis rules.

## Pre-specified transport policy

- A stream over 1 MiB is aborted immediately and recorded as a transport failure.
- A call that reaches 55 s is aborted; the run becomes an adapter failure with unknown usage.
- Transport failures are **not retried**. Retrying after an aborted stream cannot recover that call's token usage, so usage would stay incomplete; v2 tests whether the cap removes the failure mode instead.
- Provider content rejections remain terminal, unretried failures with unknown usage and remain in every denominator. The sensitivity analysis excludes only those runs.
- Missing usage still suppresses token differences; known totals remain lower bounds. No imputation.

## Execution and analysis

```sh
cargo run --locked -- plan-runs dataset main-plan-v2.json 43 3
cargo run --locked -- run-plan dataset main-plan-v2.json main-v2 configs/run-v2.json configs/connector.luna.json
```

The whole 576-cell matrix is rerun; no cell is selected on v1 outcomes. Execution may be split into disjoint shards of the same frozen plan (no duplicate cells); shard membership is not analyzed. Records and reproduction follow `docs/main-run.md` with `main-v2*` and `configs/analysis-v2.json`. v2 was aborted after 43 cells, so no `main-v2-report` exists; v2 data are never pooled with v1/v3.

Primary analysis is unchanged: D versus A on total tokens and success non-inferiority at a 5-point margin, with failures and unknown usage retained. H2 and H3 remain unavailable unless token usage is complete and attribution is null-safe; H4 remains observational.
