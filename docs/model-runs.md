# Model adapter, arms and telemetry

Current selection: GPT-6 Luna through ChatGPT Plus subscription access. Live connector accounting is verified: a probe forwarded decoding `{"max_output_tokens":64,"reasoning_effort":"none"}` and received provider-reported usage `input_tokens=28`, `output_tokens=7`, `cached_tokens=0` with resolved model `gpt-6-luna` equal to the pinned request. The same probe showed that the endpoint rejects `temperature` for this model and applies its own defaults (`temperature=1.0`, `top_p=0.98`); pinned decoding excludes temperature accordingly. Evidence: `reports/luna-connector-probe.json`. An earlier text-only access probe is in `reports/luna-access-check.json`. Pricing is deferred by user request; monetary cost remains null. Subscription quotas and execution limits still apply.

The Rust harness supports provider-neutral model connectors and deterministic replay. A vendor-specific connector exists for GPT-6 Luna (`luna-connector`, `src/luna_connector.rs`). Offline test usage is deliberately synthetic; it must never enter a research analysis as measured usage. Total-token accounting is now validated against a live reply; per-message attribution is still unresolved.

The connector targets the ChatGPT Codex Responses endpoint (`https://chatgpt.com/backend-api/codex/responses`) using the OpenAI OAuth credential held by the local OpenCode credential broker. It calls `opencode api get /api/credential` itself, keeps the access token in memory and in a `0600` header file that is deleted after the call, and never writes it to argv, replies or traces. `LUNA_ACCESS_TOKEN`/`LUNA_ACCOUNT_ID` override the broker for operator use and tests; `LUNA_BASE_URL` exists only so tests can point at a local server.

Accounting rules enforced by the connector:

- Input and output tokens come only from `response.usage` (`input_tokens`, `output_tokens`, `input_tokens_details.cached_tokens`). A reply without provider-reported usage is rejected; totals are never estimated.
- `usage.source` is `api_reported`. `schema_tokens`, `feedback_input_tokens` and `verification_input_tokens` remain null until a validated tokenizer attribution exists.
- The resolved `response.model` must equal the requested pinned model; otherwise the call fails rather than silently substituting a different snapshot.
- `finish_reason` is `stop` only for a completed response; `incomplete` maps by reason, and failed streams are errors. Truncation is never completion.
- Only `temperature`, `top_p`, `max_output_tokens` and `reasoning_effort` decoding fields are forwarded; any other field is a configuration error. The echoed `decoding` is the exact object forwarded. The endpoint does not report its effective decoding, so equality here means "requested and forwarded unchanged", not provider-confirmed.
- Provider-side tools are always `[]`, `store` is false and no conversation state is reused. Assistant turns are replayed as `assistant` items and tool feedback as `user` input items, because the harness action interface is not a provider function-call interface.
- The harness action schema is appended to the provider instructions on every call, so each arm sees its permitted actions without provider tool definitions. The schema uses a flat `allowed_actions` catalog and flat parseable examples; it has no wrapper array or nested `fields`.
- Malformed-action feedback includes the bounded parser error, so a model can correct its own JSON instead of repeating it blindly. This interface feedback is identical across arms and is not oracle feedback.
- A non-stream error body (for example HTTP 429 `usage_limit_reached`) is surfaced with its type and `resets_at`; the run is recorded as an adapter failure with incomplete usage, never as a model result.

Known limits: multi-turn reasoning state is not carried across calls (`store: false`), OpenAI does not accept a seed here, the ChatGPT subscription quota can interrupt runs, and the harness still has no per-message tokenizer attribution. A hard kill mid-call can leave the `0600` credential header file behind inside its `0700` temporary directory; normal completion and error paths delete both. `reports/luna-connector-probe.json` records the first live connector exchange; `reports/smoke-run.json` records the first full task run and the interface defects it exposed.

## Interfaces

`ModelAdapter` accepts a JSON request containing the pinned model, decoding settings, complete message history, permitted action definitions, and remaining resource budgets. It returns a `Reply`. The model's action is a JSON **string**, parsed strictly by the harness; malformed actions and unknown fields are rejected, not corrected. Requests, replies, original provider payloads, call latency and usage are stored in host-only JSONL traces.

