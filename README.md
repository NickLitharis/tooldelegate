# Verified CLI Delegation Benchmark

This project will investigate when delegating deterministic transformations to CLI tools reduces model token consumption without sacrificing correctness. It is a scientific investigation, not a demonstration: negative results and failures must remain in the analysis.

**Status: offline harness, exploratory analysis, live total-token accounting and the pilot executor are implemented; one live interface smoke run succeeded; no pilot runs yet.** Dataset generation, hidden scoring, Linux sandbox execution, feedback verification, certificates, four-arm orchestration, provider-neutral adapters, a GPT-6 Luna connector with provider-reported usage, resumable pilot planning/execution, JSONL telemetry and reproducible reports are available. The arm-D smoke run on `sorting-v0-s0` finished in 5 calls and 3,933 tokens with verifier and oracle agreement; it is not pilot data. Per-message token attribution, pilot results and main benchmark results remain outstanding. See the [execution restrictions](docs/execution.md), [model-run contract](docs/model-runs.md), [pilot workflow](docs/pilot.md) and [analysis instructions](docs/analysis.md).

A **48-task dataset generator** is available: `cargo run --locked -- generate-suite DESTINATION SEED`. See [dataset coverage, scoring commands and limitations](tasks/README.md). The suite is not yet frozen for the main experiment.

Generate a preliminary numeric sorting input with `cargo run -- /tmp/opencode/sorting-example 2026 10`. The destination must not exist. This is not yet the frozen benchmark dataset; its generator differs from the earlier Python prototype.

The preliminary hidden-oracle check can score this task with `cargo run -- score INPUT.csv OUTPUT.csv`. It verifies ordering, row preservation, schema, and stable ties. This local diagnostic must not be exposed to models during experimental runs.

A preliminary exact text replacement case can be generated with `cargo run -- generate-text DESTINATION SEED` and scored with `cargo run -- score-text INPUT.txt OUTPUT.txt`. This is a prototype, not the completed text task family.

Additional preliminary family cases:

| Case | Generate | Score |
| --- | --- | --- |
| Filesystem rename | `cargo run -- generate-filesystem DESTINATION SEED` | `cargo run -- score-filesystem BEFORE_TREE AFTER_TREE` |
| Null-sensitive filtered count | `cargo run -- generate-aggregation DESTINATION SEED ROWS` | `cargo run -- score-aggregation INPUT.csv OUTPUT.json` |

For filesystem tasks, score against an immutable copy of the original `input/` directory, not the modified tree. These checks run locally and are not a sandbox: no model-controlled execution is permitted yet. The aggregation scorer checks only the result; whole-run preservation must additionally compare input snapshots when the harness is implemented.

## Pilot

The pilot matrix is planned separately from execution, so quota exhaustion cannot change the randomization or silently drop runs:

```sh
cargo run --locked -- pilot-plan DATASET configs/pilot.selection.json PILOT_DIR 42 5
cargo run --locked -- pilot-run DATASET PILOT_DIR configs/run.luna.json configs/connector.luna.json
```

The first command writes the 40-run randomized plan and spends no quota. The second executes it, resumes from `index.jsonl` after interruption, stops on subscription-quota exhaustion, and never scores a quota-blocked attempt. See [pilot planning and execution](docs/pilot.md) for records, stop behavior and limitations.

## Planned experiment

The same model, decoding settings, task requirements, inputs, and retry budget will be used for four randomized arms: direct artifact generation (A), CLI with bounded raw output (B), CLI with bounded raw verifier feedback (C), and CLI with a compact verifier certificate (D). An independent hidden oracle will score every arm. Complete prompts, schemas, run traces, resource usage, and provider-reported token counts will be retained outside the model context.

The intended dataset has 12 tasks in each of sorting, filesystem, text, and structured queries. The pilot has two fixed sorting tasks, four arms, and five repetitions (40 runs). The main design has 48 tasks, four arms, and at least three repetitions (576 runs). The protocol and task definitions must be frozen before the main experiment.

The primary comparison is D versus A for both tokens and success. A 5 percentage point success-rate non-inferiority margin is proposed, but must be justified and frozen before the main study. Token savings alone cannot establish improvement. Failed and incomplete runs will be included; token efficiency is total tokens across all runs divided by successful runs and is undefined when there are no successes. Repetitions will be clustered by task instance for uncertainty estimates.

## Prerequisites and gates

1. Implement and pass the deterministic infrastructure tests, including sandbox escape and timeout tests, before making model calls.
2. Verify the selected GPT-6 Luna model through ChatGPT Plus access. Fix its model version, decoding configuration, token accounting and usage limits. Pricing is deferred by user request. Monetary cost remains unknown, not zero.
3. Run and inspect the pilot. Fix infrastructure defects before freezing the main protocol; do not alter hypotheses to match pilot outcomes.
4. Approve the cost and execute the frozen main protocol. Generate reports solely from saved raw records.

Draft settings are recorded in `configs/experiment.json` and `configs/models.json`. Null fields are unresolved decisions, not defaults; `configs/experiment.json` is not yet consumed by an experiment runner, and individual runs use `configs/run.luna.json` plus `configs/connector.luna.json`. The proposed 5-point margin must be justified before protocol freeze.

See [PLAN.md](PLAN.md) for the implementation sequence. Run the current checks with `cargo test`; they do not validate benchmark functionality.
