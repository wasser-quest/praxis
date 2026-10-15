# Praxis — how it actually works

The long feature list lives in [`docs/archive/README.md`](docs/archive/README.md). This page is the plain-language version: what the thing is, why it is shaped like this, and what you can do with it.

## What it is

Praxis runs an AI agent on your own machine. You pick the model. You describe the behaviour in files instead of code. The agent then answers you — through the terminal, the web dashboard, or Discord — while a small kernel decides what it is actually allowed to do.

That last sentence is the whole design. The model is not trusted with authority. It can only use the tools you put in front of it, in the state it is currently in, and the kernel keeps a record of what happened.

## The five things you configure

1. **Context** — the facts about this user and this task: who is talking, what role they have, what has been remembered, what is pending. It is a JSON document the kernel owns. The model can suggest changes to it; the kernel decides whether they stick.
2. **State machine** (`.sm` file, e.g. `contexts/standard.sm`) — the states the agent can be in, what prompt each state uses, which tool groups it gets there, and which transitions are allowed. A state is not a mood; it is a permission set with a prompt attached.
3. **Templates** (Markdown files in `templates/`, with [MiniJinja](https://docs.rs/minijinja) for `{{ variables }}`, `{% if %}` and `{% include %}`) — the system prompt for each state, rendered from the context. Nothing is hardcoded in Rust; the prompt you read in a `.md` template is the prompt the model gets. `praxis template <name>` prints it. Legacy `.poml` files still render through the POML CLI.
4. **Tools** — the things the agent can actually do: read and write files, run a command, search, send a message. Tools arrive as packages. A state lists the tool groups it may use, so "the agent can run shell commands" is a line in a file, not a flag in a binary.
5. **Skills** — short, declarative instructions for a specific job (a search discipline, a coding method), as a folder with a `SKILL.md` (YAML front matter + Markdown). A skill is data the agent reads, not code it runs. `praxis skill check <folder>` validates one.

Put together: **context says who, the state machine says what is allowed, the template says how to behave, tools say what can be done, skills say how to do a particular job.**

## Why it is built this way

A prompt is not a program. If behaviour lives only in a prompt string, you cannot test it, version it, or explain why the agent did something. So Praxis keeps behaviour in files that a human can read and diff, and keeps authority in code that the model cannot touch.

Three consequences:

- **Declarative beats imperative.** You write `[state review]` with its tools and template, and the runtime applies it. That is why the setup looks odd until it clicks: you are describing a machine, not calling functions.
- **Routing is a decision, not a vibe.** A transition is only taken through the declared path (`agent_next`), with guards that can require evidence before a state is entered or a run is marked complete. This is also why changing `sm_data.role` by hand changes nothing — the graph, not a variable, is the thing that moves the agent.
- **The kernel keeps receipts.** Actions can be guarded by contracts: what was promised, what was checked before and after, what actually changed. When a run claims to be finished, there is something to point at.
- **A model that loops gets stopped.** A workflow's `[limits]` (same tool call, failing tools, identical replies, state flapping, repeated output) warn one step early and then abort the task, optionally back to a safe state.
- **Changes are events.** Every context change is logged with who made it (user, model, tool, workflow, compaction) and what changed. Workflows can react with `[on]` rules (`tool_failed execute_terminal x2 -> state debugger`), and clients catch up with `after=<seq>`.

The tradeoff is upfront ceremony. Once a workflow exists it is very good; creating the first one is where all the friction is. That is why the preflight, the graph view, and the `/state` briefing exist — they show you the machine you are standing inside.

## What happens when you send a message

1. Your message reaches the gateway (port `3537`) and is stored against your session.
2. The agent loop starts for that session: it loads your context, applies the state machine, checks the workspace preflight, and binds the action contracts for the current state.
3. The system prompt is rendered from the state's template and your context — every request, so a state change changes the prompt on the next turn.
4. The model answers. If it asks for a tool, the kernel runs that tool under the current state's permission set, and the result goes back as a tool message.
5. Guards and transitions are evaluated. The agent may move to another state, or the run may be marked complete through the exit edge (`_complete`).
6. The answer and the receipts are persisted; the terminal, dashboard, or Discord shows them.

The dashboard's graph explorer and the TUI `/state` command show the same graph: where you are, what the state can move to, and what is blocking each edge.

## What you can do with it

- **Chat** with any supported provider (OpenAI, Anthropic, Ollama, llama.cpp, OpenRouter, and others) from the terminal, the web dashboard, or Discord. Local models get a context meter, overflow recovery and a stable prompt prefix for the KV cache; the `billion_context` plugin puts the [billion-context](https://github.com/ranxianglei/billion-context) proxy in front of llama.cpp.
- **Define workflows** as state machines: a coding role with file and shell tools, a tutor that waits for real student input before giving feedback, a research role that only reads, a role that must show evidence before it claims success.
- **Install setups with `xis`**: a signed package can bring plugins, a workflow, personas, and a config profile in one plan, with an ownership lock and a change report.
- **Run scheduled work** (cron jobs), keep per-persona memory profiles, and use RAG over a vector store.
- **Extend it with plugins** — search, media, voice, VM control, model providers — as packages, each with its own declared tools and assets. A package can add a dashboard page and a status line or `/` commands in the terminal ([`docs/PLUGIN_TERMINAL.md`](docs/PLUGIN_TERMINAL.md)). The dashboard only shows what is active.
- **Watch it work**: execution timeline, actual prompts, token accounting, receipts.

## Running it

```bash
cargo run                 # gateway + dashboard
cargo run -- run --workspace-dir /path/to/prepared/project
cargo run -- run --minimal  # gateway, file tools and plugin management only
cargo build --no-default-features  # core-only binary: no VM, dashboard, shell, voice, Discord
```

`praxis plugin install <dir> --build` builds and installs a package; `praxis plugin list` shows what is installed and what each package provides. Plugin binaries are build output and are not committed.

Praxis reads configuration from the environment (there is no config file). The two services are separate: dashboard on `1337` (admin password), gateway on `3537` (API key). Keys and passwords are generated on first run; provider credentials are encrypted at rest.

A few things worth knowing, because they are the parts that bite:

- **Workspace preflight.** A workflow that declares required files or directories refuses to run against a directory that is not prepared — that is a trust boundary, not a nicety. A workflow that declares nothing needs no prepared project, so a bare `praxis` run can just chat.
- **Rate limits.** The gateway budgets requests per key. Chat writes get `GATEWAY_RATE_LIMIT_PER_MINUTE` (default 240); polling and reads get `GATEWAY_RATE_LIMIT_READ_PER_MINUTE` (default 1200), so a browser tab that polls every few seconds no longer eats your chat budget. Both keys are declarative config-profile keys.
- **Sandbox.** Plugin scripts and terminal commands run under resource limits and a syscall filter (`crates/praxis-sandbox`): no `ptrace`, no module loading, no setuid, no cross-process memory. Write access is opt-in per declared directory. If the host cannot install a filter, the sandbox degrades to limits instead of killing every child.
- **Prompt injection.** Search and other third-party results are data, not instructions. The `safe_web_search` skill that ships with the Brave plugin states the policy, and the plugin reports provenance and flags content that is shaped like an instruction.

## In the terminal

`/help` lists the commands, including those added by installed packages. The header shows how full the model's context window is. The ones that make it feel like a tool rather than a form: `/state` (where the workflow is and what it can do next), `/changes` (who changed what in the context), `/sessions` and `/session <n>` (Alt+digit switches, or Ctrl+digit when your terminal keeps Alt for its own tabs), `/copy` then `v` to mark lines and `y` to yank just the mark, and long tool output folds to a few preview lines instead of vanishing.

## Reading order

- [`docs/archive/README.md`](docs/archive/README.md) — the full feature list and setup.
- [`docs/WORKFLOW_GRAPHS.md`](docs/WORKFLOW_GRAPHS.md) — states, transitions, guards.
- [`docs/POML_WORKFLOWS.md`](docs/POML_WORKFLOWS.md) — workflows, templates, `[limits]` and `[on]` rules.
- [`docs/CONTEXT_EVENTS.md`](docs/CONTEXT_EVENTS.md) — the context change log and event rules.
- [`docs/PLUGIN_LIFECYCLE.md`](docs/PLUGIN_LIFECYCLE.md), [`docs/PLUGIN_TERMINAL.md`](docs/PLUGIN_TERMINAL.md) and [`docs/XIS_PACKAGE_MANAGER.md`](docs/XIS_PACKAGE_MANAGER.md) — packages and installs.
- [`docs/PLUGINIZATION_HANDOFF.md`](docs/PLUGINIZATION_HANDOFF.md) and [`plan/README.md`](plan/README.md) — architecture and what is still open.
- [`docs/SKILLS.md`](docs/SKILLS.md), [`docs/EXECUTION_CONTRACTS.md`](docs/EXECUTION_CONTRACTS.md), [`docs/DECISION_IR.md`](docs/DECISION_IR.md) — skills, receipts, decision routing.
