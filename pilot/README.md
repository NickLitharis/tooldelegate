# Pilot records

Exploratory pilot: two sorting tasks (`sorting-v0-s0`, `sorting-v0-s1`), four arms, five repetitions, 40 planned runs. All 40 ran; nothing was dropped. This is not the main experiment and its intervals are not confirmatory.

```sh
cargo run --locked -- pilot-plan /tmp/unix-ai-suite configs/pilot.selection.json pilot 42 5
cargo run --locked -- pilot-run /tmp/unix-ai-suite pilot configs/run.luna.json configs/connector.luna.json
cargo run --locked -- collect-records pilot/records.jsonl pilot/runs/*
cargo run --locked -- analyze pilot/records.jsonl pilot-report configs/analysis.pilot.json
```

The plan was written before execution; `plan.json` records seed 42, the randomized matrix and the dataset manifest hash. `protocol.json` locks the run configuration and connector. `index.jsonl` holds one summary per run; `pilot-state.jsonl` records start and completion. `pilot-report/report.md` is regenerable byte-for-byte from `records.jsonl` and `configs/analysis.pilot.json`.

## Observed results

| Arm | Runs | Success | Token total | Terminations |
|---|---:|---:|---:|---|
| A direct | 10 | 0 | 1,192,730 known | 5 finished (oracle rejected), 4 resource limit, 1 adapter error |
| B raw tool | 10 | 8 | 36,496 known | 9 finished, 1 adapter error |
| C raw verifier | 10 | 9 | 37,701 known | 9 finished, 1 adapter error |
| D certificate | 10 | 7 | 91,106 known | 8 finished, 2 adapter errors |

These are point estimates over one small suite. Success non-inferiority is not established. D did not beat B or C on success at the point estimate. Arm A produced no successful artifact; four A runs hit the input-token budget while paging through the 48.9 KB input in 1,000-byte reads, and the rest wrote incorrect artefacts. Token totals exclude five adapter-error runs whose usage is unknown, so they are lower bounds.

Five adapter errors, no quota block:

- Four provider `invalid_prompt` rejections (moderation) across A/B/C/D. These are provider-side failures with unknown usage, retained as failures.
- One connector call with empty stdout in B, consistent with the harness killing the connector at its 90-second deadline. The connector now ends at `min(85s, remaining-5s)` so a timeout is reported as JSON rather than an unexplained empty response. The affected run remains unchanged in these records.

## Defects found and fixed after the pilot

- Connector timeout left no response at the harness deadline; fixed as above.
- Arm A cannot page through the 48.9 KB input within `max_input_tokens` at 1,000 bytes per read (four resource limits). The main protocol must choose the read/feedback budget or task sizes before freezing; this pilot did not change the protocol to make A win.
- Provider moderation false positives need a prespecified policy (currently they are failures, not retried).
- Per-message feedback and verification attribution remain null; H3 is not estimable from this pilot.

Do not edit raw records to remove failures. Re-run the analysis instead.
