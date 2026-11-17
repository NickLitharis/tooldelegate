# Verified CLI Delegation Benchmark

This Rust benchmark tests whether large language models can reduce token use by delegating deterministic transformations to command-line tools and consuming independently verified feedback. It is designed to support negative findings; no treatment is presumed superior.

## Current status

The 48-task dataset and runner are implemented. The 40-run pilot and two separate 576-cell main-study cohorts have been executed. v1 and v3 are exploratory; v2 was aborted after 43 cells. Never pool cohorts. The v3 matrix is complete, but token differences remain unestimable because 26 runs have unknown provider usage. Therefore H1's token-saving claim is not established. See [v3 protocol and execution record](docs/protocol-v3.md) and [analysis/reproduction guide](docs/main-run.md).

V3 success rates: A 24/144 (16.7%), B 106/144 (73.6%), C 104/144 (72.2%), D 101/144 (70.1%). D−A correctness non-inferiority is supported on an exploratory basis; D is not non-inferior to B or C. These results do not establish token efficiency. The report is `main-v3-report/report.md`; v1's report is `main-report/report.md`.

A malformed `curl` header option was found after v3 and fixed with a regression test. The completed v3 run data remain unchanged. See the protocol's post-run correction before interpreting the transport-failure evidence.

## Reproduce analysis

The v3 records are collected from indexed runs only:

```sh
./scripts/collect-main.sh main-v3/records.jsonl main-v3
cargo run --locked -- analyze main-v3/records.jsonl NEW_REPORT_DIR configs/analysis-v3.json
```

The collection command refuses to overwrite existing output. Reports are generated from JSONL records; no model calls are needed. To regenerate the deterministic dataset into a new path:

```sh
cargo run --locked -- generate-suite NEW_DATASET_DIR 2026
```

## Verification

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

See [PLAN.md](PLAN.md), [main experiment documentation](docs/main-run.md), [execution/security limits](docs/execution.md), and [analysis documentation](docs/analysis.md). Provider pricing remains unknown; no cost estimate is reported.
