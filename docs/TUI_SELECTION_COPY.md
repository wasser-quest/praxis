# TUI selection, copy and paste — 2026-09-29

Tracking: [upstream #2](https://forgejo.the.grid/Marvin/praxis/issues/2),
[fork #2](https://forgejo.the.grid/Praxis/praxis/issues/2).
Baseline: `testing` at `40dd13a` (includes merged terminal-command PR #9).

## User guide

| Key / command | Action |
| --- | --- |
| F2 or `/mouse` | Toggle mouse capture. OFF gives drag selection to the terminal emulator; ON restores TUI wheel scrolling. |
| PageUp / PageDown | Scroll history in either mouse mode. |
| F3 or `/copy` | Enter whole-message keyboard copy mode, initially selecting the last saved transcript row. F3 again leaves it. |
| Up / Down, Home / End | Select previous / next, first / last message in copy mode. The selected message is highlighted and brought into view. |
| Ctrl+C or `y` in copy mode | Request copying the selected message's original content through OSC 52. Does not submit text or quit. |
| Esc | Leave copy mode and return to the unchanged draft. |
| Ctrl+Q | Intentionally quit in either mode. |
| Ctrl+C outside copy mode | Quit, preserving the existing shortcut. |
| Terminal paste shortcut | Insert bracketed multiline/Unicode paste into the draft, without submitting it. Leave copy mode first. |
| `/help` | Show bindings and clipboard fallback instructions. |

Native selection copies arbitrary visible text using your terminal's normal copy
shortcut/menu (often Ctrl+Shift+C or Cmd+C). Some terminals also support
Shift-drag while mouse capture is ON. Emulator/multiplexer behavior varies.
Praxis does not implement character-range mouse selection itself. F2 explicitly
releases mouse reporting; keyboard navigation remains available.

Keyboard copy copies **one whole saved transcript row**, not box borders, labels
or visually wrapped lines. This includes persisted terminal commands and tool
results. The selected content is a snapshot, so asynchronous updates cannot
silently change the copied text. Live-only streaming fragments are not keyboard
copy targets until saved; native selection is available for visible fragments.

Copy mode does not edit or send the draft, attachments or history. Enter and
other editing keys are ignored; bracketed paste shows a prompt to leave copy
mode first. Session reset/clear drops the selection and any queued copy request.
On terminal reflow the selected message is revealed again. Normal PageUp/Down
scrolling remains available for inspecting a long selected message.

### Clipboard support and safety

OSC 52 sends a clipboard-write request to the terminal displaying the TUI, so
it can work over SSH without accessing the gateway machine's clipboard. It
requires terminal support/permission; tmux/screen may block it. Praxis deliberately
does not enable multiplexer passthrough, read/query the clipboard, launch shell
clipboard helpers, or bypass terminal policy. The footer says **Copy requested**,
not **Copied**, because write success does not prove OS clipboard acceptance.
If unavailable, press F2 and use native selection/copy instead.

Only an explicit Ctrl+C/`y` in copy mode writes OSC 52. Message bytes are base64
encoded, so embedded terminal controls cannot become additional escape commands
in that write. The original content (including whitespace/control characters)
is preserved on the clipboard; inspect untrusted commands before pasting them
into a shell. Messages above **100,000 UTF-8 bytes** are refused, not truncated;
terminals may impose lower limits. Selection is view-only: nothing is sent to the
model, gateway, shell, logs or an external clipboard service.

## Investigation log — hypothesis → test → evidence → conclusion

1. **Mouse capture blocks ordinary native selection and has no escape toggle.**
   Inspection: setup unconditionally enables capture; the event handler handles
   only wheel events, discarding press/drag/release. Regression on unchanged
   production source: F2 should report native-selection mode but does nothing.
   **Confirmed.** Add explicit F2/`/mouse` state, synchronize real terminal mode
   bytes only when that state changes, and ignore queued wheel events after
   releasing capture. Retest: disable/enable bytes, wheel restore and PageUp
   pass. At this stage 42 isolated tests passed; only keyboard copy remained red.

2. **There is no keyboard copy route; Ctrl+C always quits.**
   Reproduction: F3 then Ctrl+C with a saved Unicode message and unsent draft.
   Actual `should_quit=true`, expected false. Separate baseline paste control
   passed, confirming paste was already supported and must not be 'fixed' by
   rewriting it. **Confirmed.** Add explicit whole-message copy mode, highlighted
   snapshot, guarded clipboard request and documented intentional quit routes.
   Retest with both local and remote configurations: exact content encoded once,
   no HTTP requests, no draft mutation/send, no automatic clipboard write.

3. **Reflow can lose the selected row even though copying still targets it.**
   Draw at 100 columns, keep selection, resize to 24 columns without resetting
   mode. The new test failed: selected text disappeared while old scroll offset
   was retained. **Confirmed.** Viewport-size changes now mark the selection for
   reveal; existing row-scroll behavior remains unchanged otherwise. The reflow
   regression and all prior tests pass.

4. **State tests alone do not establish event-loop/terminal integration.**
   Build the real application; run it through a pseudo-terminal with actual
   crossterm input parsing and stdout capture. Use a temporary SQLite DB and
   synthetic authenticated loopback HTTP gateway. F2 emits disable/enable mouse
   reporting, F3 activates copy mode, Ctrl+C emits the exact OSC 52 payload and
   does not exit, Enter does not submit, Ctrl+Q exits and terminal modes restore.
   Local and remote paths pass. The first local fixture insert failed because
   it omitted the `contexts` parent row required by the real schema; the fixture
   was corrected, not the application or foreign-key enforcement.

## Verification and prevention

```sh
cargo test --locked tui:: -- --test-threads=1
cargo test --locked sse:: -- --test-threads=1
cargo test --locked -p praxis-tui
cargo build --locked --bin praxis
node scripts/test_tui_selection_pty.js target/debug/praxis
node scripts/test_chat_terminal.js
node scripts/test_chat_history.js
node scripts/test_ui_browser.js
node scripts/test_ui_static.js
node --check scripts/test_tui_selection_pty.js
node --check scripts/test_tui_selection_pty.js
git diff --check
```

Final results after the reflow fix:

- Normal application: **47 TUI tests + 2 shared SSE tests passed**.
- Isolated runner: **49 passed** (same TUI/SSE tests, not additional coverage).
- Normal debug binary build passed, with existing unrelated compiler warnings.
- Real-binary PTY startup, local SQLite and remote HTTP scenarios all passed.
- Web terminal/history (including linear-refresh counts), six-viewport/theme UI,
  static UI, JavaScript syntax and whitespace checks passed.

The PTY runner requires Linux/util-linux `script` + `stty` and Node 22+ with
`node:sqlite`. It uses a minimal child environment, temporary data/log directories,
synthetic key, loopback server, and captured terminal output. It never forwards
OSC 52 to the user's actual terminal or alters a real clipboard. It checks that
only GET requests reach the mock gateway and no new local message is persisted.

Regression coverage includes empty history, navigation bounds, session reset,
explicit-only/one-shot writes, byte-size limits, clipboard/mouse write failures,
Unicode/combining/emoji/newline/tab/control payloads, draft/attachments retained,
local/remote no-submit paths, intentional quit, slash commands/help, selection
highlight and snapshot rendering at 24/40/80/100 columns, growth and reflow.
The isolated harness uses actual TUI sources and project dependency versions,
with unrelated backend services stubbed; normal application and PTY tests are
separate checks of the real dependencies and event loop.

No live service restart/deployment. No claim that all application tests ran or
that every desktop terminal/multiplexer accepts OSC 52. Actual OS clipboard and
mouse-drag behavior still require manual validation in the user's terminal.
