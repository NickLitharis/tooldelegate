# Pilot planning and execution

The pilot is two fixed sorting tasks × four arms × five repetitions = 40 live runs. Planning is separate from execution so a subscription-quota block cannot change the randomization or the matrix. The pilot is still not the frozen main protocol; it exists to expose harness defects.

## Commands

```sh
cargo run --locked -- pilot-plan DATASET SELECTION PILOT_DIR SEED REPETITIONS
cargo run --locked -- pilot-run DATASET PILOT_DIR RUN_CONFIG CONNECTOR_CONFIG
```

Selection file:

```json
{"tasks": ["sorting-v0-s0", "sorting-v0-s1"]}
```

`PILOT_DIR` must not exist when planning. Task ids must be safe, unique and present in `DATASET/dataset.json`. Preparation writes `plan.json` only; it spends no model quota.

Execution reads that plan, verifies the dataset manifest hash, and writes `protocol.json` on the first attempt containing the exact run and connector configuration. A later attempt with a different configuration is rejected instead of silently mixing parameters. Each planned run gets a fresh directory under `runs/`, and the orchestrator's isolation, scoring, retry and telemetry rules still apply.

## Resume and stop behavior

- `index.jsonl` is the execution log; one terminal line per completed plan entry.
- A crash after a run finished but before its index line is recovered from `summary.json` instead of repeating live calls.
- A run stopped by `usage_limit_reached` gets `quota-blocked.json` and **no** index entry. It is retried on resume in a new `-a2` attempt directory. Quota evidence is retained but never scored.
- The pilot stops with a nonzero exit on quota exhaustion, after a run-setup error, or after three consecutive `adapter_error` runs.
- A resumed pilot must use the same run configuration and connector. Use `pilot-plan` again only when a different design is intended.

## Records

| Path | Contents |
| --- | --- |
| `plan.json` | seed, algorithm, repetitions, selected tasks, dataset hash, randomized matrix |
| `protocol.json` | exact run and connector configuration used for this pilot |
| `index.jsonl` | per-entry status and the complete orchestrator summary |
| `pilot-state.jsonl` | start, blocked and complete events |
| `pilot-summary.json` | derived counts only: planned/executed/remaining, success, oracle pass, incomplete, usage completeness, calls, tokens per arm |
| `runs/` | full traces, tool logs, work artifacts and summaries |

Raw run records remain authoritative; `pilot-summary.json` is regenerated from the index and must not be treated as raw data.

## Checks

```sh
cargo test --locked --bin tooldelegate pilot
cargo test --locked --test pilot_cli
```

The unit test covers reproducible planning, all-arm matrix completeness, contiguous indices and selection rejection. The CLI test runs all four arms through the real sandbox and scoring path against a local provider double, resumes a completed pilot without spending calls, verifies quota stop/resume in a new attempt, and rejects configuration drift.

## Limitations

The pilot uses provider-reported totals only; feedback and verification token attribution remain null. The ChatGPT Plus quota is shared with interactive OpenCode use, so an interrupted pilot is expected rather than exceptional. The plan records randomization but does not freeze hypotheses, the non-inferiority margin or the main protocol. The interface was smoke-tested once per arm on `sorting-v0-s0`: B, C and D succeeded; A's interface worked but the model wrote a wrongly ordered artifact. See `reports/smoke-run.json`. These runs are not pilot data.
