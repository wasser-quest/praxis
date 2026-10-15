# Completion update — Codex integration and bug audit

**The original handoff below is historical and superseded by this update.**
The remaining memory-discovery failure is resolved and final offline verification
is complete. Existing working-tree changes were preserved; nothing was committed.

- The save receipt rejected missing `expected_profile`; the routed persona was
  `states/standard/standard`, not legacy `standard`. The integration test now
  discovers, creates/loads a named profile, reads before writing, checks receipts
  and reopened-DB persistence, and verifies user/profile/task isolation. Runtime
  memory guards were not weakened.
- Registry follow-up fixes preserve discovery schemas without creating an
  accidental allow-list and keep an empty native DB catalog fail-closed.
  Empty/cyclic groups, native disables and plugin isolation remain covered.
- Added Codex regressions for malformed/duplicate terminal tool calls, both
  streaming entry points (reasoning/refusal/tool deltas), and rejected refreshes.
- Final results: **661 library tests passed, 29 ignored; 13 binary tests passed;
  all 16 explicitly enabled offline tool-loop tests passed**. POML 0.0.8 was real,
  with `PRAXIS_REQUIRE_POML=1`; no render skips. `cargo build -j 1`, JS syntax/UI/chat/
  memory checks, and `git diff --check` also passed. Existing compiler warnings remain.
- Updated `bugs.md`, `README.md`, `docs/CONTEXT_VARIABLES.md` and
  `docs/MEMORY_PROFILES.md`. Reproduction commands and limitations are in
  `bugs.md#verification`.
- **No real credentials were read, tokens rotated, live Codex requests/device
  logins made, or deployments started.** Live subscription compatibility remains
  unverified; the opt-in smoke test was not run.
- Logs: `/tmp/praxis-final-tests.log`, `/tmp/praxis-final-tool-loops.log`,
  `/tmp/praxis-final-build.log`, `/tmp/praxis-final-js.log` (each has a `.exit`
  counterpart containing `0`). Temporary POML CLI:
  `/tmp/praxis-poml/lib/python3.14/site-packages/poml/js/cli.js`.

---

# Original handoff — resolved historical context

You are continuing work in `/home/user/praxis`, a Rust application (Praxis 0.11.0). The original user request was: **make Codex work in Praxis and fix all bugs listed in `bugs.md`**. The user paused to request this handoff for a fresh/larger context. Continue the implementation and verification below; do not start over.

## Working-tree and safety rules

- The working tree contains substantial unfinished changes for this task, including new Rust files. **No commits have been made. Do not reset or discard the changes.** Review `git status --short` and `git diff`.
- Read `bugs.md`, the changed README login section, and relevant source. `docs/AGENTS.md` requests reading `docs/BLUEPRINT.md`; it was read completely in the previous session. The blueprint is old; the actual source and current tests are more up to date.
- No live Codex request was made. Do not access real credentials, rotate tokens, or start a deployment without authorization. Offline test success is not proof of live subscription compatibility.
- Do not run all ignored tests indiscriminately: some require live/paid services. The specifically named tool-loop tests below are offline, using synthetic providers and temporary databases/files.
- `bugs.md` has already been rewritten to describe the implemented fixes, **but final verification is NOT complete. Do not claim everything is fixed until the remaining failure is resolved.** Update the document if any defect remains.

## Immediate next step: fix the one remaining integration failure

The last command sequence completed while this handoff was being written:

```bash
POML_CLI=/tmp/praxis-poml/lib/python3.14/site-packages/poml/js/cli.js \
PRAXIS_REQUIRE_POML=1 cargo test -j 1 > /tmp/praxis-complete-tests.log 2>&1
# followed by:
POML_CLI=/tmp/praxis-poml/lib/python3.14/site-packages/poml/js/cli.js \
cargo test --lib -j 1 gateway::message_handler::message_tool_loop_tests -- --ignored \
  > /tmp/praxis-complete-tool-loops.log 2>&1
```

