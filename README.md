# Tooldelegate — verified delegation benchmark (CLI vs MCP)

A Rust benchmark that measures whether an LLM uses fewer tokens — and produces correct artifacts more often — when it delegates deterministic transformations (sorting CSVs, aggregating rows, renaming files, rewriting text) to external tools instead of generating the artifact itself. Every run is scored by a hidden oracle that does not share the feedback verifier's implementation, and every model-supplied command runs in a Linux sandbox (bubblewrap + seccomp + rlimits, argv-only allowlist, no unsandboxed fallback). The benchmark is built to support negative findings: no arm is presumed superior.

## The MCP question (v4, planned)

The next cohort — v4, see [PLAN.md](PLAN.md) — adds an MCP interface so two effects that are confounded today can be measured separately:

- **interface**: CLI commands vs MCP tool calls
- **output mode**: full tool/verifier output vs a ≤96-byte verification certificate

Five arms: `direct` (the model writes the artifact itself), `cli-full`, `cli-cert`, `mcp-full`, `mcp-cert`. The planned MCP server exposes the same six actions the CLI schema offers (`read`, `put`, `remove`, `mkdir`, `submit`, `finish`) as MCP tools over local stdio, and every tool call executes through the same sandbox. The planned contrasts: compression benefit per interface, interface overhead at matched output mode, and delegation vs direct generation.

Why this is worth measuring: the completed v1/v3 cohorts showed delegation winning on correctness (v3: 70–74% success vs 16.7% direct) but could **not** establish token savings, because 26 of 576 v3 runs have unknown provider usage. External studies show CLI-vs-MCP differences are small and depend on harness and model, and no study crosses interface × output mode against a no-tool baseline on oracle-scored exact transformations. That gap is what v4 fills.

**Status: no MCP code exists yet.** v4 is a frozen-protocol plan awaiting owner approval, not a result. Do not cite it as evidence.

## Current status

The 48-task dataset and runner are implemented. The 40-run pilot and two separate 576-cell main-study cohorts have been executed. v1 and v3 are exploratory; v2 was aborted after 43 cells. Never pool cohorts. The v3 matrix is complete, but token differences remain unestimable because 26 runs have unknown provider usage. Therefore H1's token-saving claim is not established. See [v3 protocol and execution record](docs/protocol-v3.md) and [analysis/reproduction guide](docs/main-run.md).

**Validity limitation:** Historical v1/v3 CLI arms permitted Python; most recorded v3 commands ran model-written Python programs. These cohorts do not measure native Unix-utility-only delegation and must not be presented as evidence for it. Preserve them separately from any future native-utility study.

Native-only readiness is not yet established for the whole suite. Automated sandbox feasibility checks cover 12 filesystem cases and 11 valid-text cases; all 24 sorting/aggregation cases and the invalid-UTF-8 text case remain unproved in the sandbox. In particular, the sorting CSV includes quoted multiline fields, which ordinary line-oriented `sort` cannot safely handle. Do not start a native-only benchmark or alter the historical dataset until suitable utilities and all task variants are verified.

A host-only feasibility probe using Debian's SQLite CLI 3.46.1 passed oracle scoring for nine sorting and nine aggregation cases. SQLite is not installed or allowed inside the sandbox; the probe does not establish sandbox support or feasibility for the other cases.

V3 success rates: A 24/144 (16.7%), B 106/144 (73.6%), C 104/144 (72.2%), D 101/144 (70.1%). D−A correctness non-inferiority is supported on an exploratory basis; D is not non-inferior to B or C. These results do not establish token efficiency. The report is `main-v3-report/report.md`; v1's report is `main-report/report.md`.

A malformed `curl` header option was found after v3 and fixed with a regression test. The completed v3 run data remain unchanged. See the protocol's post-run correction before interpreting the transport-failure evidence.

## Repository layout

- `src/` — the harness (23 Rust modules): `main.rs` CLI, `orchestrator.rs` run loop and arms, `executor.rs` sandbox, `verifier.rs`/`oracle.rs` independent scoring, `certificates.rs` compact feedback, `model_adapter.rs` provider-agnostic replay/connector, `analysis.rs`/`report.rs` statistics and reports, `suite.rs`/`dataset.rs` task suite and generation.
- `tests/` — integration tests, including a mock provider and real-sandbox CLI tests.
- `configs/` — frozen experiment/analysis/run configs (JSON, schema-checked with `deny_unknown_fields`).
- `docs/` — protocol, execution, analysis, pilot and main-run records (v1–v3).
- `dataset/` — the deterministic 48-task suite (committed, regenerable via `generate-suite`).
- `main/`, `main-1..4/`, `main-v3-1..4/`, `main-v3/` — v1/v3 cohort runs. Only records, indexes, plans and protocol locks are committed; raw `runs/` trees stay on disk but are git-ignored.
- `pilot/` — the 40-run pilot. `reports/` — connector probes, smoke runs, EDA notes. `main-report/`, `main-v3-report/`, `pilot-report/` — generated reports, byte-identical on regeneration from records.
- `PLAN.md` — the v4 (CLI vs MCP) plan. Root `main-plan*.json` — frozen run matrices.

## Reproduce analysis

The v3 records are collected from indexed runs only:

```sh
./scripts/collect-main.sh NEW_RECORDS.jsonl main-v3
cargo run --locked -- analyze NEW_RECORDS.jsonl NEW_REPORT_DIR configs/analysis-v3.json
```

(The committed `main-v3/records.jsonl` is already collected; use a fresh output path — the collector refuses to overwrite existing output.) Reports are generated from JSONL records; no model calls are needed. To regenerate the deterministic dataset into a new path:

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
