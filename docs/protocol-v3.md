# Protocol v3 (pre-registered addendum)

Frozen before execution on 2026-11-17. Settings: `configs/experiment-v3.json`, `configs/run-v3.json`, `configs/analysis-v3.json`.

## Why

v2 fixed the reasoning-stream cause but set a 55 s call deadline. It was aborted after 43/576 cells because 9 runs failed on slow but bounded streams (~780 KB) cut at 55 s, not on runaway streams. v3 keeps v2's fixes, gives such calls time to finish, and reruns the whole matrix. v2 is retained as aborted evidence and is never analyzed or pooled.

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