Each command writes its exit status to the corresponding `.exit` file.

- The regular suite completed: **655 library tests passed, 29 ignored; 13 binary tests passed**. Real POML rendering was enabled, not skipped.
- One subsequent small registry change (empty/cyclic group allow-lists must remain closed) added another unit test. The regular suite needs to be rerun against that final change.
- The **16 explicitly enabled offline tool-loop integration tests completed: 15 passed, 1 failed**, in 231 seconds. No need to wait for the old test process.
- **The remaining failure is:**
  `gateway::message_handler::message_tool_loop_tests::tool_chain_discovery_loads_memory_only_for_current_task_on_both_paths`.
- The latest log reports `no entry found for key` at `src/gateway/message_tool_loop_tests.rs:694:94`, reading `db::memory::load_memory(...).custom_variables["xp"]`. The fixture first intentionally calls undiscovered `memory_set`, then `search_tools {query:"memory"}`, then `memory_set {key:"xp",value:42,expected_value:null}`, then `memory_get`. Its final mock reply succeeds, but the memory assertion fails.
- **Inspect the actual tool receipts/errors, offered schemas, and active-profile persistence contract; do not assume the cause or simply weaken the assertion.** Relevant files: `src/gateway/message_tool_loop_tests.rs`, `src/tools/discovery.rs`, `src/tools/memory.rs`, `src/db/memory_profiles.rs`, and `src/tools/registry.rs`.
- All other tests in that group now pass, including compaction, image forwarding, stale-completion reset, tool-output controls across all three loops, retries without reexecution, budgets, skills, cancellation, and 20-tasks state rerendering.

Useful focused rerun:

```bash
export POML_CLI=/tmp/praxis-poml/lib/python3.14/site-packages/poml/js/cli.js
export PRAXIS_REQUIRE_POML=1
cargo test --lib -j 1 tool_chain_discovery_loads_memory_only_for_current_task_on_both_paths \
  -- --ignored --nocapture
```

## What has been implemented

### 1. Codex transport, refresh, and recovery

Files: `src/gateway/llm/codex.rs`, new `codex_stream.rs` and `codex_tests.rs`, plus `http.rs`, `mod.rs`, `provider.rs`, `routing.rs`.

- Replaced the handwritten lossy SSE parser with the existing bounded byte-oriented `crate::sse::Decoder`: CRLF and split UTF-8 work.
- Require a terminal successful response before returning executable tool calls. Truncated, malformed, incomplete, and failed streams produce typed errors. Incomplete/invalid tool calls are never executed.
- Forward text, reasoning, refusal, and display-only tool-call deltas. Support both streaming trait entry points.
- Do not echo provider error messages (which can contain secrets/prompts).
- Handle Codex exhausted primary/secondary rate-window reset headers, HTTP Retry-After, and `error.resets_at` through the shared router's cooldown metadata.
- Added `LLMProvider::supports_output_limit()` (default true; Codex false). The subscription endpoint ignores/rejects configurable output limits, so the router must not retry identical incomplete requests by merely increasing `max_tokens`.
- Serialize OAuth refresh, **including overlapping hot-reloaded provider instances**, using a weak shared auth-session cache keyed by endpoint and matching auth identity. Two concurrent 401s consume a rotating refresh token only once.
- Refresh callbacks update live credentials atomically and cannot undo logout or overwrite a different account. A persistence failure does not discard already-rotated live credentials.
- OAuth 400/401/403 refresh failures are authentication failures with a fresh-login hint.
- Validate CLI/normalized auth JSON, derive account IDs from access/id JWTs, and handle system/developer messages correctly.
- Thinking off maps to Codex's minimum low; original Codex models cap xhigh at high. Unsupported temperature/output-limit fields are not sent.
- New ignored `codex_live_smoke` test requires an explicitly supplied `PRAXIS_CODEX_AUTH_FILE`. It clears the refresh token before use, so it cannot rotate/persist the supplied credentials. **Not executed.**

