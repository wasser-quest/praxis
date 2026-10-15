# Bug audit — tools, tool groups, state machine, templates

Result of the review that accompanied the `activated_tools` / `[tool_groups]` /
SM-routing work. Status: **fixed** = implemented (covered by tests
unless noted); **open** = confirmed but not changed; **watch** = design
caveat, not a defect on its own. Entries include the earlier routing fixes and
this Codex/audit follow-up. See [Verification](#verification) for the final
checks; offline coverage does not establish live Codex subscription compatibility.

## Fixed

1. **Plugin tools leaked past the state allow-list** (`src/tools/registry.rs`).
   In `DescriptionOnly` mode every enabled plugin tool (e.g. `brave_web_search`)
   was appended with an empty schema even when the state listed tools
   explicitly. Plugins now follow the same allow-list as built-ins.
2. **All built-in tools dropped when a state used `tool_groups` without
   `full_tool_categories`** (`registry.rs`). `allowed_cats` became `Some([])`,
   which filtered out every built-in — states got only the leaked plugin tools.
3. **`[tool_groups]` section unreachable in `.sm` files** (`src/sm.rs`). The
   parser had a branch for it but rejected the header as "Unknown section", and
   `apply_tool_groups` *activated* every group instead of defining them; the
   recursive resolver had no cycle guard. Now: definitions merge into
   `settings.tool_group_definitions`; states opt in via `settings.activated_tools`.
4. **Hardcoded tool groups removed**. No built-in groups; the shipped
   `contexts/standard.sm` defines its own. `activated_tools` mixes group names
   and single tools (`["coding", "execute_terminal", "brave_web_search"]`).
   `tool_groups` / `full_tool_schemas` remain as legacy aliases.
5. **Model-driven state switching never happened** (`contexts/standard.sm`).
   Roles were wired with `[transitions]`, which only `agent_next` evaluates.
   `set_context("sm_data.role", …)` therefore changed nothing. Replaced by
   `[auto]` rules, evaluated on every routing pass.
6. **Routing froze after the first state switch**
   (`src/gateway/workflow_actions.rs`). `plan()` skipped routing whenever
   `system_template` was not `standard`, so the SM's own
   `states/code/code` selection disabled all further routing. Now only a
   template that no state of the active workflow produces counts as a manual
   choice.
7. **`transition_to` stored state values as raw strings** (`sm.rs`). An array
   like `settings.activated_tools = [...]` became `"[\"…\"]"`, and the
   context failed to deserialize on `agent_next`. Uses the same JSON typing as
   `apply_to_context`.
8. **Every `templates/states/*/…poml` failed to render.** POML resolves nested
   includes relative to the *including file*; `shared/state_base.poml` used
   `../../shared/…`, which pointed outside `templates/`. Only visible with a
   real POML CLI, so earlier tests missed it. Now covered by the real-render
   regression in #23, verified with POML 0.0.8.
9. **`states/teach/teach.poml` had a raw `&&` in text content** — XML parse
   error, template never rendered. Moved into a `<let>`.
10. **Per-state blueprints ignored.** State templates set `blueprint`, but
    `shared/blueprint.poml` only read `persona_blueprint`; every state got
    `blueprints/standard.json`. Both are honored now.
11. **Review persona loaded the language-instructor blueprint**
    (`templates/personas/review.poml`). Points at `states/review/review.json`.
12. **State templates had no way back.** Once in `code` etc. the system prompt
    said nothing about `set_context("sm_data.role", …)`; added
    `shared/roles.poml` and a `role_switch` group (`set_context`, `get_context`)
    activated in every state.
13. **`search_tools` discoveries were never callable.**
    `task_control::selected_tools` fed only the POML `tools` variable, not the
    request's tool list, so a discovered tool was rejected as "Unknown tool".
    `build_tool_definitions_for_user` now merges the task-local selection into
    the state's allow-list.
14. **Duplicate registry entries** — `understand_image` and `update_template`
    were registered twice, producing duplicate function names in requests.
15. **Assets resolved against the shell's cwd.** ~15 call sites use
    `Path::new("templates")`, `"contexts"`, `"./plugins"`. `pin_install_root()`
    (`src/main.rs`, `config::resolve_install_root`) now chdirs to `ROOT_DIR`,
    else the cwd if it holds `templates/`, else the executable's directory — so
    `~/praxis/praxis` uses `~/praxis/templates` wherever it is started from.
16. **System prompt too large for small contexts.** `states/standard/standard`
    rendered ≈970 tokens, the root `standard.poml` ≈2300 (full blueprint JSON,
    duplicated user prompt, duplicated tool list, 7 KB memory boilerplate).
    Standard/root prompts now have a 2800-character regression cap. The larger
    teach/task prompts were also reduced in #24. These are synthetic-context
    character budgets, not model-token or complete-request guarantees.

17. **`tool_discovery_mode = "None"` now sends only activated tools**
    (`filter_tools_for_request`); previously every allowed built-in was sent
    with an empty schema.
18. **`20-tasks.sm` offered ~45 tools with empty schemas.** It now defines
    `[tool_groups]` and per-state `activated_tools`.
19. **Sticky state variables.** `apply_to_context`/`transition_to` now reset
    every variable another state declares but the resolved state does not
    (falls back to a `_default` value, else removed). Keys are compared
    canonically (`cl_data.*` ≡ `sm_data.*`).
20. **Decision router vs `[auto]` rules.** While a decision profile is active
    (`decision_profile` set, `use_decision_router` true) auto rules are
    skipped, so the router's chosen state is not overridden during routing.
21. **`_default` no longer selectable** as `active_state` (SM fallback and
    `route_context_once` validation).
23. **Gated render test added** (`src/gateway/template_render_tests.rs`): with
    `POML_CLI` set it renders all 25 shipped templates through the real CLI,
    checks every SM state's template rendered, and caps the standard prompt at
    2800 chars. `PRAXIS_REQUIRE_POML=1` turns a missing CLI into a failure
    (use in CI/Docker where the CLI exists). Without `POML_CLI` it prints a
    skip notice.
    The first run found four more dead includes left over from the
    "unified routing" refactor: `tasks/code`, `tasks/test`
    (`../code_assistant.poml`), `tasks/review` (`code_review.poml`),
    `tasks/daily_quiz`/`language_learning` (`language_instructor.poml`) — now
    point at `personas/*.poml`.
26. **Provider changes required a restart.** `GatewayState.llm` is now a
    hot-swappable `LlmHandle`; `/login`, `/logout` and dashboard secret updates
    rebuild the router in place.

## New in this pass: `/login`

- TUI `/login` / `/logout` (`src/tui/app.rs`), gateway
  `GET /v1/providers`, `POST /v1/providers/login` (`src/gateway/client_api.rs`,
  `src/gateway/providers.rs`). Runs on the gateway host, so a remote TUI logs
  the remote machine in. Prints `/context set settings.provider=… settings.model=…`
  hints on success.
- Codex provider (`src/gateway/llm/codex.rs`): ChatGPT-subscription OAuth
  tokens (imported from the Codex CLI's `~/.codex/auth.json`, or via
  `codex login --device-auth` spawned on the gateway host, or pasted with
  `--auth-json`) against the Codex Responses API, with streaming, tool calls
  and automatic token refresh. Tokens are stored under `custom.codex_auth`.
- Master-key retention is now **opt-in** (`PRAXIS_RETAIN_MASTER_KEY=1`);
  retained passwords use zeroizing storage. Without it, logins are in-memory
  only; the dashboard can persist credentials with an explicit master password.

## Audit follow-up fixes

22. **Fixed: stale role catalogs.** `contexts/standard.sm` applies its catalog
    through a workflow override on every routing pass, not a missing-only
    `_default`. Regression test starts with a stale saved catalog.
24. **Fixed: oversized task prompts.** Shared compact tutor instructions now
    serve `teach`, `language_learning` and both quiz entry points, including
    actual due-card selection (the old teach state always claimed no due items).
    `20-tasks` and transcript checking no longer include the large duplicated
    runtime. Real POML 0.0.8 renders: teach 3867 chars, 20-tasks 1978,
    daily quiz 2770, transcript check 1101 (synthetic default context).
    Render tests enforce per-template budgets and exercise fixed/entry/continuous
    routing. Character estimates are not exact model token counts.
25. **Fixed: inactive settings.** Discord attachment downloads now require
    `download=true`, after channel authorization; load failures deny downloads.
    `feedback_enabled` gates external agent feedback, with a shared per-session
    sliding window using the validated `feedback_max_per_5min` and
    `feedback_window_secs`. Feedback tools cannot silently enable delivery;
    web progress/final replies remain available. `tool_result_limit` is now
    load-only legacy compatibility, not serialized; `_output`/`read_tool_result`
    remain its replacement. Unused `feedback_template` was removed; old values
    still deserialize harmlessly. See `docs/CONTEXT_VARIABLES.md`.
27. **Fixed offline; live verification outstanding.** Codex uses the bounded
    byte-oriented SSE decoder (CRLF and fragmented UTF-8), requires terminal
    success, validates complete tool calls, handles refusals and tool deltas,
    and reports typed failures without echoing server messages. Incomplete
    responses never execute tools; fixed subscription output limits are not
    retried with an ignored `max_tokens`. Exhausted primary/secondary rate
    windows and Retry-After/reset metadata reach the router. Parallel 401s share
    token refresh, and retired-router refresh callbacks cannot undo logout.
    Tests cover the request shape, images/tool history/reasoning, refresh,
    rate limits, malformed/truncated/failed streams and router recovery.
    **No live subscription call has been claimed.** Explicit read-only smoke
    test: `PRAXIS_CODEX_AUTH_FILE=/path/to/auth.json cargo test --lib
    codex_live_smoke -- --ignored` (fresh access token, no rotation/persistence).
28. **Fixed: heuristic CLI parsing.** The device-login job continuously drains
    both pipes into bounded storage. Polls display late output without guessing
    English wording, line count or code position. Failures reach the user;
    timeout/logout kill and reap the child. Cancellation prevents a late import
    from resurrecting credentials. Synthetic CLI tests cover delayed codes,
    noisy output, failure, timeout and cancellation. Stored refreshed tokens
    take precedence over stale CLI tokens; pending model choices are retained.
    `/login codex --device-auth` explicitly renews a rejected login. CLI argument
    parsing consumes JSON as a flag value (including leading whitespace), never
    mistakes it for a model, and rejects conflicting/unknown options.
29. **Fixed: false-positive endpoint probes.** Successful probes require the
    expected JSON model-list shape. Only llama.cpp may fall back from a 404 on
    `/v1/models` to a verified root `/health` response. Its own API key is used,
    not Ollama's. Generic 404/HTML/invalid JSON/auth failures are rejected.
30. **Fixed: implicit master-key lifetime expansion.** Retention defaults off;
    only `PRAXIS_RETAIN_MASTER_KEY=1` or `true` opts in. Retained/replaced key
    allocations are zeroized, persistence does not clone the password, and
    startup clears its original password after optional retention. Tests cover
    the secure default and opt-in parsing. Retention still intentionally grants
    the gateway lifetime access to the encrypted store; see README.
31. **Fixed: dashboard Codex import.** `codex_auth` is an explicit write-only
    field accepting validated CLI or normalized JSON. GET returns only `***`,
    masked round-trips preserve the login, empty values remove it, and malformed
    credentials cannot overwrite it. The dashboard lists the field with import
    instructions; its existing master-password save and hot reload apply.

### Additional regressions exposed by the real-POML tool-loop tests

- **Fixed: discovery unavailable in narrow states.** Both shipped workflows
  now keep `search_tools`, `search_skills`, `use_skill` and `read_tool_result` in
  their common agent group. Previously even `teach` could not discover its
  required memory tools. Tools still require activation/discovery; shell tests
  explicitly select the coding state rather than bypassing the allow-list.
- **Fixed: routed/discovered schemas diverged.** State routing now uses the
  canonical enabled built-in contracts, including tools without category
  metadata. Memory no longer requires a nonexistent `profile` argument, images
  use `path` rather than `image_path`, and `_output` receives a complete schema
  instead of a dangling reference. Disabled built-ins cannot be re-enabled or
  shadowed by plugins. Contract-parity and real tool-loop regressions cover this.
- **Corrected and strengthened: memory-discovery integration fixture.** The
  actual save receipt rejected the old fixture's missing `expected_profile`;
  routing had selected the missing `states/standard/standard` profile, not
  legacy `standard` memory. The test now discovers schemas, creates/loads a
  named profile, reads before writing with both expected fields, and checks
  successful tool receipts and persistence through a reopened DB on both chat
  and agent paths. It also checks user/legacy-profile isolation and that tool
  discovery expires at task end. Runtime profile safeguards were not relaxed.
- **Fixed: discovery without a state allow-list left empty schemas.** Selected
  tools now receive full contracts without hiding other allowed tools. `None`
  mode offers only activated/discovered tools. Empty/cyclic group resolutions
  stay closed in all modes, with and without a database. An empty native DB
  catalog no longer falls back to static tool contracts; disabled native tools
  still cannot be shadowed by plugins. Registry regression tests cover these
  boundaries.
- **Fixed: compacted history disappeared.** Runtime system prompts now append
  the stored conversation summary even with minimal/custom templates, avoiding
  duplicate injection when an author already rendered it. The existing offline
  integration test covers successful and failed compaction.

## Compatibility decisions (not unresolved defects)

- Unknown/unavailable `activated_tools` now emit a bounded diagnostic; late
  plugin names remain accepted. A regression test distinguishes installed tools
  from unknown groups/plugins.
- Legacy `tool_groups`/`full_tool_schemas` stay merged so saved contexts do not
  lose tools before migration.
- Root `templates/standard.poml` remains a role-aware compatibility fallback;
  state-specific `states/standard/standard.poml` remains the explicit SM target.
- Prompt caps are regression tripwires, not a guarantee that arbitrary user
  memory, tool schemas and history fit a 4096-token model context.

## TUI and diagnostic follow-up

The offline merge retains the audit fixes above and adds the TUI fixes:
explicit/blank line breaks and indentation, column-aware wrapping, bottom
padding, and scroll offsets that do not overflow on large transcripts. Failed
tool previews are cleared, provider errors include specific safe causes and
request IDs, and opt-in debug logs omit stream payloads. Both sets of regression
tests are retained; live Codex compatibility remains unverified.

The disappearing-box regression is also covered: completion popups cannot erase
the composer on small terminals; validated replies survive the saved-history
handoff and tool continuations; disconnects keep clearly marked partial output
until history/final events recover it. Durable IDs prevent long replies with
the same opening text from being collapsed into one.

## Verification

### Earlier audit (before the TUI merge)

Verified offline against the audit source with default Cargo features and real
Microsoft POML **0.0.8** (installed in a temporary virtualenv, not the repository):

- `cargo test -j 1`: **661 library tests passed, 29 ignored; 13 binary tests
  passed**. All 25 shipped root templates rendered; routing, prompt budgets
  and due-card filtering were checked with the real CLI, not skipped.
- Explicitly enabled `gateway::message_handler::message_tool_loop_tests`:
  **16 passed, 0 failed**, including memory discovery on both handler paths.
  Providers, CLI fixtures, files and databases are synthetic/local; no live
  or paid-service tests were enabled indiscriminately.
- `cargo build -j 1`: **passed** (normal default-feature development build).
  Existing unrelated compiler warnings remain.
- `node --check static/app.js`, `node scripts/test_ui_static.js`,
  `node tests/test_chat_commands.js`, and `node scripts/test_memory_ui.js`:
  **passed**.
- `git diff --check`: **passed**; tracked changes and new source files reviewed.

Reproduce the Rust verification with Node and a locally installed POML CLI:

```bash
export POML_CLI=/path/to/poml/js/cli.js
export PRAXIS_REQUIRE_POML=1
cargo test -j 1
cargo test --lib -j 1 gateway::message_handler::message_tool_loop_tests -- --ignored
cargo build -j 1
```

**Live Codex verification remains outstanding.** No real credentials were read,
no real tokens were rotated, no live subscription request/device login was made,
and no deployment was started. The opt-in smoke command in #27 is separate from
these passing offline checks.

### Combined offline merge and streaming-visibility verification

Merged the TUI/diagnostic changes onto `d28eb85`, retaining shared OAuth refresh,
rate-window metadata, secret-store safeguards, login fixes and workflow changes.
On the agent qube, with one build job and debug symbols disabled:

- `cargo test --offline -- --test-threads=1`: **689 library tests and 13 binary
  tests passed; 29 optional library tests ignored**. This includes both Codex
  suites, reconnect/history handoff races and popup/composer rendering tests.
- The JS syntax, static UI, chat-command and memory UI checks listed above:
  **passed**.
- No POML CLI was available in this qube, so real-POML gated/ignored integration
  checks were not rerun here. Their earlier audit results remain separate above.
- No live model requests, real credentials, token rotations or deployment were
  used for this merge verification.
