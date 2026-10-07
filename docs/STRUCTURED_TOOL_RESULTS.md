# Structured tool-result presentation — issue #6

Tracking: [upstream #6](https://forgejo.the.grid/Marvin/praxis/issues/6),
[fork #6](https://forgejo.the.grid/Praxis/praxis/issues/6).
Baseline: `testing` at `5f233f1` (usage-integrity PR #11 already merged).

## Investigation log: hypothesis → test → evidence → conclusion

1. **Hypothesis:** result JSON is rendered as a string without decoding its
   stdout/stderr fields; the problem is presentation, not lost newlines in storage.
   **Test before production edits:** feed
   `{"stdout":"FIRST\nSECOND 世界","stderr":"warning\nlast","exit_code":3}`
   into real dashboard history and TUI history/rendering, using synthetic data.
   **Evidence:** browser regression fails with `stdout must decode JSON newlines`.
   Ratatui TestBackend renders FIRST and SECOND on the same row (row 5), including
   the literal backslash-n. Both tests fail before any production edits.
   **Conclusion:** confirmed. `renderChatMessage` assigns `m.content` directly to
   `.tool-out.textContent`; `append_messages` passes the same raw JSON string to
   the TUI bubble renderer. Storage is not the correct place to fix this.

2. **Fix and retest, TUI first:** distinguish saved result bubbles from commands
   and in-progress arguments. Decode the recognized result once per saved row;
   cache presentation separately from original content. Render decoded fields,
   retain the raw snapshot for F3 selection/copy. Original reproduction passes;
   all 48 TUI tests at this stage pass.

3. **Fix and retest, browser next:** share the result renderer between history
   and live tool-result previews, not tool-call arguments. Render text via
   textContent. Add raw/formatted toggle and explicit raw-copy button. Cache the
   receipt; unchanged polls neither parse again nor replace text nodes.
   Original browser reproduction passes. Expanded tests exercise actual DOM,
   live preview→saved history reconciliation, copying, reload and session switch.

4. **Test-fixture correction:** an added DB unit fixture failed with SQLite
   `FOREIGN KEY constraint failed` because its dummy App had no saved context.
   This was not an application failure and prompted no production change. Keep
   authenticated remote-history/copy coverage in the isolation-compatible unit
   harness; test local SQLite persistence with the real-binary PTY fixture,
   which creates the context correctly and verifies stored bytes remain unchanged.

## Display contract

- Only a complete JSON object containing top-level `stdout` and/or `stderr`,
  with each present field a string, is decoded. This is schema-based, not a tool
  name allowlist, so nonterminal tools with the same result schema also work.
- Label stdout/stderr and exit_code, including empty strings, zero and null.
  Unknown fields (including nested metadata, retention IDs and truncation flags)
  remain visible in a metadata JSON section; nothing is silently discarded.
- Decode exactly once. JSON inside stdout is output data; do not recursively
  interpret it. Literal `\\n`, Windows paths, regexes, script/HTML text, and Unicode
  remain literal output. Unknown schemas, arrays, JSON strings, malformed or
  truncated JSON remain verbatim. Arbitrary nested wrappers are not guessed.
- Tool commands and still-generating arguments are never sent to the formatter.
- No storage schema, gateway result, model context, or execution policy changed.
  TUI raw content and browser raw receipts are retained byte-for-byte.
- TUI F3 or `/copy` selects a saved result, reveals its raw snapshot and allows
  Ctrl+C/y to request raw clipboard copy. Esc/F3 returns to formatted display.
  Existing 100,000-byte OSC 52 safety limit remains; larger copies are refused
  explicitly, not silently truncated. F2/native selection remains available.
- Browser `Rohdaten anzeigen` toggles raw/formatted view; `Rohdaten kopieren`
  copies the complete received receipt, independent of the scroll viewport.
  Raw-view choice and text selection survive unchanged polls (not full reload).
  Empty plain-text receipts have copy access but no meaningless format toggle.
- Gateway SSE result text may already be shortened to 400 characters. Mark live
  results as previews and label copy as `Vorschau kopieren`; invalid preview JSON
  stays raw. Authoritative saved history replaces it through existing polling.
  TUI continues using authoritative history rather than adding truncated result
  previews. This change does not reconstruct omitted upstream bytes.
- Decoded output is text, not HTML, shell input, markdown or terminal commands.
  Browser display uses textContent. Clipboard writes occur only after an explicit
  copy action; tests capture/stub them, never use the real clipboard.

## Regression prevention and verification

Shared corpus: `scripts/fixtures/structured_tool_results.json` is consumed by
both Rust and browser tests. Cases cover decoded line breaks, Unicode, empty
stdout/stderr, failure/success exit codes, null exit status, literal escapes,
paths/regexes, JSON embedded inside stdout, metadata/nested JSON, invalid schemas,
truncated input, empty/plain text, arrays, nonterminal results and deep unknown JSON.
Separate tests cover large Unicode receipts without presentation truncation.

Commands (normal build plus loopback-only fixtures):

```sh
cargo test --locked tui:: -- --test-threads=1
cargo test --locked gateway::llm:: -- --test-threads=1
cargo test --locked sse:: -- --test-threads=1
cargo test --locked -p praxis-tui
cargo build --locked --bin praxis
node scripts/test_tui_selection_pty.js target/debug/praxis
PRAXIS_PTY_TOOL_RESULT=1 node scripts/test_tui_selection_pty.js target/debug/praxis
node scripts/test_chat_terminal.js
node scripts/test_chat_history.js
node scripts/test_ui_browser.js
node scripts/test_ui_static.js
git diff --check
```

The real-binary PTY test runs local SQLite and authenticated synthetic remote
HTTP history, checks raw copy bytes, keyboard/mouse/paste/quit/teardown, and
asserts that stored content is unchanged and no chat is submitted. TestBackend
independently verifies decoded line layout and raw-mode rendering. Browser tests
check raw copy, raw-toggle state, exact text-node/selection/scroll preservation,
updated receipts, reload, session isolation, 360px layout, script-shaped output,
live truncated previews, reused saved call IDs, and zero formatter calls during
unchanged polls over 200 long results. Existing linear-history and command-copy
regressions are retained.

Final verification (all commands above passed):

- **52 TUI tests** and **2 shared SSE tests** passed in the normal application build.
- **128 provider/router tests** passed; 3 existing tests explicitly ignored
  (live Codex and external-POML integrations).
- Isolation harness: **54 passed**, including the same TUI/SSE tests; this is
  overlapping coverage, not 54 additional application integration tests.
- Normal debug binary built successfully, with existing unrelated warnings.
  Both prose and structured-result PTY runs passed startup, local and remote cases.
- Browser terminal/history (including five structured-result scenarios and the
  linear-refresh regression), viewport/theme UI and static UI suites passed.
- JavaScript syntax checks and `git diff --check` passed.

No deployment, service restart, live model request or application-wide full-suite
pass is claimed.
Issue #4's persistence/timing/visible token-rate feature and issue #5's view-only
folding remain separate work; this change does not implement them.
