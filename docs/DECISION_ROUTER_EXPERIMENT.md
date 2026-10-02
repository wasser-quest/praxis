# Decision-router experiment

The question is whether state routing improves task behavior, compared with
keeping the initial state or letting the solving model select states. A correct
classifier label or a larger number of transitions does not answer that question.

## Protocol

Keep the original twenty natural tasks in `tests/fixtures/20-tasks.json`. Ten
require reading a temporary file before answering. The user prompts never ask
for a state switch; acceptable states and answer rubrics stay outside model
context. Each task runs in four arms, for **80 task runs on 20 distinct tasks**:

| Arm | State controller | Selection policy |
| --- | --- | --- |
| `fixed` | Initial state | No model state writes. |
| `entry` | Solving model | At most one state-write attempt in its first response. |
| `continuous` | Solving model | May reconsider after evidence/tool steps. |
| `decision` | Trusted Decision profile | Router reevaluates before each solving request; no model state writes. |

All arms use the same solving model, `20-tasks.sm`, `20-tasks.poml`, corpus,
tool budget and reasoning setting. The bundled Decision profile has nine labels,
`every_step` reevaluation and a minimum probability of 0.8. Each run gets a fresh
temporary runtime, database, user and fixture file. Runtime tool guards permit
reading that fixture, local discovery/context operations and valid state writes;
they reject raw shell, file writes and external side effects. Arm-specific
instructions and state-dependent tools are part of the tested treatment, so this
does not isolate classifier accuracy from prompt/tool-availability changes.

The runner freezes the test binary, workflows, templates, corpus, effective
Decision profile and Python observer, and records their SHA-256 hashes. Endpoint
overrides affect this snapshot only. The POML CLI entry point is hash checked;
its external Node/POML installation is not copied and must remain unchanged.
Backend readiness and model identities are checked before inference and between
runs. llama.cpp uses `/health` and `/props`; Ollama uses `/api/tags` and
`/api/show`, pinning the selected model digest and hashes of template/model
metadata. System One also checks `/api/version`. These probes do not generate
text, download a model, load a slot or start/wake an instance. Normal Ollama
inference may load an already installed model into memory.

Arm order rotates between tasks to reduce simple ordering bias. Concurrent GPU
load is not controlled. One run per task/arm is descriptive evidence about this
corpus, not a statistical or general reliability claim.

## What counts as evidence

`analyse_run` joins generated tool calls with unique runtime history records by
call ID and tool name. File evidence additionally requires the exact fixture path
and returned fixture bytes. Complete runtime archive metadata is validated and
removed before that comparison. A proposed read, an error response or model prose
claiming success cannot satisfy the evidence requirement.

Decision events are collected from the runtime stream before each observed
solving request, then compared with its actual state. Applied Decision transitions,
successful model state writes, redundant writes and unexplained runtime changes
are separate metrics. Failed, low-probability, unchanged and stale router outcomes
are retained without counting them as applied transitions. Missing, malformed or
lagged Decision traces cannot establish policy compliance. An `every_step`
profile requires one event for every observed solving request.

The report separates:

- Answer-rubric passes, including required file evidence and successful execution.
- Arm-policy compliance, including rejected model state-write attempts.
- Paired rubric gains/losses and paired **compliant** rubric gains/losses.
- Actual states used, switches after evidence and router outcomes/elapsed time.
- End-to-end latency and solving-model token costs. Missing latency/usage is
  unknown, not zero; paired costs report how many pairs have complete measurements.

Router latency is already included in end-to-end latency; do not add it twice.
System One classifier usage, when reported, is retained in route traces and
separate input/output-token totals. Missing classifier usage remains unknown;
paired token deltas still measure the solving model only. Regex rubrics are
proxies: review answers and failure traces before
drawing a product conclusion. More switches or an expected final state alone
never produces a benefit claim.

`decision_conclusion` stays `insufficient_evidence` until all twenty IDs have all
four arms, every run is live and routing traces are complete. The resulting
conclusion reports more/fewer/no net compliant passes against `fixed` on this
corpus. Comparisons against `entry` and `continuous` remain in the same report.

## Prepare and run

From the Rust project root, build the library test executable without running
live tests:

```bash
cargo test --locked --lib --no-run
```

Use the executable path printed by Cargo and the real POML CLI path. The example
endpoints are placeholders for already running, reachable model servers:

```bash
python3 scripts/bench_state_machine.py \
  --test-binary /absolute/path/to/target/debug/deps/praxis-HASH \
  --runtime-root "$PWD" \
  --poml-cli /absolute/path/to/poml-cli \
  --url http://solver-host:11435 --model qwen3.8-27B \
  --decision-url http://decision-host:11440/v1/decision \
  --output /absolute/path/to/state-experiment-v3 \
  --prepare-only
```

This prepares all 80 runs without network requests or inference. Run the same
command with `--resume --allow-paid-live-inference` instead of `--prepare-only`
when the existing servers are reachable. An authenticated solving server may use
`PRAXIS_LIVE_API_KEY` from the environment; do not put credentials in endpoint
URLs. The Decision client uses the trusted profile's existing protocol.

For a separate pilot, add `--cases 04,14,20` and use a different output directory
for twelve runs. A pilot directory cannot be expanded into a full run with
`--resume`, because its frozen plan differs. The full default selects IDs 01–20.

Outputs include `plan.json`, `backends.json`, per-run requests/responses,
`decision-routes.json`, tool history/results, logs, `rows.json`, `summary.json`
and `REPORT.md`. Failed preflight writes `stopped.json` and starts no inference.
Backend/runtime changes stop the next run and preserve completed rows. Resume
skips recorded task/arm pairs only with matching configuration and hashes. A
process failure before fixture creation needs investigation from its log; keep
that directory and use a fresh output for a corrected run.

