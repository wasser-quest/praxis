# TUI missing terminal commands — 2026-09-29

Tracking: [upstream #3](https://forgejo.the.grid/Marvin/praxis/issues/3),
[fork #3](https://forgejo.the.grid/Praxis/praxis/issues/3).
Reproduced against `ad9d08c`; branch subsequently fast-forwarded onto `testing`
`72c47f7` after web-history PR #8 merged. The TUI sources were identical in both
baselines. Only synthetic fixtures were used; no commands in fixtures execute.

## Investigation log: hypothesis → test → evidence → conclusion

1. **Persisted terminal commands disappear at the TUI history boundary.**
   Feed an assistant message with an empty body and a structured
   `execute_terminal` call into the unchanged `App::append_messages`. Replay
   through the history event after `stream_end`, then repeat history.
   Expected one command row; actual zero. With assistant prose, two terminal
   calls, a nonterminal call and a tool result, expected four rows; actual two.
   **Confirmed:** the renderer skips tool-only assistant rows and never reads
   command arguments from assistant messages containing prose either.

2. **Rendering commands from complete persisted arguments repairs the loss.**
   Add command rows on all three assistant reconciliation paths: tool-only,
   fresh prose and matching optimistic/SSE prose. Retest both history/SSE
   orders, repeated polls, reused call IDs, reloads, three terminal tool names,
   multiline Unicode, and result ordering. These tests pass. Existing TUI/SSE
   regressions remain green. Invalid JSON/missing or non-string commands show
   an explicit unavailable label, not a misleading partial command.

3. **The legacy fallback identity drops distinct tool-only messages.**
   Include messages without DB IDs, equal empty body/call ID, but different
   structured commands. After the rendering fix, actual six rows versus seven
   expected. `bubble_key` uses only role, result call ID and body in the legacy
   path; assistant tool calls are absent from the key.
   **Confirmed:** include the structured tool calls in an unambiguous serialized
   tuple. Retest: both legacy commands appear exactly once on repeated polls.

4. **Rendered terminal cells, not just transcript state, must retain commands.**
   Drive the real renderer using ratatui `TestBackend` at 40/80/100 columns after
   a truncated live preview, stream end and history reload. Check command head
   and tail, wide Unicode glyphs, composer cursor, absence of unrelated fields
   and a single durable row. The test passes. The initial screenshot assertion
   was corrected to account for wide-glyph continuation cells in `TestBackend`;
   application rendering was not changed to accommodate the assertion.

## Fix

`App::append_terminal_commands` reads only the top-level string `command` of
complete JSON arguments for `execute_terminal`, `run_background`, and
`vm_shell`. The existing `Tool` bubble renders that string with a `— command`
label; no schema/backend/SSE change is needed. Nonterminal payloads and other
argument fields are not newly exposed. Assistant prose and tool results stay
unchanged, and empty command strings are preserved as valid strings.

Command rows share their parent message's deduplication boundary, preserving
call order and keeping equal/reused provider call IDs distinct across saved
message IDs. The legacy identity now includes structured tool calls.

The gateway saves full arguments **before** emitting `tool_call`, whose preview
may be truncated to 200 characters. The TUI deliberately does not interpret that
preview as an executed/full command. During a live run, the existing local
250 ms DB poll or remote one-second authenticated history poll supplies the
complete command; it remains visible after reload. The label says `command`,
not `succeeded`: a persisted request is not proof that execution completed.

The raw command stays exact in the transcript. Existing terminal display
sanitization still normalizes tabs/line endings and removes control characters.
Nothing is sent to a shell or back to the model by this renderer.

## Verification and regression prevention

New tests in `src/tui/app.rs` and `src/tui/ui.rs` cover:

- All three terminal tool names; empty and nonempty assistant prose.
- Both SSE/history handoff orders, repeat polls, reset/reload, reused call IDs.
- Multiple calls and result ordering; no unrelated tool payload disclosure.
- Long commands (>200 characters), Unicode, indentation, literal backslashes.
- Invalid/truncated/nonobject JSON, missing/non-string and empty commands.
- Legacy history without message IDs.
- Actual authenticated remote HTTP history with a synthetic wiremock server.
- Actual ratatui terminal cells at three widths after live preview and reload.

Commands:

```sh
# Normal project build: real application dependencies and DB implementation
cargo test --locked tui:: -- --test-threads=1

# Smaller diagnostic harness: actual TUI/SSE sources and Message types,
# but unrelated backend services stubbed (unsupported calls panic).
cargo test --locked -p praxis-tui

node scripts/test_chat_terminal.js
node scripts/test_chat_history.js
node scripts/test_ui_browser.js
node scripts/test_ui_static.js
node --check scripts/test_tui_selection_pty.js
git diff --check
```

The isolated runner copies dependency declarations and the project lockfile into
an ephemeral crate. It does not rewrite application code or the real lockfile.
The initial reproduction ran before application changes: 34 existing tests
passed and the new command assertions failed. With the fix and all new cases,
39 isolated TUI/SSE tests passed using the project's locked dependency versions.
This harness alone does **not** certify real DB access, startup configuration or
other backend integration. It is supplied for repeatable diagnosis on small
containers, not as a replacement for normal application tests.

Final verification also built the normal application with
`cargo test --locked tui:: -- --test-threads=1`: **37 TUI tests passed**, using the actual DB type and
other application dependencies (no harness stubs). The two additional isolated
tests are from the shared SSE module. The targeted run is not a claim that all
other application tests ran. Existing compiler warnings were reported; no
compiler errors occurred. A separate `cargo test --locked sse:: --
--test-threads=1` run passed both shared SSE tests, and the final TUI rerun again
passed all 37 tests.

The web terminal, history (including the merged linear-refresh regression),
static UI and six-viewport/theme browser UI suites passed on the updated branch.
Syntax/whitespace checks passed. A first combined Rust/browser command lacked
Chromium's temporary `lib/` library path and failed at browser launch with
`libdbus-1.so.3` missing; restoring the documented browser environment resolved
it without any application change. Rust/C and browser test prerequisites were
installed under `/tmp`, without changing system packages or the running service.

## Limits / separate work

- No interactive terminal emulator/local clipboard or real remote server test.
- Live command visibility retains the existing polling latency; no new SSE event
  or immediate refresh mechanism was introduced.
- Legacy history without DB IDs cannot distinguish *identical* repeated messages;
  this patch prevents distinct command arguments from colliding, not that
  pre-existing identity limitation.
- No TUI layout-performance optimization, text selection, token metrics, folding,
  or structured result formatting in this patch (issues #2 and #4–#6).
- No live service restart or deployment performed.