### 2. Login lifecycle and endpoint validation

Files: `src/gateway/providers.rs`, new `src/gateway/codex_login.rs`, new `provider_login_tests.rs`, `src/tui/app.rs`.

- Device login continuously drains stdout/stderr into bounded latest-output storage. No heuristic English wording, URL/code line count, or early cutoff. Polling `/login codex` shows late codes.
- Report failures; cancel/timeout kills and reaps the process. On Unix, use a dedicated process group so the npm launcher and Rust child are both terminated (`libc` dependency).
- Background completion captures the actual initiating gateway state, retains the requested model, and cannot commit after logout/cancellation.
- Reuse stored refreshed credentials instead of silently replacing them with a stale CLI auth file.
- New `/login codex --device-auth [model]` explicitly bypasses stored/CLI credentials for reauthentication. `--auth-json` still imports credentials. Conflicting flags are rejected.
- TUI argument parsing consumes JSON as the flag's value, including leading whitespace. It no longer mistakes formatted JSON for a model.
- Setup hints select provider **and model together**, avoiding a leftover local-model name.
- Provider commits merge only that provider's fields into the latest live store, preserving other logins/rotations.
- Endpoint probes require expected model-list JSON. Only llama.cpp falls back from a models 404 to a verified `/health` response. It uses the llama.cpp key, not Ollama's key. Wrong URLs returning 404/HTML/invalid JSON are rejected.

### 3. Secret-store policy and dashboard import

Files: `src/db/secrets.rs`, `src/main.rs`, `src/dashboard/routes.rs`, `static/app.js`, `Cargo.toml`/`Cargo.lock`.

- **Master-key retention is now opt-in:** `PRAXIS_RETAIN_MASTER_KEY=1` or `true`. Default is off. Retained keys use `zeroize::Zeroizing<String>`, persistence avoids password copies, and the original startup password is zeroized after optional retention.
- Added atomic secret mutation helpers, including a separate runtime-refresh helper whose in-memory rotation survives disk-write failure.
- Fixed UTF-8-unsafe secret masking.
- Dashboard `codex_auth` is a dedicated write-only field: accept validated CLI or normalized JSON; GET returns only `***`; masked round trips preserve credentials; empty values remove them; malformed values do not replace the login.
- Dashboard UI lists/imports Codex and does not style failed HTTP updates as success. Existing master-password persistence and router hot reload apply.

### 4. State catalog, prompts, downloads, feedback

- `contexts/standard.sm`: `sm_data.persona_roles` is an every-routing-pass override, not a missing-only `_default`, fixing stale saved catalogs.
- Rewrote large `20-tasks`, tutor/teach, quiz, and transcript-check templates. Shared compact tutor instructions retain consent, profile isolation, optimistic memory updates, SRS grading, and idempotent XP rules. Teach now selects actual due cards rather than always claiming an empty deck.
- Real-render regression tests cover all shipped roots, state template references, per-template budgets, fixed/entry/continuous routing, and due-card language/date filtering.
- Dedicated real-render tests enforce compact prompt budgets. Character-count bounds are **not a guarantee that every model's full prompt, schemas, history, and output fit its token context**.
- `settings.download` now gates Discord attachment downloads, fails closed on unreadable context, and channel authorization happens before downloading/injecting messages.
- `feedback_enabled` now gates external agent feedback. New `src/gateway/feedback.rs` provides a per-user/session sliding-window limiter using validated `feedback_max_per_5min` and `feedback_window_secs`. Web progress and final replies are not suppressed. Feedback tools cannot silently enable external delivery.
- Download/feedback enable flags are user-only through context merges. Removed unused `feedback_template`; `tool_result_limit` is legacy load-only and no longer serialized. `_output`/`read_tool_result` remain its replacement.