Connectors must disable provider-side browsing, execution, persistent conversation state and any other capability outside this action protocol. They must not read reference answers or oracle results. Multiple native tool calls must not be silently reduced to the first call or repaired into one action. The trusted connector must not daemonize or leave helpers outside its process group.

Replay:

```sh
cargo run --locked -- replay-run TASK RUN_DIRECTORY A RUN_CONFIG REPLIES.jsonl
```

Each reply file line is a serialized `Reply` as defined in `src/model_adapter.rs`. Replay runs are always stamped `synthetic: true`, regardless of supplied token values. Test fixtures exercise accounting arithmetic, not the accuracy of any real tokenizer or API. See `tests/orchestrator_cli.rs` for runnable synthetic fixtures.

Trusted provider connector:

```sh
cargo run --locked -- connector-run TASK RUN_DIRECTORY D RUN_CONFIG CONNECTOR_CONFIG
```

`CONNECTOR_CONFIG` is `{"program":"/absolute/path/to/trusted-connector","args":[]}`. The connector is explicitly trusted **host code**, not a command or path selected by a model. It receives one request JSON object on stdin and emits one reply JSON object on stdout. It may use environment credentials; never put secrets in command-line arguments or record them in provider payloads. Its stdout envelope is bounded to 4 MiB. Execution is limited to the smaller of 90 seconds and the remaining task wall budget; its process group is killed on timeout and after completion. Connector stderr is not captured because it may contain credentials; errors must instead be reported in a scrubbed stdout response or exit failure.

Required reply fields:

| Field | Contract |
| --- | --- |
| `model` | Exact pinned identifier; must equal requested identifier |
| `decoding` | Exact effective requested decoding object; mismatches fail the run |
| `action` | Original, unmodified action JSON text |
| `finish_reason` | `stop` for a complete response; any other value is a resource-limit failure, never artifact completion |
| `usage.input_tokens`, `usage.output_tokens` | API-reported nonnegative integers for the whole call |
| `usage.cached_input_tokens` | Reported cached-input subset, or null when unknown |
| `usage.source` | `api_reported` for a real connector; synthetic fixtures use a different label |
| `usage.schema_tokens` | Schema token attribution for this call, or null when not measured |
| `usage.feedback_input_tokens` | Attribution to all feedback included in this call, including repeats, or null |
| `usage.verification_input_tokens` | Subset attributable to verifier feedback in this call, or null |
| `provider_request`, `provider_response` | Full API payloads with credentials removed; never reinserted into model history |

Attribution is **not** inferred from UTF-8 bytes or API totals. A selected connector must implement and document its model-specific tokenizer/attribution method before schema-overhead or verification-tax conclusions can be drawn. Unknown values remain null, not zero. Individual feedback events currently record token counts as null; the exact content is retained for subsequent validated tokenization. Thus the current interfaces support accurate total-usage logging but do **not yet provide provider-calibrated per-message token measurements**. This is a prerequisite to the real pilot, not a measurement supplied by the replay tests.

Use `configs/run.example.json` as a starting point. It contains an unresolved model identifier and no pricing, not a study-ready protocol. Choose identical settings and budgets across arms and preserve the config with the run. Pricing requires recorded currency, source, input, cached-input and output prices per million tokens. Cost remains null if pricing, cache attribution or total usage is incomplete. Cached tokens remain part of total input tokens; they are not subtracted when calculating token consumption.

Set `repetition` to the scheduled repetition index for each run. It is metadata, not a request to change decoding or dataset seeds. Keep the provider mapping stateless between runs and preserve any provider-side cache reporting. A connector may normalize a provider's complete structured-tool response finish reason to `stop`, but must preserve the original payload and must never map truncation, filtering or incomplete generation to completion.

## Treatment arms

