# Context change log

Every context (one per conversation/session) keeps an append-only log of its
changes. The context row is the current snapshot; the log says how it got there.

Each event has:

| Field | Meaning |
|---|---|
| `seq` | Per-context sequence number: 1, 2, 3, … |
| `kind` | `created` (first save) or `changed` |
| `source` | Who changed it: `user`, `model:set_context`, `tool:<name>`, `workflow` (state-machine routing), `compaction`, `runtime` (the agent loop's own bookkeeping, e.g. `turn`) |
| `changes` | Changed paths with `before` → `after` (values over 512 bytes appear as size + hash) |
| `at` | UTC time |

Saving an unchanged context writes no event. Up to 10 000 events are kept per
context. Implementation: `src/db/context_events.rs`, table `context_events`
(migration 017).

## Reading it

- TUI: `/changes` (last 20) or `/changes 50`. Changes made by the model, a tool,
  the workflow or compaction also appear briefly in the status line.
- API: `GET /v1/context/<id>/events?after=<seq>&limit=<n>` returns
  `{"seq": <current>, "events": [...]}`. A client that remembers the last `seq`
  it saw can fetch exactly what it missed, e.g. after a reconnect.
- The live event stream (`/v1/events/<id>`) announces each change as
  `context_changed` with `seq`, `source` and the changed **paths only**. Values are
  never broadcast; they are available through the authenticated endpoint above.

## Changing something

Past events are never edited. You change the context by **appending a new
change**; the log then shows the old value, the new value and that you made it.

- TUI / Discord: `/context set settings.max_llm_turns=20` (logged as `user`).
- API, with protection against overwriting changes you have not seen:

  ```bash
  curl -X POST http://127.0.0.1:3537/v1/context/<id>/changes \
    -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d '{"set": {"settings.max_llm_turns": 20}, "expected_seq": 41}'
  ```

  If the context is no longer at seq 41 (the model or a workflow changed it in
  the meantime), the request fails with `409` and the current `seq`; nothing is
  written. Read `/events?after=41`, decide, and retry with the new seq. Without
  `expected_seq` the change applies unconditionally, like `/context set`.

The same validation as `/context set` applies: identity fields cannot change,
user-only settings stay user-only, workflow guards still run.

## Undoing a change

There is no rewind of the log. Read the `before` value of the change in
`/changes` and set it again; that is a new, visible change.

## Reacting to events: `[on]` rules

A workflow (`.sm` file) can react to what happens instead of re-reading state:

```ini
[on]
tool_failed execute_terminal x2 -> state debugger
tool_failed * x5               -> stop "Too many failing tool calls"
tool_ok write_file             -> set sm_data.dirty = true
changed sm_data.role by model  -> note "Role changed: re-read the plan first"
```

Triggers:

| Trigger | Fires when |
|---|---|
| `tool_failed <tool\|*> [xN]` | the Nth failure in a row of a matching tool (default 1); a success of that tool resets the count |
| `tool_ok <tool\|*>` | a matching tool call succeeded |
| `changed <path\|*> [by <source>]` | a logged change touched the path or anything below it; `by` matches the source prefix (`user`, `model`, `tool:write_file`, `compaction`, …) |

Without `by`, `changed` ignores the runtime's bookkeeping (`runtime`) and the
rules' own writes (`workflow…`), so a rule cannot trigger itself.

Actions:

| Action | Effect |
|---|---|
| `state <name>` | switch state, through the workflow's guards (a guarded state is not entered; the model gets a note why) |
| `set <path> = <value>` | set a context value (JSON, otherwise a string); logged with source `workflow:on` |
| `note <text>` | system note the model reads before its next turn |
| `stop <text>` | stop the task, like the loop guard |

Rules run in the agent loop after each round of tool calls: first the tool
outcomes in order, then every context change logged since the previous round.
A task fires at most 32 rule actions; a workflow has at most 64 rules. Unknown
triggers or actions and `state` targets the workflow does not declare are parse
errors. Implementation: `src/gateway/event_rules.rs`.

What stays state-driven: `[auto]` rules and `[transitions]` still evaluate the
current context, and the context row remains the source of truth. The log
records and triggers; it does not replace the snapshot.
