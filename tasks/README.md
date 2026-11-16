# Dataset suite v2 (not protocol-frozen)

Generate all inputs and manifests:

```sh
cargo run --locked -- generate-suite /tmp/opencode/verified-cli-suite 2026
```

The destination must not exist. Its parent must exist. Generation never writes reference answers. A failed generation may leave a partial directory; do not treat it as a complete dataset without checking `dataset.json` and all 48 manifests. Do not overwrite an existing dataset to regenerate it.

There are four variants per family, each at size parameters 10, 1,000 and 100,000. Sorting and aggregation sizes count rows. Text sizes count repeated content blocks in the two main target files; nested fixtures have fixed size. Filesystem sizes count repeated content blocks, not files: bulk rename always has five renamed files. Thus artifact size varies without claiming to vary invocation complexity. Each manifest records actual input bytes separately.

| Family | v0 | v1 | v2 | v3 |
| --- | --- | --- | --- | --- |
| Sorting | Signed numeric | Case-sensitive lexical | ASCII-insensitive lexical + descending secondary key | Mixed timestamp normalization |
| Filesystem | Single rename | Nested move | Five-file prefix rename | Collision; no change allowed |
| Text | Exact substring | Case-sensitive Unicode whole-word boundaries | ASCII-insensitive multi-file substring | Recursive restricted-scope whole-word |
| Aggregation | Multi-condition count | Grouped count and sum | Filtered grouped sum and average | Filtered distinct and null counts |

Sorting fixtures include nulls, duplicates, ties, quoted delimiters, embedded quotes and embedded newlines. Timestamp formats and timezone rules are stated in the manifest. Text fixtures include substring traps, punctuation, Unicode, LF and CRLF. Filesystem paths contain spaces, brackets and Greek characters, with unaffected files present. Aggregation inputs include nulls and boundary values.

The small v3 sorting, text and aggregation cases are explicit failure cases: an invalid timestamp, invalid UTF-8 in a scoped file, and an invalid number, respectively. They require atomic rejection: preserve the input tree, create no normal output and write `result.json` with the specified error code. Filesystem v3 at every size requires preservation on collision and a `COLLISION` report. A refusal without the required report is incomplete, not success.

Every task directory contains `input/` and `manifest.json`. `dataset.json` enumerates task IDs. Manifests include generation version, seed, exact requirements, complexity, expected outcome, preservation invariants, input hashes, input bytes and draft artifact/memory/time limits. Directory entries appear in snapshots as well as file hashes. No model budget is implied by these limits.

## Host-only references and independent scoring

```sh
cargo run --locked -- reference-suite-task DATASET/TASK_ID /tmp/opencode/reference-output
cargo run --locked -- score-suite-task DATASET/TASK_ID /tmp/opencode/reference-output
cargo test --locked
```

The reference destination must not exist. Both interfaces are for trusted harness code only; they must never be exposed as model tools. The output tree represents the full post-run input filesystem, including unchanged original inputs and any permitted output. The scorer checks original input hashes before evaluating the complete post-run tree; missing or malformed artifacts return rejection or an error, never success.

Reference sorting uses stable sorting; the oracle instead checks adjacent keys, row provenance and original positions. Text uses a character-scanning reference and a match-index oracle with full byte preservation. Aggregation uses a one-pass accumulator reference and a per-group selection/reduction oracle. Filesystem reference operations perform actual renames; the oracle independently remaps original snapshot hashes. Parsing and explicit semantic primitives (CSV, timestamp parsing, word-character definition) are shared; these are not fully independent implementations of every policy, a threat to validity. Hand-calculated cases test those shared semantics.

The full-suite test generates every instance twice, compares hashes, checks row counts, accepts 48 reference outputs, rejects an extra file for every task and rejects a corrupted task artifact for every task. Additional tests check hand-calculated aggregation, timestamp normalization, Unicode text boundaries, precision tolerance, stable ties, null placement and row preservation. Inputs and reference trees are checked against the 128 MiB artifact limit. The 60-second and 512 MiB limits are dataset defaults; resource enforcement belongs to the next sandbox step, not this generator.

## Limitations before protocol freeze

This is 48 instances of 16 semantic variants, **not 48 independent transformation definitions**. Statistical analysis must account for size and variant relationships. Small v3 failure cases are not matched success-size measurements, so size analyses must distinguish outcome type rather than attributing their behavior solely to size. Runtime limits must be checked again against the selected model/tool environment before protocol freeze.

The older `score`, `score-text`, `score-filesystem` and `score-aggregation` commands still score their original prototypes, not this suite. Use `score-suite-task` for suite v2. Neither reference answers nor oracle feedback may be accessible to experimental models. No sandbox or model runner exists yet: do not begin model experiments on these inputs.
