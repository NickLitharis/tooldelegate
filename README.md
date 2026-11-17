# Verified CLI Delegation Benchmark

This project will investigate when delegating deterministic transformations to CLI tools reduces model token consumption without sacrificing correctness. It is a scientific investigation, not a demonstration: negative results and failures must remain in the analysis.

**Status: main experiment complete (576/576 cells); exploratory report generated.** Success: A 23/144 (16.0%), B 104/144 (72.2%), C 94/144 (65.3%), D 95/144 (66.0%). Known tokens: A 5,034,745, B 3,052,082, C 2,532,189, D 1,357,952, with 97 unknown-usage adapter errors. D−A success non-inferiority is supported at the point estimate (lower bound 0.389 vs 0.05 margin), but the token difference is not estimable, so H1 is not established. D is not non-inferior to B or C. Reports: `main-report/report.md` (regenerates byte-identically) and `main-report-sensitivity/`; records and reproduction: [main/README.md](main/README.md). Frozen settings: [docs/protocol.md](docs/protocol.md). The analyzer labels all reports exploratory; this is not a preregistered confirmatory result. Pricing is deferred, so monetary cost is unknown.

A **48-task dataset generator** is available: `cargo run --locked -- generate-suite DESTINATION SEED`. The frozen suite v3 dataset is committed under `dataset/`. See [dataset coverage, scoring commands and limitations](tasks/README.md).

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