### 5. Additional real integration bugs found while validating

These were exposed by running previously ignored offline POML tool-loop tests:

- **Discovery tools were absent in narrow states.** Both shipped workflows' common `agent_basic` group now includes `search_tools`, `search_skills`, `use_skill`, and `read_tool_result`. Keep shell execution scoped to coding states; do not add unrestricted terminal access merely to satisfy tests. Two shell tests now explicitly select `sm_data.role=code`.
- **Compacted history disappeared from minimal prompts.** `src/gateway/prompt.rs::render_system` now appends the saved summary as historical data unless the template already rendered it. The success/failure compaction integration test now passes.
- **Discovery and routed tool schemas diverged.** The DB's built-in definitions are the canonical contracts. The static registry formerly required nonexistent `profile` on memory tools and `image_path` instead of `path`, contained dangling `_output` references, omitted built-ins such as background tools, and ignored native disable flags.
  - `src/db/tools.rs::get_default_tools` is now `pub(crate)`.
  - Registry metadata uses canonical default parameters; runtime routing overlays actual enabled DB definitions, includes activated native tools lacking category metadata, and expands full `_output` schemas.
  - Plugins cannot shadow disabled built-ins. Unknown/unavailable activation names produce bounded diagnostics.
  - A new contract-parity/disabled-tool test passes in the regular suite. **The memory discovery end-to-end test still fails, so this area needs one more investigation.**
- Most recent change: nonempty activation lists resolving to empty/cyclic groups must remain a **closed empty allow-list**, not accidentally enable all tools. `has_tool_allowlist` and its new regression test were added in `src/tools/registry.rs`; rerun the regular suite for this exact final source state.

## Verification and environment

- Initial untouched baseline: 632 library tests passed, 28 ignored.
- Latest completed regular run: 655 library + 13 binary tests passed; one later added unit test needs inclusion in the final rerun.
- `node --check static/app.js`, `node scripts/test_ui_static.js`, `node tests/test_chat_commands.js`, and `git diff --check` passed.
- POML was installed **only in a temporary virtualenv**, not the repository:
  `/tmp/praxis-poml` (`poml==0.0.8`). CLI path is in the commands above; Node is available.
- Prefer **`-j 1`**. This VM has ~4 GiB RAM and ~2 GiB swap. `-j 2` works but causes heavy swapping when compiling lib/test copies.
- Disk filled from repeated debug/incremental builds. Obsolete incremental caches created during this task were removed; the user's original `target/release` was preserved. At handoff ~3.1 GiB disk remained. Check `df -h .` before more builds. Do not indiscriminately delete release artifacts or user caches.
- Existing unrelated compiler warnings remain. No successful normal `cargo build` executable has yet been explicitly verified; test binaries compiled successfully.

## Finish checklist

1. Inspect `/tmp/praxis-complete-tool-loops.log` and fix the remaining memory-discovery failure. The run is finished (15/16 passed). Use tool receipts to diagnose, not blanket test relaxations.
2. Review the latest canonical-registry/empty-group changes for permission and schema regressions. Preserve state allow-lists, native disable flags, task-local discovery, and plugin isolation.
3. Rerun the full default-feature suite with real POML enabled, the 16 offline tool-loop tests, static JS checks, and `git diff --check`. Build normally with `cargo build -j 1` if resources permit.
4. Reconcile `bugs.md`/README with final results. Explicitly distinguish offline verification from live Codex verification; never claim a live subscription call passed.
5. Check the complete diff and untracked new source files. Do not commit unless requested. Give the user a concise summary and these login instructions:

   ```text
   /login codex
   /context set settings.provider=codex settings.model=gpt-5-codex
   /thinking medium
   ```

   For a rejected/stale login: `/login codex --device-auth`. For persistence, explicitly opt into retained master-key storage or use dashboard Secrets with the master password.

Stay focused on completing and validating this work; avoid another broad refactor or unrelated service changes.