Store artifacts outside transient `/tmp` when running on the original VPS.
Do not combine measurements from different runtime/model/template snapshots.

## Ollama on one URL

Ollama chat and its native Decision API can share one reverse-proxy URL. Select
the solving model with `--model` and the routing model with `--decision-model`.
The solving model must support tool calls. The Decision arm requires **Ollama
0.35.0 or later** and a System One model such as `nimble`; an ordinary chat model
is not a substitute. Install the models on your server beforehand. For example,
run `ollama pull nimble` there. Check installed tags with:

```bash
curl -fsS http://ollama.the.grid:8080/api/tags
curl -fsS http://ollama.the.grid:8080/api/version
```

Ollama does not need llama.cpp's `/health` or `/props` routes. Nginx must forward
`/api/*` and `/v1/systemone` to Ollama without rewriting away their path prefixes.
The native provider uses `/api/chat` with streaming tool calls and token usage.
Ollama defaults to `--thinking-mode auto`, leaving the selected model's default
in place; use an explicit supported mode to control it consistently in every arm.
llama.cpp retains the previous `low` default.

After building the test executable as above, prepare a twelve-run pilot:

```bash
TEST_BIN=/absolute/path/printed/by/cargo
export POML_CLI=/absolute/path/to/real/poml-cli.cjs
BENCH_ARGS=(
  --test-binary "$TEST_BIN"
  --runtime-root "$PWD"
  --poml-cli "$POML_CLI"
  --provider ollama
  --url http://ollama.the.grid:8080
  --model YOUR_INSTALLED_TOOL_MODEL_TAG
  --decision-model nimble:latest
  --decision-minimum-probability 0.8
  --cases 04,14,20
  --output "$PWD/bench-results/ollama-pilot-v3"
)
python3 scripts/bench_state_machine.py "${BENCH_ARGS[@]}" --prepare-only
python3 scripts/bench_state_machine.py "${BENCH_ARGS[@]}" --resume --allow-paid-live-inference
```

`--provider ollama` defaults the Decision arm to Ollama too. Its endpoint defaults
to your base URL plus `/v1/systemone`, and its model to `nimble:latest`. Override
with `--decision-provider native|ollama`, `--decision-url FULL_ENDPOINT`, or
`--decision-model TAG`; both servers may also have different base URLs.
`--decision-timeout-ms` overrides the System One 60000 ms default (maximum
300000); native Decision keeps its existing timeout limits.

The routing threshold remains **selected-choice probability >= 0.8**, including
equality. `--decision-minimum-probability` overrides the trusted profile value in
the frozen snapshot. Low probability retains the previous state. The API's
separate `confidence` value is recorded as a diagnostic and is not substituted
for the selected choice's probability. Probabilities are not guarantees of
correct classification. Undeclared labels, wrong model/answer types, incomplete
or invalid probability distributions, malformed responses, failures and timeouts
cannot authorize transitions. No chat-model self-reported confidence is used.

For use outside the experiment, `decisions/task-router-ollama.json` is a bundled
profile with `backend: "ollama"`, described `criteria`, `/v1/systemone` and the
same 0.8 threshold. Edit its endpoint/model for your server and select it as the
trusted Decision profile. Profiles without `backend` keep the native protocol.
See [Ollama's Decision guide](https://docs.ollama.com/capabilities/decision) and
[System One API](https://docs.ollama.com/api/systemone) for backend requirements.

Ollama support uses v3 plans. Existing v2 snapshots must remain separate; use a
new output directory. To run all twenty tasks/four arms, remove `--cases` and
choose another new directory. Reports still distinguish live results from local
HTTP fixture tests; compatibility tests do not establish routing benefit.

## Evidence and remaining work, 2026-10-02

The historical work log records **15/20 expected-label matches** in a classifier
test. That measures routing labels, not task improvement. `TODO.md` separately
records a later five-task four-arm checkpoint at
`/tmp/praxis-live-baseline-9uao6aup/state-experiment-4arm/`. Those raw artifacts
are unavailable in the current workspace, so the checkpoint has not been
reverified and must not be reported as a completed quality comparison.

The v2 scorer changes transition attribution and file-evidence accounting.
It intentionally refuses to resume unversioned historical results. Recover the
old artifacts and original snapshot first: finish/evaluate them under that
protocol if comparable, or keep them as historical pilot evidence and run the
complete frozen v3 protocol separately. Do not silently pool versions.

Current offline validation comprises 34 Python scoring/preparation/readiness
regressions and 31 selected Rust tests. Two scripted full-gateway tests use real
POML and file receipts across five scenarios, including router failure and
Ollama probabilities 0.79, 0.80 and 0.95 against a 0.8 threshold. Opposing
confidence values do not change the threshold result. The Python scorer also
processes those actual gateway artifacts. A complete CLI smoke test runs all
four arms for one original corpus task against a local Ollama HTTP fixture.
These validate the measurement machinery, **not live-model benefit**.

The private solver/Decision endpoints could not be reached from this workspace.
No new live corpus result has been produced. Finishing the experiment requires
reachable existing endpoints, or running this frozen protocol on the VPS with
access to the historical artifacts.

## Next architectural steps

After the live comparison, extend the existing named-check receipts and
transactional patches into generic plugin action/effect contracts: typed
preconditions, postconditions, idempotency and explicit rollback/compensation.
Then let `.sm` transitions consume general runtime-verified facts/receipts,
introduce semantic capabilities in place of raw terminal orchestration, and
only then consider a compact Praxis Decision IR. Existing execution guarantees
and their limits are documented in [execution contracts](EXECUTION_CONTRACTS.md).
