# Analysis and reports

The analysis reads saved run records. It does not call a model or execute a transformation. All current test data are synthetic. They test the software, not the research hypotheses.

## Collect records

A run directory contains `record.jsonl`, `trace.jsonl`, tool logs and final artifacts. Use the directories from the same study configuration.

```sh
cargo run --locked -- collect-records NEW_RECORDS.jsonl RUN_DIRECTORY_1 RUN_DIRECTORY_2
```

The destination must not exist. The command does not change the original directories. It adds source trace hashes, environment metadata, attempted command counts, argument volume and classified error events.

If a run lacks a summary, the collector reads its start event and saved responses. It records that run as interrupted. Known token counts remain lower bounds. Completion, oracle outcome and unavailable costs remain unknown or failed, as appropriate. It never substitutes a successful result.

A partial final trace line is marked as truncated. Its bytes remain part of the source hash. Corruption before that final line stops collection. A partial summary file is recovered only when it exactly matches a prefix of the complete summary in the trace. Conflicting summaries stop collection.

If only summary records are available, analysis still works. Missing trace classifications remain unknown. Keep the original directories for complete prompts, provider payloads and diagnostics.

## Generate a report

Copy `configs/analysis.example.json` to a new configuration file. Set the expected task count and repetitions for the study. The defaults describe the 48-task main design, not the two-task pilot. Set the pilot count to 2 and its repetitions to 5 when analyzing pilot records.

```sh
cargo run --locked -- analyze RECORDS.jsonl NEW_REPORT_DIRECTORY ANALYSIS_CONFIG.json
```

The report directory must not exist. The analyzer rejects invalid records before it creates that directory. It rejects duplicate run IDs, duplicate matrix cells, changed task definitions and incompatible model configurations. It also rejects identical inputs and requirements presented under different task IDs. It never treats those duplicates as independent tasks.

Synthetic data require `allow_synthetic: true`. Mixed synthetic and real records are rejected. Every synthetic report states that it contains test fixtures, not research measurements. All reports currently remain exploratory because the protocol is not frozen.

The output contains:

- `records.jsonl`, an unchanged copy of the analysis input.
- `config.json`, the normalized analysis configuration.
- `summary.json`, the statistics, coverage, diagnostics and source hashes.
- `report.md`, the research report with 14 sections.
- `report.html`, a local HTML view with no external scripts.
- Seven SVG charts for tokens, success, size, verification overhead, context, efficiency and latency.

Run the command again with the bundled input and configuration to reproduce the report. Use a different output directory. Tests compare every output byte across two generations.

## Measurements and missing data

Arm summaries retain all supplied runs. Failures and incomplete runs remain in their denominators. Missing matrix cells and missing task instances are reported separately. A missing record is not silently converted into a measured zero-token failure. Collect its trace when possible.

Total model tokens equal input plus output. Cached input remains part of input tokens. Token efficiency equals total tokens across all runs divided by successful runs. It is undefined when there are no successes or when total usage is incomplete.

Known token totals are lower bounds when an API call lacks usage. Exact token comparisons then remain unavailable for affected pairs. Context peaks also remain uncertain. Cost requires complete usage, cache counts and recorded pricing. Missing cost is not zero cost.

Verifier input tokens, CPU, elapsed time, peak memory and retries are separate quantities. CPU and memory are never converted into model tokens. Latency means use available measurements and report their missing counts. Interrupted runs remain in success counts even when their duration is unknown.

## Exploratory token-usage missingness

Generate a separate descriptive diagnostic from saved run records:

```sh
cargo run --locked -- usage-missingness RECORDS.jsonl NEW_MISSINGNESS.md
```

The output path must not exist. The report breaks incomplete-usage rates down by arm, task family, generator size parameter, expected outcome and complexity. It reports failure-event categories and known partial token subtotals already present in the records. A zero subtotal is not evidence of zero token consumption. No failed-call usage is estimated, and this diagnostic does not change the primary report or make affected paired comparisons exact. Size parameters are family-specific settings, not a shared artifact-size scale. Treat all subgroup rates as descriptive.