All arms receive identical task requirements and original input inventories, plus a common bounded `read` action. Reads use byte offsets and return UTF-8 text, or base64 bytes when the range is not valid UTF-8; a read is capped at `(feedback_bytes-96)*3/4` bytes so the encoded response fits the feedback budget. Input is not silently injected into later calls. Models may page through a file with offsets. The immutable original snapshot is available through read; hidden manifest fields, reference answers and oracle outcomes are not.

- **A:** `put` writes model-generated UTF-8 chunks, with explicit truncate/append behavior; `mkdir` and `remove` supply the minimal filesystem sink primitives. No shell, command execution or verifier feedback is available. Filesystem moves require generating destination bytes and removing the source, not asking the sink to transform content. Full chunks are saved in response traces and counted in model output usage. Only chunk metadata, not content, is repeated in subsequent history. `submit` acknowledges delivery without checking correctness.
- **B:** `submit` supplies an ordered batch of permitted command argument arrays; it receives bounded raw status/stdout/stderr. No feedback verifier is run.
- **C:** the same command interface returns bounded independent verifier output after the complete batch.
- **D:** the same command interface returns only the bounded verification certificate after the batch. Command output is never returned.

Common read results, sink acknowledgments, malformed-action feedback and interface rejection messages are recorded separately from verification. Those ordinary interface errors are not oracle feedback. For B, raw bytes are decoded with UTF-8 replacement and bounded again at a character boundary; the original bounded bytes and exact raw byte counts remain in tool logs.

Each complete submitted batch/artifact is an attempt. Additional submissions count as retries, with the same limit across arms. Chunk writes and input reads do not each consume a retry; multi-command transformations can submit one complete batch without intermediate verifier failures exhausting retries. There is no automatic oracle-based repair. Invalid JSON consumes a model call, not a submission; invalid command execution consumes its submitted attempt. Both are bounded by the same call and token limits. `finish` requires a prior submission and, in A, no artifact changes after that submission. Hitting any resource or call budget is recorded as incomplete, even if a partial final tree happens to pass the oracle.

The hidden oracle scores the final tree for every arm in a separate resource-limited host worker. It never influences another model call. Both raw `oracle_pass` and completion-aware `success` are recorded. Oracle failure to execute is distinct from a conclusive rejection. Every run directory must be fresh; existing results are never overwritten.

## Records

- `trace.jsonl`: complete abstract requests, provider payloads, action failures, exact returned feedback, verification records, latencies and final summary.
- `tool-N.json`: command status, resource flags, bounded raw captures, full byte counts and snapshots.
- `summary.json` and one-line `record.jsonl`: run-level settings, manifest, outcome, usage, costs, final hashes and timings.
- `work/`: final artifacts, outside model context.

An environment record includes compiler, kernel and CLI versions, executable hash and Cargo lock hash. Input and final hashes are retained. Known usage from every returned response is counted, including malformed actions, configuration mismatches and limit failures. If a connector fails before supplying valid usage, `usage_complete` is false; recorded totals are **known lower bounds**, never complete measurements, and cost is null. Missing/censored runs must not be discarded later.

Verification elapsed time, CPU, raw tool bytes, returned feedback bytes and context peak (largest reported input-plus-output call) are separate metrics. Retries after negative verifier feedback are recorded as an observed sequence, not a causal claim. Final verifier disagreement with the oracle is recorded when both judgments exist. These are run-level observations, not statistical results.

## Randomized main-run plan (no execution)

```sh
cargo run --locked -- plan-runs DATASET NEW_PLAN.json 42 3
```

This creates the paired 48 × 4 × 3 = 576 matrix and shuffles it reproducibly using a recorded, dataset-independent randomization seed. It neither authorizes spending nor executes runs. The main protocol still needs pilot review and freeze. Automated tests check the complete matrix, uniqueness, deterministic ordering and seed sensitivity.

## Gates still open

Document tokenizer attribution, record usage limits, and freeze the protocol after pilot review. The live connector probe verified total-token accounting and model pinning; `temperature` is rejected by the endpoint. Pricing remains deferred. No full benchmark or scientific comparison has been run. The [analysis and report commands](analysis.md) work with saved records and explicitly labelled synthetic fixtures.
