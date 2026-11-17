# Protocol v3 (pre-registered addendum)

Frozen before execution on 2026-11-17. Settings: `configs/experiment-v3.json`, `configs/run-v3.json`, `configs/analysis-v3.json`.

## Why

The v2 replication was stopped after 43/576 cells. At the time, its failure pattern was attributed to calls cut off at the 55 s deadline; v3 increased the deadline and retained other transport changes. Post-execution review corrected that account: v2 had 6 deadline failures and 3 provider content rejections, and the effect of omitting encrypted reasoning was not isolated. v2 remains aborted evidence and is never analyzed or pooled.

## Changes from v2

| Setting | v2 | v3 |
| --- | --- | --- |
| Connector deadline | 55 s | **115 s** |
| Harness call kill | 90 s | **120 s** |
| Stream cap | 1 MiB | **2 MiB** |
| Randomization seed | 43 | **44** |

Unchanged: no encrypted reasoning content at effort `none`, 4,096-token per-call output cap, model `gpt-6-luna`, 48 suite-v3 tasks, four arms, three repetitions, 300,000 input tokens, 65,536 feedback bytes, 49,080-byte reads, one submission retry, 300 s wall, same oracle/verifier, 5-point margin, terminal moderation policy, null per-message attribution, no transport retries.

## Rules

- Streams over 2 MiB are aborted as transport failures; calls over 115 s are aborted. Either makes the run an adapter failure with unknown usage, retained in denominators.
- No transport retries. Missing usage suppresses token differences; known totals remain lower bounds.
- Whole 576-cell matrix, seed 44; disjoint shards allowed, no duplicate cells, shard membership not analyzed.
- v3 is reported separately from v1, v2, pilot and smoke runs.

## Execution record (post-run)

All 576 planned cells completed. Terminations: 477 finished, 67 resource-limit, 26 adapter-error, and 6 call-limit. Unknown usage occurred in 26 runs: 21 provider content rejections and 5 connector transport failures.

Successes: A 24/144 (16.7%), B 106/144 (73.6%), C 104/144 (72.2%), D 101/144 (70.1%). D−A success difference was +0.535 with exploratory one-sided lower bound +0.410; D was not non-inferior to B or C under the 0.05 margin. Token differences remain not estimable: 14 D−A pairs have unknown usage. Known token sums are lower bounds, not totals.

Connector traces exposed a malformed `curl` argument: the `x-codex-beta-features` value lacked its `--header` option, producing `curl: (3) URL rejected: Malformed input to a URL function` in timed-out requests. This was fixed after v3 with a request-header regression test. The execution is preserved as run; this correction cannot restore provider usage or change its records. Do not attribute all v3 transport failures to this defect without further evidence.