## Paired comparisons and uncertainty

A pair uses the same task instance and repetition. The comparisons are D versus A, B versus A, C versus B, D versus C and D versus B. Each also appears within family, size, semantic complexity and expected-outcome groups.

A cluster contains one task instance and its paired repetitions. The bootstrap samples whole clusters with replacement. It never treats repeated runs as independent task instances. Effects use equal task weights. Missing pairs remain visible and prevent a complete-design support claim.

The default bootstrap uses 2,000 samples and a recorded seed. `summary.json` records two-sided 95% percentile intervals. Non-inferiority uses a separate one-sided 95% lower bound. The default minimum is 10 clusters. Fewer clusters still produce descriptive statistics, but not a support claim.

Success non-inferiority means that the success-rate loss stays below the chosen margin. The proposed margin is 0.05, or five percentage points. Its practical justification is a draft. Approve and freeze the margin and method before the main study. Do not choose them to match observed treatment advantages.

The code adds a conservative risk guard for all-nonnegative or constant success differences. It prevents a degenerate bootstrap from implying zero unseen failure risk. With n clusters and minimum difference m, the added lower bound is:

```text
m * 0.05^(1/n) - (1 - 0.05^(1/n))
```

The decision uses the smaller lower bound. This guard can leave non-inferiority unresolved even when every observed difference is nonnegative. For example, 48 perfect ties do not establish a five-point margin under this method. Review this conservative design choice before protocol freeze. Its interpretation depends on task-sampling assumptions.

Token reduction and success non-inferiority are separate requirements. A lower token interval alone does not establish H1. Synthetic data establish neither requirement as a research finding. Secondary comparisons lack multiplicity correction and remain exploratory.

## Size and crossover model

The boundary model uses nalgebra SVD, a matrix method for least-squares estimation. It fits log token ratio against centered log input bytes. It includes interactions with family, semantic complexity and expected failure when those factors vary.

Input bytes are a proxy for artifact size. They are not final-output byte counts. Failed and incomplete runs stay in the paired data. Each size point includes both arms' success rates. The model describes token cost, not correctness-adjusted efficiency.

Nonpositive token ratios cannot enter a log model. The report counts those exclusions. Unknown paired usage prevents the fit rather than selectively removing expensive failures. A rank-deficient design produces no fit. Bootstrap fits resample task instances. Intervals require at least 80% identifiable fits.

A crossover needs at least three distinct measured sizes in its stratum. Its point estimate must stay within that range. Root intervals use all finite bootstrap roots, not only roots that fall in range. An interval remains unavailable if it leaves the range or its slope is unstable. The code never reports an extrapolated crossover as a measured boundary. Numerical slopes below `1e-10` do not support a decreasing-token-ratio claim.

## Failures and validity

Terminations count runs. Error categories count encountered events. Those events are not mutually exclusive final failures because retries can recover. Known harness messages receive deterministic categories. Other execution errors remain unclassified. Verifier diagnostics are marked as verifier reports, not oracle truth.

Verifier false-positive rates use oracle-negative evaluated runs as their denominator. False-negative rates use oracle-positive evaluated runs. Worker failures have no verifier judgment. A zero count does not prove perfect verification.

These rates compare the last feedback judgment with the final oracle outcome. Intermediate verifier diagnostics do not receive an independent oracle label here.

Task instances at different sizes can still share one semantic recipe. Their correlation is a limitation of task-instance clustering. Semantic task complexity also differs from actual command complexity. Command request counts and argument bytes are observational proxies, not a causal test of H4.

The H4 summaries correlate log serialized argument volume with interface errors, success and paired D/A token savings. Their bootstrap retains all arms within each task. Missing trace groups remain counted. Zero variation produces no correlation estimate.

The report lists negative point estimates and unestablished requirements. It does not remove valid negative findings. No current report proves a delegation advantage or claims that the main experiment ran.
