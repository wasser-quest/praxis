# Reproduce the verified Snake test

Run the preparation commands as the operator, before starting the agent task.
Use an updated Praxis checkout and binary. The project directory must be new;
if `ir-snake` already exists, keep its contents and choose another name for a
fresh reproduction.

From the Praxis repository directory:

```bash
PRAXIS_DIR="$(pwd)"
rustup component add rustfmt
cargo new --bin --edition 2021 "$PRAXIS_DIR/ir-snake"
cd "$PRAXIS_DIR/ir-snake"
cargo add ratatui@0.29 crossterm@0.28
cargo fmt
cargo build --locked
cargo test --locked
cd "$PRAXIS_DIR"
```

This produces an existing `src/main.rs`, its own `Cargo.toml` and `Cargo.lock`.
The initial test command only verifies the prepared stub; game tests are added
by the task below. Dependencies are installed before verified source editing.

Build the updated runtime and install its current opt-in Rust capability
manifest into the plugin directory that will be used at startup:

```bash
cargo build --release --locked
mkdir -p "$PRAXIS_DIR/plugins/verified-rust"
cp "$PRAXIS_DIR/examples/plugins/verified-rust/plugin.json" \
   "$PRAXIS_DIR/plugins/verified-rust/plugin.json"
PLUGINS_DIR="$PRAXIS_DIR/plugins" "$PRAXIS_DIR/target/release/praxis" plugin list
```

Expect `verified_rust v1.2.1 [enabled]` with three tools. Keep exactly one
enabled owner for each tool name. In the dashboard's Tools view, enable
`execute_decision`, `inspect_file`, `agent_next`, `agent_back`, `agent_complete`,
`modify_source`, `build_workspace` and `run_workspace_tests`.

After stopping the running Praxis instance, start the updated binary with
explicit installation, plugin and project locations:

```bash
ROOT_DIR="$PRAXIS_DIR" PLUGINS_DIR="$PRAXIS_DIR/plugins" \
  "$PRAXIS_DIR/target/release/praxis" run \
  --workspace-dir "$PRAXIS_DIR/ir-snake"
```

Set the **current chat's** `settings.sm_file` and top-level `sm_file` to
`branching-coding`, and both active-state fields to `route`. Start a new task.
The workflow selects `graph-coding`, its state-local opcode tables and its
task budgets. The configured chat model/provider is used, including Ollama;
there is no extra IR endpoint.

Paste this complete task prompt:

> Implement a playable Snake game in this prepared Rust binary project using
> Ratatui 0.29, Crossterm 0.28 and the standard library. Change only the existing
> src/main.rs; keep Cargo.toml and Cargo.lock and create no additional files.
> Use execute_decision with exactly one instruction per call and the current
> state's opcode table. Navigate branching-coding with N/K and from_state.
> Do not use direct file, terminal or capability calls.
>
> Provide a bordered board, visible snake, food and score; arrow keys/WASD;
> P for pause, R for restart, Q/Escape to quit; automatic movement about every
> 120 ms; growth and points on eating; wall/self collision; no direct reversal;
> deterministic food placement on free cells; a clear small-terminal message;
> and terminal restoration on exit and errors. Separate game logic and terminal
> presentation inside the file. Add terminal-free unit tests for movement,
> growth, wall/self collision, reversal prevention, pause and restart.
>
> Read src/main.rs with R and use its actual SHA-256. Replace it with the full,
> correctly formatted implementation using M. Inspect the real outcome and
> receipts. If a modification fails, read the current source before correcting
> it. Obtain fresh successful B and T receipts after the last modification.
> Go through review to done and use C only when current verified modification,
> build and test receipts satisfy the guards. Include a short usage guide and
> actual checked results as text alongside C. I will check the interactive
> display myself. If Praxis reports a non-retryable setup error, report its
> operator action and stop; do not guess keys, owners or another workspace.

After verified completion, play it yourself:

```bash
cargo run --locked --manifest-path "$PRAXIS_DIR/ir-snake/Cargo.toml"
```

In Graphs, inspect the visited route; in Messages, inspect actual model prompts,
tool calls, usage and returned execution receipts. A model's statement that it
compiled or ran tests is insufficient evidence. M's postcondition checks
formatting and compilation; B and T provide separate build/test receipts.

To try the same capabilities through normal role routing, select
`standard-verified` with state `code` and `sm_data.role = "code"`, keeping the
same project/plugin setup. Ask for the same game through the current state's
R/M/B/T/C table. This profile requires completion evidence once source editing
has been attempted; read-only planning and conversation do not require an edit.
