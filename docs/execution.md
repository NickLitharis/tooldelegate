# Execution and feedback contract

## Supported boundary

The implementation requires **Linux x86_64**, bubblewrap at `/usr/bin/bwrap`, `prlimit` at `/usr/bin/prlimit`, user namespaces, seccomp, and the `close_range` syscall. Missing support fails execution; there is no unsandboxed fallback. `cargo test --locked` runs real isolation tests, not mocked or silently skipped checks.

Only `/usr/bin`, `/usr/lib`, `/lib` and `/lib64` are mounted read-only as the system runtime. The fresh task copy is writable at `/work`. `/tmp` is a fresh 16 MiB tmpfs; `/proc` and `/dev` are namespace-local. Host home directories, task specifications, reference code, original input snapshots and logs are not mounted. Environment variables are cleared. Network, PID, user and other supported namespaces are unshared; nested user namespaces are disabled and capabilities dropped. An inherited seccomp filter denies fork, vfork, clone and clone3 and kills alternate syscall ABIs. All nonstandard file descriptors except the filter descriptor are marked close-on-exec before launching bubblewrap.

Allowed top-level commands are `python3`, `sort`, `mv`, `mkdir`, `cat`, and `sed`; names must match exactly. Arguments are passed directly, with no host shell. Python is an intentional arbitrary-code capability **inside** this boundary, not a parser for safe Python. The allowlist controls entry points, not code semantics: a program can exec another runtime binary, but cannot create another process or thread. Tools that require children or threads may fail; for GNU sort use `--parallel=1`. Record such failures rather than relaxing the boundary selectively between arms.

This is OS-level isolation, not protection against kernel vulnerabilities. Do not run as host root or mount credentials into this runtime. Keep the runtime and kernel patched. Tests establish observed behavior on the current machine, not universal escape-proofness.

## Resource policy

Default tool limits, inherited as soft **and hard** rlimits after namespace setup:

| Resource | Limit |
| --- | --- |
| Wall time | 60 seconds, parent watchdog |
| CPU | 30 seconds |
| Address space | 512 MiB |
| Individual file size | 128 MiB |
| Open file descriptors | 64 |
| RLIMIT_NPROC | 128, additionally restricted to one tool process by seccomp |
| Core dumps | Disabled |
| Workspace | 128 MiB logical bytes, 4,096 entries, nesting depth 64 |

Workspace budgets are sampled while running, not filesystem quotas: transient overshoot is possible. All final trees are checked before hashing. This protects post-run parsing from oversized artifacts but is not a strict disk reservation. The watchdog uses directory-descriptor-relative, no-follow lookups to avoid following symlinks introduced during execution. Links and special files are disallowed. Timeouts and budget violations kill the bubblewrap monitor and tear down its namespace. A failed post-run manifest is explicitly recorded as unavailable, never an empty successful tree.

The single-process restriction avoids multiplying memory/CPU budgets through children without requiring delegated cgroups (which are unavailable in this environment). No claims about supporting multi-process tools should be made. The in-process Rust API accepts limits; the diagnostic CLI uses the fixed defaults. Per-task budgets are not yet wired to a model runner.

## Local diagnostic CLI

```sh
cargo run --locked -- generate-suite /tmp/opencode/suite 2026
cargo run --locked -- execute-sandbox \
  /tmp/opencode/suite/sorting-v0-s0 \
  /tmp/opencode/work /tmp/opencode/logs D \
  python3 -I -c 'print("This command does not complete the task")'
```

The work and log directories must not already exist. Every invocation copies the original snapshot; it never reuses or deletes a previous run. Logs must be outside the work directory. Modes `B`, `C`, and `D` select feedback rendering, **not complete model treatment arms**:

- B: bounded raw command stdout/stderr and status; no feedback verification.
- C: bounded raw feedback-verifier report.
- D: only a compact JSON certificate, never successful command output.

Raw stdout and stderr are each retained up to 4,096 bytes while independent counters drain and count every produced byte. Returned B/C feedback is capped at 4,096 bytes total. Excess raw data is deliberately not stored; truncation flags and exact byte counts are preserved. `feedback.bin` preserves the exact returned bytes, which may end mid-UTF-8 sequence for raw feedback; a future model adapter must define and log its decoding policy and tokenization. Token counts are **not** inferred from bytes.

Host-only logs include invocation arguments and limits, pre/post snapshot hashes, exit status, timeout/resource flags, bounded stream captures and byte counts, verifier details and telemetry, and exact returned feedback. Rejected entry points are logged. This is component-level JSON logging, not the future experiment-wide JSONL telemetry.

The CLI's own successful exit means that execution and feedback delivery finished, **not that the task succeeded**. Inspect the certificate or final oracle score. Exit status zero alone is never correctness evidence. Integration tests deliberately use an exit-zero, incorrect transformation and require a negative certificate and oracle rejection.

## Independent verification

The feedback verifier never calls the hidden oracle or reference transformation. Sorting uses stable reference ordering and CSV field equality; the hidden oracle instead checks adjacent ordering and provenance. Aggregation uses an in-memory SQLite computation, independent of the Rust hidden oracle's reductions. Text uses Unicode character-vector scanning and hash comparison rather than the oracle's substring-index reconstruction. Filesystem verification remaps original hashes and checks the whole tree. Failure cases require unchanged trees plus the exact semantic error report. CSV parsing, timestamp parsing, hashing and explicit policy definitions are shared primitives, so correlated failures remain possible.

The diagnostic CLI runs the trusted verifier in a dedicated process, inaccessible to the tool sandbox. It has a 512 MiB address-space limit, 30-second CPU limit, 60-second wall watchdog, bounded output, and no model capability. Worker failure returns `VERIFIER_ERROR`, not approval. Logs record verifier-only thread CPU, elapsed time including worker launch, inner verification time, and worker process peak RSS (including startup). Unavailable failure metrics are null. In-process unit tests report the test process RSS instead; do not use those readings as experimental verifier memory measurements.

`PASS`, `ORDER_ERROR`, `PRESERVATION_ERROR`, `CONTENT_ERROR`, `FORMAT_ERROR`, `SAFETY_ERROR`, and `VERIFIER_ERROR` form the stable feedback taxonomy. Certificates contain only `ok`, `code`, and an optional numeric row index, never free text or artifacts. Their maximum size is 96 bytes. An insufficient return cap is rejected instead of truncating JSON. Full verifier diagnostics stay in host logs.

The hidden oracle remains a separate host-only scoring operation, invoked after execution, never feedback to a model. Automated dataset tests compare both mechanisms on 48 positive and 96 negative fixtures. This is a finite corruption test set, not an estimated real-world false-positive/false-negative rate. Model token accounting, equal retry handling, provider integration and study-wide failure analysis are subsequent work.
