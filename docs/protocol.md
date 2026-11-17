# Frozen protocol (main experiment)

Status: **frozen 2026-11-17**, before the main run. Machine-readable settings: `configs/experiment.json`, `configs/run.luna.json`, `configs/analysis.main.json`.

## Question and hypotheses

Does delegation to CLI tools with independently verified, compressed feedback reduce model tokens without sacrificing correctness? H1 efficiency/non-inferiority; H2 size dependence; H3 verification-feedback overhead (raw versus certificate); H4 invocation-complexity limitations. All are evaluated independently; no advantage is assumed.

## Model and access

- Model: `gpt-6-luna` via ChatGPT Plus OAuth through the benchmark connector; resolved model must equal the request.
- Decoding forwarded: `{"max_output_tokens":8192,"reasoning_effort":"none"}`. `temperature` is rejected by this endpoint; provider defaults (`temperature=1.0`, `top_p=0.98`) are not controllable and are a declared limitation.
- Usage is provider-reported only. Pricing is null by instruction; monetary cost is unknown, not zero.
- Provider moderation rejections are **terminal failures**: no retry, recorded as adapter errors with unknown usage, retained in every denominator. A sensitivity analysis may exclude only provider-rejection runs and is labelled as such.

## Dataset

- Generator: `cargo run --locked -- generate-suite dataset 2026` (suite-v3). Seed 2026, 48 tasks, 12 per family, four variants at three sizes (10, 200, 500 units). Largest input is 44,023 bytes.
- Sizes were reduced from the pilot's suite-v2 before freeze so the direct arm is not censored by context mechanics unrelated to the transformation. This is a protocol design decision, not a result-driven exclusion; the pilot is not pooled with the main data.
- The frozen dataset directory is committed; its `dataset.json` hash is stored in the main plan and copied into every run record.

## Arms

A direct generation (`put` chunks), B CLI with bounded raw tool output, C CLI with bounded raw verifier output, D CLI with the bounded verification certificate. All arms use the same requirements, inputs, model, decoding, retry budget, wall budget and hidden oracle. Reads return UTF-8 text, or base64 bytes when the range is not valid UTF-8.

## Limits

- 64 model calls, 1 retry, 300,000 cumulative input tokens, 128,000 output tokens, 400,000 per-call context, 300 s wall, 65,536 feedback bytes, read chunks up to 49,080 bytes.
- Per model call the connector stops at `min(85 s, remaining-5 s)`; the harness kills it at `min(90 s, remaining)`. A call that exceeds this is an adapter failure with unknown usage, never a completed run.
- Tool execution: Linux bubblewrap, allowlisted single-process commands, hard CPU/memory/file limits. Workspace disk budgets are sampled, not quotas.

## Randomization and execution

- Matrix: 48 tasks × 4 arms × 3 repetitions = 576 runs.
- Randomization: SplitMix64 Fisher–Yates, seed 42, independent of dataset generation.
- `plan-runs dataset main-plan.json 42 3` creates the plan; `run-plan dataset main-plan.json main configs/run.luna.json configs/connector.luna.json` executes it.
- The executor creates fresh run directories, locks the configuration in `main/protocol.json`, resumes from `main/index.jsonl`, never overwrites evidence, and stops on quota, setup errors or three consecutive adapter failures. Quota-blocked attempts are retried later and are never scored.
- Operational deviation: after 54 sequential runs, the remaining 522 cells were executed as four disjoint shards of the same frozen plan (`main-1` … `main-4`) to reduce wall time. Sharding preserves the frozen seed and cell assignment; no cell was duplicated, and shard membership is not an analysis variable. Concurrent provider load is a recorded threat, not a treatment condition.

## Analysis

- Primary: D versus A on total tokens and success non-inferiority; 5-percentage-point margin, frozen before execution with the justification in `configs/analysis.main.json`.
- Task-cluster bootstrap over 48 tasks, preserving paired repetitions; secondary comparisons are exploratory and not multiplicity-adjusted.
- Failures, incomplete runs and unknown-usage runs remain in denominators. Known token totals are lower bounds when any usage is missing; no imputation.
- H3 proxy: total input tokens for C versus D plus returned feedback bytes. Per-message feedback and verification attribution are null and must not be presented as measured.
- Provider moderation failures are reported separately and in a labelled sensitivity analysis. Build the sensitivity cohort from collected records, which classify `adapter_error` events:

```sh
jq -c 'select(([.failure_events[]?.category] | index("provider_content_rejection")) | not)' main/records.jsonl > main/records-no-content-rejections.jsonl
cargo run --locked -- analyze main/records-no-content-rejections.jsonl main-report-sensitivity configs/analysis.main.json
```

  That report will list the excluded cells as missing; it is a sensitivity view, not the primary analysis.

## Reporting rules

Reports are generated only from `main/records.jsonl` and the frozen analysis config, must regenerate identically, and must state negative findings. The pilot and smoke runs are excluded. Charts and the 14-section report come from the existing analyzer.

## Known threats

Provider false-positive moderation; uncontrollable decoding defaults; shared ChatGPT Plus quota; no per-message tokenizer attribution; input bytes as an artifact-size proxy; sampled workspace disk budgets; one model version only.
