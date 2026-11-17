# Main experiment: execution and reproduction

Protocol: [protocol.md](protocol.md). Frozen settings: `configs/experiment.json`, `configs/run.luna.json`, `configs/analysis.main.json`. Dataset: `dataset/` (suite v3, seed 2026); it is committed and regenerable with `cargo run --locked -- generate-suite dataset 2026` into a new path.

## Execution

```sh
cargo run --locked -- plan-runs dataset main-plan.json 42 3
cargo run --locked -- run-plan dataset main-plan.json main configs/run.luna.json configs/connector.luna.json
```

`run-plan` creates fresh run directories, writes the normalized plan and protocol lock, and resumes from `main/index.jsonl`. It never overwrites existing evidence and stops on quota or repeated adapter failures. Quota-blocked attempts are not indexed and are retried on resume.

The actual run was split for wall time: 54 cells ran sequentially in `main/`, then the remaining 522 cells ran as four disjoint shards in `main-1/` … `main-4/` from plans split without reordering the frozen matrix. No cell was duplicated. This is an operational deviation; shard membership is not analyzed.

## Records and report

```sh
./scripts/collect-main.sh main/records.jsonl
cargo run --locked -- analyze main/records.jsonl main-report configs/analysis.main.json
```

`collect-main.sh` walks `main*/index.jsonl`, uses only indexed run directories, rejects duplicate directories, and refuses to overwrite an existing output. Partial attempts without an index entry are excluded.

Reproduce the report into a different directory and compare:

```sh
cargo run --locked -- analyze main/records.jsonl /tmp/main-report-repro configs/analysis.main.json
diff -r main-report /tmp/main-report-repro
```

## Sensitivity: provider content rejections

```sh
jq -c 'select(([.failure_events[]?.category] | index("provider_content_rejection")) | not)' \
  main/records.jsonl > main/records-no-content-rejections.jsonl
cargo run --locked -- analyze main/records-no-content-rejections.jsonl main-report-sensitivity configs/analysis.main.json
```

The sensitivity report lists excluded cells as missing. It is not the primary analysis.

## Failure handling

- Provider content and quota rejections are terminal, unretried failures with unknown usage.
- Unknown usage makes token totals lower bounds; no imputation.
- Resource-limit incompletions remain in every denominator and in the success rate.
- Per-message feedback/verification attribution is null; H3 uses total input tokens and returned feedback bytes only.
