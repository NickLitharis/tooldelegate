# V3 token-usage missingness EDA

Exploratory descriptive diagnostic; not a preregistered hypothesis test. Model recorded in source: `gpt-6-luna`.

Records: **576**  
Usage incomplete: **26 (4.51%)**  
Known partial token subtotal within incomplete records: **38734**  
Incomplete records with a zero known subtotal: **9**

A zero subtotal does not prove zero token usage; it means the record contains no known usage total for that run. `metrics.total_tokens` on incomplete records is only a known subtotal, not the complete run total.

## Missing usage by arm

| Arm | Records | Usage incomplete | Rate | Known partial token subtotal | Incomplete with nonzero subtotal |
|---|---:|---:|---:|---:|---:|
| A | 144 | 8 | 5.56% | 13010 | 8 |
| B | 144 | 7 | 4.86% | 8375 | 2 |
| C | 144 | 5 | 3.47% | 5674 | 3 |
| D | 144 | 6 | 4.17% | 11675 | 4 |

## Incomplete usage by classified failure cause

| Cause | Runs | Runs with nonzero known subtotal | Known partial token subtotal |
|---|---:|---:|---:|
| `adapter_transport_failure` | 5 | 5 | 15706 |
| `provider_content_rejection` | 21 | 12 | 23028 |

### Failure cause by arm

| Arm | Provider content rejection | Adapter transport failure | Other/unclassified |
|---|---:|---:|---:|
| A | 8 | 0 | 0 |
| B | 7 | 0 | 0 |
| C | 4 | 1 | 0 |
| D | 2 | 4 | 0 |

Cause labels come from run-record `failure_events`; unrecognized or absent categories are grouped as `other_or_unclassified`.

## Impact on paired token comparisons

| Contrast | Observed matched pairs | Pairs with incomplete usage | Pairs with exact token deltas |
|---|---:|---:|---:|
| D vs A | 144 | 14 | 130 |
| B vs A | 144 | 15 | 129 |
| C vs B | 144 | 11 | 133 |
| D vs C | 144 | 11 | 133 |
| D vs B | 144 | 13 | 131 |

A pair is matched by task ID and repetition; it is incomplete when either arm lacks complete usage. Only observed pairs with both arms present are counted. These counts describe which contrasts lack exact token deltas; they do not estimate missing values.

## Where incomplete usage occurred

### Task family

| Group | Records | Usage incomplete | Rate |
|---|---:|---:|---:|
| aggregation | 144 | 2 | 1.39% |
| filesystem | 144 | 10 | 6.94% |
| sorting | 144 | 10 | 6.94% |
| text | 144 | 4 | 2.78% |

### Generator size parameter

| Group | Records | Usage incomplete | Rate |
|---|---:|---:|---:|
| 10 | 192 | 9 | 4.69% |
| 200 | 192 | 9 | 4.69% |
| 500 | 192 | 8 | 4.17% |

Size parameters are family-specific generator settings, not a common physical artifact-size scale.

### Expected outcome

| Group | Records | Usage incomplete | Rate |
|---|---:|---:|---:|
| INVALID_NUMBER | 12 | 0 | 0.00% |
| INVALID_TIMESTAMP | 12 | 2 | 16.67% |
| INVALID_UTF8 | 12 | 1 | 8.33% |
| collision_no_change | 36 | 1 | 2.78% |
| success | 504 | 22 | 4.37% |

### Complexity

| Group | Records | Usage incomplete | Rate |
|---|---:|---:|---:|
| multi-step | 324 | 13 | 4.01% |
| simple | 252 | 13 | 5.16% |

Incomplete records span **18 task IDs**; the maximum observed incomplete repetitions for one task ID is **3**. These are repeated task-level observations, not independent task samples.

## Interpretation limits

- No failed-call token counts are estimated or imputed.
- Partial subtotals do not replace missing run totals and cannot make an affected paired treatment comparison exact.
- The main report remains authoritative for token comparisons; this diagnostic does not modify it. Known partial usage already contributes to the main report’s lower-bound token totals and must not be added again.
- Rates are descriptive. The small outcome strata and repeated runs per task do not establish a missingness mechanism or causal arm effect.
- Regenerate from the JSONL run records with `cargo run --locked -- usage-missingness RECORDS.jsonl OUTPUT.md`; the output path must not already exist.
