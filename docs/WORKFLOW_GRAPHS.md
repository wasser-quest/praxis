# Workflow graphs and state-specific Decision IR

Select `branching-coding` in the context's `settings.sm_file`, set the active
state to `route`, and start a new task. The bundled workflow selects its
`graph-coding` template. It uses the same chat provider/model, including Ollama;
there is no separate IR endpoint.

Graph states select their declared templates even when the previous workflow
left a different template in the context. A new task after successful completion
restarts at the graph's entry node. After an interrupted task, the current node
is retained but previous receipts and back history are unavailable; select
`route` before starting again if the retained node cannot recover.

This example uses the existing `verified_rust` plugin. Install and enable its
example capabilities as described in [Decision IR setup](DECISION_IR.md).
Point `WORKSPACE_DIR` or `praxis run --workspace-dir /absolute/project` at a
prepared Rust project containing `Cargo.toml`, `Cargo.lock`, `src/` and an
existing source file. Install its dependencies before starting the task.

## Declaring a graph

```ini
@name "Coding workflow"
@description "Choose actions and verify their results."
@routing graph
@start route

[state route]
settings.activated_tools = ["execute_decision", "agent_next", "agent_back"]
[state inspect]
settings.activated_tools = ["execute_decision", "inspect_file", "agent_next", "agent_back"]

[node inspect]
title = "Inspect source"
description = "Read the source and its current hash before editing."

[transitions]
route -> inspect
inspect -> route

[edge route:0]
title = "Read the source"
description = "Choose this edge when you need to understand the existing code."

[decision_ir]
N = agent_next
K = agent_back
[decision_ir inspect]
R = inspect_file
N = agent_next
K = agent_back
```

`[node STATE]` and `[edge STATE:INDEX]` hold display/purpose metadata; they do
not become editable context variables. Titles and descriptions are JSON strings.
Explicit transitions preserve existing `from -> to : when condition` syntax.
Indices follow the outgoing transitions' declaration order, including blocked
edges. Five outgoing transitions are numbered **0–4**; 0–5 means six.

`@routing graph` enables explicit navigation. `[auto]` and a Decision-router
profile do not autonomously switch graph nodes. Graphs without `@routing graph`
retain existing first-matching-transition and `@steps` behavior. The Graphs
view also displays these legacy connections and automatic rules.

## Navigating

```json
{"ir":"1 N {\"edge\":1,\"from_state\":\"route\"}"}
```

In `branching-coding`, this chooses `implement`. Through normal tools, the same
selection is `agent_next` with `{"edge":1,"from_state":"route"}`.

`agent_next` without `edge` requires exactly one eligible edge in graph mode;
otherwise its diagnostic lists all choices. `from_state` rejects stale choices.
The runtime rechecks conditions and current verification guards before saving
the destination. Invalid, blocked, cancelled or stale transitions leave the
state and traversal history unchanged.

```json
{"ir":"1 K"}
```

This invokes `agent_back`: return to the last node actually visited in this
task. It is not an arbitrary incoming edge and it does not roll back source
changes. Back navigation still checks the destination's receipt guards.
History belongs to the task, resets for a new task and is limited to 512 forward
transitions. Both successful forward and back transitions return an execution
record containing the source, destination and new graph choices.

The runtime supplies the workflow name/purpose, node purposes, current choices,
blocked reasons and visited route to the model. Model context updates cannot
change graph states. Editing a pinned workflow or moving its state externally
requires starting a new task.

## State-local IR

`[decision_ir STATE]` **replaces** the global `[decision_ir]` table. Omission
inherits the global table. An explicitly empty local section disables IR in
that state. Repeat navigation mappings in each local table that needs them.

All tables and graph policy are pinned together for the task. The active
state selects the effective table at execution and before the next model call.
An alias never grants extra authority: the native target must still be enabled
and allowed, and plugin targets must have the declared owner and action contract.

In the bundled example, `route` has N/K; `inspect` has R/N/K; `implement` has
R/M/N/K; `build` has B/N/K; `test` has T/N/K; `review` has R/N/K; and `done` has
C/K. Both entering `done` and completion require current verified modification,
build and test receipts.

Include the final result/usage guide as text alongside the C tool call. Agent
mode stops once completion is verified and preserves that text. Chat mode keeps
its existing follow-up request to summarize the tool result.

## Dashboard

- **Graphs:** select a context and active or preview workflow; zoom, pan and
  inspect nodes. Current/visited states and blocked connections are marked.
  Previewing a workflow changes no context and borrows no execution receipts.
  During a task, the route is live; after it finishes, the view can show its
  audited route. Node selection only opens the inspector.
- **Messages:** filter conversation/tools, model calls/prompts or transitions/
  compaction. Expand actual system messages, prompt changes, setting changes,
  IR, tools and request timings. Captured attempts include retries and the
  actual selected provider and output allowance. Prompts are deduplicated by
  SHA-256 and stored separately from LLM history. Earlier prompts are unavailable.
- **Chat:** estimated live throughput becomes provider-usage throughput when a
  call completes. First-output latency measures visible text/tool output. The
  history estimate/limit and compaction trigger use runtime counters. The
  compaction trigger applies before a model call, when older complete turns can
  be summarized; it is not the model's context-window or output limit.

Token estimates are labelled. Missing usage/context-window size is shown as
unavailable. Throughput uses reported completion tokens divided by attempt wall
time, including time to first output; it is not an isolated decoder benchmark.
Audit records are scoped to the current conversation session and never replayed
as chat history. Existing authentication protects the graph, audit and usage APIs.

## Test prompt

> Implement a playable Snake game in this prepared Rust project, changing only
> the existing src/main.rs. Use execute_decision, one instruction per call, and
> the current state's IR table. Navigate the branching-coding graph with N/K.
> Read the current source/hash, apply a verified modification, then obtain fresh
> build and test receipts. Request completion only through review → done → C.
> Include logic-only unit tests and a brief usage guide. Report actual receipts
> and checks; I will test the interactive rendering separately.

For Ratatui, prepare `ratatui = "0.29"` and `crossterm = "0.28"` in the project
before this prompt; the source-modification contract does not install packages
or create Cargo manifests.

## Local verification

```sh
cargo test --lib state_graph
cargo test --lib execution_
node scripts/test_workflow_dashboard.js
node scripts/test_ui_static.js
node scripts/dashboard_fixture.cjs
```

The fixture serves synthetic API data with the real dashboard assets at
`http://127.0.0.1:44148`. It makes no model calls and does not use private data.
