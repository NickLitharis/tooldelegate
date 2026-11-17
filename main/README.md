# Main experiment records

Frozen protocol: [../docs/protocol.md](../docs/protocol.md). Matrix: 48 tasks × 4 arms × 3 repetitions, randomization seed 42. All 576 cells are present exactly once; no protocol violations were reported.

Execution: 54 cells ran sequentially in `main/`; the remaining 522 ran as four disjoint shards (`main-1/` … `main-4/`) from the same frozen matrix to reduce wall time. No cell was duplicated. Shard membership is not analyzed.

## Results

| Arm | Successes / runs | Success rate | Known tokens | Unknown-usage runs |
|---|---:|---:|---:|---:|
| A direct | 23 / 144 | 16.0% | 5,034,745 | 44 |
| B raw tool | 104 / 144 | 72.2% | 3,052,082 | 14 |
| C raw verifier | 94 / 144 | 65.3% | 2,532,189 | 20 |
| D certificate | 95 / 144 | 66.0% | 1,357,952 | 19 |

Terminations: 456 finished, 97 adapter errors (unknown usage), 15 resource limits, 8 call limits.

Primary D−A: success difference +50 percentage points; one-sided 95% lower bound 0.389 against a 5-point margin, so exploratory success non-inferiority is supported. The token difference is **not estimable** because 58 of 144 paired runs have unknown usage; known totals are lower bounds, not imputations.

D versus B: success −6.25 points; non-inferiority not supported. D versus C: +0.69 points; non-inferiority not supported. C versus B: −6.94 points; not supported. The report therefore does not establish a certificate advantage over raw feedback.

Report conclusions: H1 token reduction not established; H2/H3 not estimable from incomplete usage (size trends and attribution are exploratory only); H4 invocation relationships are observational, with argument volume positively associated with interface errors (correlation 0.228, task-cluster CI excludes zero).

## Report and reproduction

```sh
./scripts/collect-main.sh main/records.jsonl
cargo run --locked -- analyze main/records.jsonl main-report configs/analysis.main.json
```

`main-report/report.md` regenerates byte-identically. The moderation sensitivity cohort and report are `main/records-no-content-rejections.jsonl` and `main-report-sensitivity/`; the sensitivity report excludes 16 provider content rejections and lists their cells as missing. It is not the primary analysis.

The analyzer labels reports exploratory; the frozen protocol was recorded before execution, but the analysis tool has no confirmatory mode. Do not present this as a preregistered confirmatory result.
