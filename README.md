<p align="center">
  <img src="static/logo.png" alt="Praxis" width="112" height="112">
</p>

# Praxis — A runtime for AI agent workflows

Run agents on your own machine, with your chosen models and explicit workflows. Praxis combines state machines, POML prompts, tools and plugins with opt-in verified actions: contracts, rollback and execution receipts that can guard completion. Chat through the web dashboard, terminal or Discord.

**Website**: [getpraxis.boo](https://getpraxis.boo)

## Features

- **Multi-Provider LLM**: OpenAI, Codex (ChatGPT subscription), Anthropic, Ollama, llama.cpp, OpenRouter, MiniMax, MiMo
- **Discord Bot**: Chat via Discord with voice, channels, threads
- **Web Dashboard**: Monitor users, messages, secrets, VM, cron jobs
- **Tool Calling**: Shell, file ops, context, agent control, Discord
- **QEMU VM**: Full Linux VM controlled by the LLM (keyboard, mouse, screenshots)
- **Everything is a package**: tools, providers, media, channels, frontends and asset packs install as packages; the kernel keeps authority, receipts and guards ([plugin lifecycle](docs/PLUGIN_LIFECYCLE.md))
- **Setups with `xis`**: signed repositories install whole setups — plugins, workflows, personas and a config profile — with a plan first, an ownership lock and a change report ([xis](docs/XIS_PACKAGE_MANAGER.md))
- **POML Templates**: Customizable system prompts and workflows
- **Memory & Learning**: User-private [memory profiles](docs/MEMORY_PROFILES.md) per persona, with separate rare shared identity facts
- **Encrypted Secrets**: AES-256-GCM encrypted at rest (enc2)
- **State machines (.sm)**: File-driven roles, prompts, tool access and guarded transitions
- **Decision router**: Select declared states with a configurable probability threshold, including Ollama System One
- **Verified execution**: Opt-in action contracts, pre/postcondition checks, transactional source edits and task-owned receipts
- **Decision IR**: Compact instructions mapped to trusted capabilities, with an opcode table per state; [verified Rust setup](docs/DECISION_IR.md)
- **Workflow graphs**: Indexed `agent_next`, task-owned `agent_back`, verified transitions and a dashboard graph explorer; [branching example and setup](docs/WORKFLOW_GRAPHS.md)
- **Verified coding roles**: Opt-in `standard-verified` keeps normal role routing with semantic Rust actions and completion evidence after source-edit attempts; [setup](docs/VERIFIED_CODING_ROLES.md), [reproducible Snake test](docs/SNAKE_IR_TEST.md)
- **Learning graph**: Interactive language lessons wait for real user input before feedback and reuse scoped tutor memory; [language-learning flow](docs/LANGUAGE_LEARNING_FLOW.md)
- **Execution timeline**: Actual system prompts, prompt/context changes, receipts, token budgets and generation metrics in the dashboard
- **Voice**: STT (Vosk/Whisper/ElevenLabs) + TTS (SAPI/ElevenLabs/Qwen)
- **RAG**: Vector store with embedding search
- **Cron Jobs**: Scheduled agent tasks

## Requirements

- A recent stable Rust toolchain (Rust and Cargo); build with the committed lockfile
- SQLite is bundled by the Rust dependency; no separate SQLite server is required
- For VM: `qemu-system-x86_64`, `qemu-img` (apt: `qemu-system-x86`)
- For voice: Vosk or Whisper models

## Network access and authentication

Praxis listens on **`0.0.0.0`** by default, so both HTTP services are reachable on the host's network interfaces. The dashboard and gateway are separate services:

| Service | Default address | Authentication |
|---------|-----------------|----------------|
| Dashboard (web UI) | `0.0.0.0:1337` | Admin password; authenticated sessions for protected APIs |
| Gateway (API / TUI) | `0.0.0.0:3537` | Gateway API key for protected endpoints |

Open **`http://localhost:1337`**, or `http://<server-hostname>:1337` from another machine. `0.0.0.0` is the bind address, not the address to type into a remote browser. `DASHBOARD_PORT` and `GATEWAY_PORT` override the ports. Enable `DASHBOARD_TLS=true` for the dashboard's built-in TLS, or terminate TLS at your reverse proxy.

Keep these services behind a firewall, VPN or authenticated TLS reverse proxy. Tool access can modify files and execute commands with Praxis's process permissions. Login and public dashboard assets remain accessible before authentication. Provider credentials are encrypted at rest; requests send credentials to the configured provider endpoint.

For dashboard access through an SSH tunnel:

```bash
ssh -L 1337:localhost:1337 user@your-server
# Open http://localhost:1337
```

Forward port `3537` separately if you also need the gateway API or a remote TUI.

## Quick Start

```bash
# Clone and build
git clone https://github.com/wasser-quest/praxis.git
cd praxis
cargo build --release --locked --features compatibility

# Install the complete bundled distribution (preserves existing files)
./target/release/praxis install-preset compatibility --directory "$PWD"

# Interactive setup
./target/release/praxis onboard --interactive

# Run
./target/release/praxis run
# Dashboard: http://localhost:1337
# Gateway API: http://localhost:3537
```

### Install what you need

Everything is a package, and each kind has one obvious home:

| What | How |
| --- | --- |
| **Terminal UI** | `./scripts/install-tui-package.sh` — then `praxis chat` finds it beside the kernel, on `PATH`, or where `PRAXIS_TUI_EXECUTABLE` points |
| **Dashboard** | `./scripts/install-dashboard-package.sh` (or the compatibility preset) |
| **Tool packages** | `./scripts/install-{shell,legacy-file-ops,vision}-package.sh` |
| **Tool & media plugins** | `praxis plugin install ./plugins/<name>`, one at a time — or all at once: `praxis plugin install-default --preset examples/presets/shipped-plugins.json` |
| **Asset packs** | `praxis plugin install ./packages/asset_coding` (likewise `asset_learning`, `asset_persona`, `asset_workflow`) |
| **Whole setups** | `xis install repo/setup@version` |
| **Kernel-bundled** | nothing to install: `file_ops`, `runtime_control`, `memory`, `rag`, `cron`, `discord`, `interaction`, `skills`, `workflow_authoring` (`praxis plugin builtins`) |

Installing through `praxis plugin install` runs the lifecycle: manifest
validation, `PLUGIN_HOOKS` policy (`allow|ask|deny`), `praxis.lock.json`
records for `praxis plugin verify` — and it never enables tools or grants
trust on its own. Everything lands in `PLUGINS_DIR`: one directory to list,
verify and remove.

### Setups with xis

`xis` is a separate executable that links nothing of the kernel. You pin a
repository and its ed25519 signing key (`xis repo add … --key ed25519:…`), it
verifies the signed index and content-addressed artifacts, prints the whole
plan before a byte is written, and applies it with `keep` (default), `--backup`
or `--force`. `xis.lock.json` records what each setup owns, so upgrades and
removals touch only owned, unedited files — a file you changed is reported and
left alone.

Plugin packages are delegated to `praxis plugin install`, so the kernel keeps
its hooks policy, its lockfile and its trust store. What you must change
afterwards lands in one change report (`praxis motd`, also shown on
`praxis run`): required environment, applied settings, backups and the exact
`praxis plugin trust …` commands. Secrets are never written and grants are
never automatic.

```bash
xis repo add standard https://getpraxis.boo/repo --key-file standard.pub   # or --key ed25519:…
xis search vim
xis plan    standard/vim-states@1.2.0
xis install standard/vim-states@1.2.0
xis motd
```

Publishing one is the same shape from the other side: `xis keygen` writes the
secret key and prints the public key, artifacts are JSON bundles named by
their own SHA-256 under `artifacts/`, and `xis sign --key-file xis-repo.key
index.json` writes `index.sig`. Host the three anywhere static and put the
public key where operators can pin it — a public repository is just files plus
a signature, and the key grants nobody download access. See
[publishing a repository](docs/XIS_PACKAGE_MANAGER.md#publishing-a-repository).

### Keep the functionality from before pluginization

Use the **`compatibility`** build and installation preset. It includes the optional
native VM, legacy file and shell backends and installs the shipped tool plugins, their helper scripts,
noVNC runtime files, dashboard assets, templates, workflows and skills. The normal
build selects `compatibility` by default. Dashboard, legacy file and shell code are
optional at build time; independently installed dashboard, VM, legacy file and
shell packages can run with a core-only host. Voice/audio, the ComfyUI media
client and GPU routing are their own crates (`praxis-voice`, `praxis-comfyui`,
`praxis-gpu-router`), the Discord channel is `praxis-channel-discord` behind
its `ChannelHost` seam, and the common provider interfaces live in
`praxis-provider-api`; a core-only dependency tree carries none of Serenity,
songbird, hound, Vosk, Whisper, minimp3, symphonia, ratatui or crossterm.

To keep legacy file code outside the host while retaining `read_file` and
`edit_file`:

```bash
cargo build --release --locked -p praxis --no-default-features
./scripts/install-legacy-file-ops-package.sh /actual/PLUGINS_DIR
# Restart Praxis with PLUGINS_DIR set to that directory.
```

The compatibility build retains their native adapter. A headless build can
also link just that adapter with `--no-default-features --features legacy_file_ops`.
Package switches cannot enable omitted code; existing names, schemas and tool
flags are preserved. See [tool packages](docs/TOOL_PACKAGES.md).

To keep shell execution outside the host while retaining `execute_terminal`,
`run_background` and `background_status`:

```bash
cargo build --release --locked -p praxis --no-default-features
./scripts/install-shell-package.sh /actual/PLUGINS_DIR
# The manifest declares the background worker; the env var below is an
# optional override. Foreground execution needs no worker.
# export SHELL_SERVICE_EXECUTABLE=plugins/shell/bin/praxis-shell
# Restart Praxis with PLUGINS_DIR set to that directory.
```

`run_background` and `background_status` need the long-lived worker because the
job registry is process memory; the raw one-shot executable transport cannot
keep it across calls. The package manifest declares the worker, so the host
binds and starts it when the package is enabled. The worker receives the
authenticated caller from the
host, never from model arguments, and the host announces completions on the
owner's stream. A headless build can link just the native adapter with
`--no-default-features --features shell`.

For an existing installation, build the updated source, stop the running Praxis
service, then run the following from the source checkout. Replace the installation
path with the directory containing your existing `.env`, `templates/` and `data/`:

```bash
cargo build --release --locked --features compatibility
PRAXIS_INSTALL=/absolute/path/to/existing/installation
./target/release/praxis install-preset compatibility \
  --directory "$PRAXIS_INSTALL" --update-dashboard
install -m 755 ./target/release/praxis "$PRAXIS_INSTALL/praxis"
cd "$PRAXIS_INSTALL"
./praxis run
```

Restart your service instead of the final command if you use systemd, Docker or
another process manager. Refresh the browser after updating dashboard assets.
Existing `.env`, secrets, data, tool flags, prompts and plugin manifests are
preserved; replaced dashboard files are backed up. The preset fills missing files,
so it preserves custom or previously disabled manifests. It does not update an
existing customized plugin's implementation or enable its external backend.

**If you use VMs**, install QEMU on the Praxis host and explicitly grant model
access to the VM tools once:

```bash
./praxis install-preset compatibility --directory "$PWD" --enable-vm-tools
# With a custom DATA_DIR, also pass --data-dir /actual/path/to/data
```

Keep or add these settings to the installation's `.env`, then restart Praxis:

```dotenv
VM_ENABLED=true
VM_MODE=shared
```

The `vm` package manifest must be enabled in `plugins/vm/plugin.json`. Existing
VM disks and shared folders stay in `DATA_DIR/vm` and `DATA_DIR/shared`. Subsequent
starts preserve disabled tool flags; rerunning `--enable-vm-tools` deliberately
reenables all 25 VM tools. State/workflow tool allow-lists still apply. Credentials
are now explicit per-VM grants; see [VM setup and migration](docs/VM_PLUGIN.md).

For VM tools and the VM dashboard with QEMU outside the Praxis binary, install
`praxis-vm-service` and set `VM_SERVICE_EXECUTABLE` to its installed path. This
also works with `--no-default-features`. Follow [worker installation](docs/INSTALLATION_PRESETS.md#install-the-headless-vm-worker).
The worker supplies its VM page, administration API, VNC/noVNC assets, CLI commands
and screenshots. `praxis vm --help` and `praxis vm status` work with either backend,
including a core-only build. See [VM CLI and screenshot setup](docs/VM_PLUGIN.md#operator-cli-and-screenshots)
and [web contributions](docs/PLUGIN_WEB_CONTRIBUTIONS.md).

Python script plugins require `python3`. Configure their declared credentials and
servers through Secrets and the plugin's settings; copying a manifest does not
install QEMU, provision ComfyUI, or log in to a provider. Verified Rust capabilities
remain an explicit opt-in:

```bash
./praxis plugin install ./examples/plugins/verified-rust
# Restart, select a verified workflow, and set WORKSPACE_DIR to the prepared project.
```

POML rendering still requires Node and your configured `POML_CLI`. Existing renderer
installations continue to work. The preset does not download the renderer; see
[POML setup](docs/POML_WORKFLOWS.md). POML, context handling, SM transitions,
Decision IR and receipts remain core functions. See [the complete compatibility
guide](docs/INSTALLATION_PRESETS.md) for custom directories and a build without VM.

### Repair missing onboarding files

With an updated binary, repair an older installation without reconfiguring or re-pairing:

```bash
/path/to/updated/praxis repair-assets --directory /path/to/installation --update-dashboard
```

This restores missing workflows, templates/includes, skills and icons. Existing prompts/settings/data remain unchanged; replaced dashboard files are backed up. Use the working directory of `praxis run`, then refresh the browser. Do not rerun interactive onboarding just for this. See [installation repair](docs/POML_WORKFLOWS.md#repair-an-incomplete-onboarding-installation).

### TUI selection, copy and paste

In `praxis chat`, **F2** toggles mouse capture for native terminal selection; **F3** enters whole-message copy mode. Use **Up/Down** to select, **Ctrl+C** or **y** to request clipboard copy, and **Esc** to return to your unchanged draft. **Ctrl+Q** always quits; Ctrl+C quits outside copy mode. PageUp/PageDown work in either mouse mode. Paste with your terminal's paste shortcut; multiline Unicode remains in the draft until sent. Clipboard requests use OSC 52 where supported, with native selection as the fallback. See [bindings, safety and terminal limitations](docs/TUI_SELECTION_COPY.md), or `/help` inside the TUI.

### Provider login from the TUI

Start the chat TUI (`praxis chat`, or `praxis chat --gateway-url https://host:3537 --gateway-key …` for a remote gateway) and use `/login`. Login always runs on the **gateway machine** — the process that talks to the model — so a remote TUI logs the remote backend in. The router is rebuilt in place; no restart.

The same client is available as a standalone `praxis-tui` executable
(`praxis-tui --gateway-url … --gateway-key …`, or without a URL for a local
gateway). `scripts/install-tui-package.sh` installs it as the `tui` frontend
package under `PLUGINS_DIR` so it can be distributed separately from the
kernel.

```
/login                                   # status of every provider + how to log in
/login codex                             # ChatGPT subscription via the Codex CLI (device auth on the gateway host)
/login codex --auth-json '<contents of auth.json>'  # JSON content, not a file path
/login openai sk-… [model] [api_base]    # same for anthropic, openrouter, minimax, mimo
/login ollama http://gpu-box:11434 qwen3:8b       # local servers: endpoint + model (also llamacpp)
/logout openai
```

After login, select **both** provider and model so an old local-model selection does not leak into Codex:

```text
/login codex
/context set settings.provider=codex settings.model=gpt-5-codex
/thinking medium
```

The CLI runs on the **gateway host**. Repeat `/login codex` to see delayed device codes; completion imports the login automatically. `/logout codex` cancels a pending login. An existing Praxis login is reused rather than overwritten with stale CLI tokens. To renew an expired/revoked login or change accounts, run `/login codex --device-auth` (bypasses old credentials), or import fresh tokens with `--auth-json`. `CODEX_HOME` is respected. `CODEX_MODEL` sets the default (`gpt-5-codex`); `USE_PROVIDER=codex` sets the gateway default **after** credentials have been saved. Codex maps thinking `off` to its minimum `low`; the original models also cap `xhigh` at `high`.

Codex requests reasoning summaries even with `/thinking auto`. Enable their separate display with `/show_thinking on`; summary headings and paragraphs are preserved. A completed reasoning-only response now continues the original task with request-local Codex state until an answer or valid tool calls arrive, within the existing `LLM_MAX_ATTEMPTS`, timeout and rate limits. Reasoning is never substituted for an answer, saved as assistant/tool history, or sent to TTS. Empty, malformed or incomplete responses still fail safely; exhausting the continuation budget reports a specific limit instead of an empty success.

Logins live in memory unless the gateway explicitly opts into master-key retention with **`PRAXIS_RETAIN_MASTER_KEY=1`** and a key supplied via `MASTER_KEY_FILE`/`MASTER_KEY`/startup prompt. Retention is **off by default**; enabling it keeps a zeroizing password allocation for the process lifetime so refresh tokens can persist. Alternatively, dashboard **Secrets → codex_auth** accepts the complete CLI `auth.json` (with master password to save it). GET only returns a mask; invalid JSON is rejected, an unchanged mask is ignored, and an empty API value removes the login. Provider changes take effect without restart. Do not paste credentials into ordinary chat.

Offline Codex contract tests run with `cargo test --lib codex`. Verification results and real-POML test commands are in [bugs.md](bugs.md#verification). **No live subscription request or real browser/device login was performed in this audit.** An explicit live smoke test is available (uses your subscription, never run automatically):

```bash
PRAXIS_CODEX_AUTH_FILE=/path/to/auth.json cargo test --lib codex_live_smoke -- --ignored
```

The smoke test uses only the supplied access token, with refresh disabled; it neither rotates nor saves credentials. Supply a fresh login if the token has expired.

Praxis resolves `templates/`, `contexts/`, `plugins/`, `skills/` from its installation root: `ROOT_DIR` if set, else the working directory when it contains `templates/`, else the executable's directory.

For verified host actions, set `WORKSPACE_DIR` to the prepared project (absolute,
or relative to `ROOT_DIR`). Installation assets remain at `ROOT_DIR`; leaving
`WORKSPACE_DIR` unset preserves the old root. Restart and start a new task after
changing it. You can also override the environment for this process:

```bash
./target/release/praxis run --workspace-dir /absolute/path/to/ir-snake
```

The bundled `verified-implementation` workflow checks for `Cargo.toml`, `Cargo.lock` and `src/` before making a model call. A binary/installation directory such as `target/release` is not a prepared coding project. `settings.path` does not change the verified root. See [Decision IR project setup and troubleshooting](docs/DECISION_IR.md#project-preflight).

### Provider application identity

Provider HTTP requests identify the application as **Praxis**, with **https://getpraxis.boo**, independently of the conversation's persona or `agent_name`.

- OpenRouter chat and the OpenRouter image plugin send `HTTP-Referer: https://getpraxis.boo` and `X-OpenRouter-Title: Praxis` automatically. No extra configuration is needed. These are OpenRouter's [app attribution headers](https://openrouter.ai/docs/app-attribution).
- Other built-in model, embedding, Decision and media clients send `User-Agent: Praxis/<version> (+https://getpraxis.boo)` (script plugins omit the version). This identifies the HTTP client; public app listings depend on the provider.

Update an installed OpenRouter image plugin by reinstalling its complete folder. Existing provider credentials and model selections continue to apply.

## Configuration (.env)

```env
# LLM Provider
USE_PROVIDER=openai          # openai | codex | anthropic | ollama | llamacpp | minimax | mimo | openrouter
OPENAI_API_KEY=sk-...
OPENAI_MODEL=gpt-4o
# OPENAI_API_BASE=https://api.openai.com/v1

# Gateway
GATEWAY_PORT=3537
GATEWAY_API_KEY=<auto-generated>

# Dashboard
DASHBOARD_PORT=1337
DASHBOARD_ADMIN_PASSWORD=<auto-generated>

# Installation and verified project (optional)
# ROOT_DIR=/path/to/praxis-installation
# WORKSPACE_DIR=/path/to/prepared-project

# Data
DATA_DIR=./data
RUST_LOG=info

# VM (optional)
VM_ENABLED=true              # Enable QEMU VM support
# Optional installed worker; omit to use the native VM backend:
# VM_SERVICE_EXECUTABLE=plugins/vm/bin/praxis-vm-service
VM_MODE=shared               # shared = host tools + guests; vm = guest backend for agent file/shell aliases
VM_CPU_CORES=2
VM_RAM_MB=4096
VM_DISK_SIZE=40G
VM_ARCH=x86_64               # x86_64 | aarch64

# Discord (optional)
DISCORD_BOT_TOKEN=...
DISCORD_APPLICATION_ID=...

# Voice (optional)
VOICE_STT_TYPE=vosk          # vosk | whisper | elevenlabs
VOICE_TTS_TYPE=windows_sapi  # windows_sapi | elevenlabs | qwen_tts
```

### Long-running LLM tasks

LLM calls share bounded retries, provider/account concurrency limits, optional RPM/TPM pacing, and a total time budget. Management starts even when `USE_PROVIDER` is not configured: health, authenticated context/status and provider login remain available. Chat/agent tasks return a non-retryable setup error before changing workflow context or history until their selected provider is configured. `/api/status` reports inference configuration readiness separately from process health; it does not probe endpoint/model availability. **No implicit fallbacks**; opt in with `LLM_FALLBACK_PROVIDERS`. Defaults are five attempts, one concurrent request, a 180-second attempt timeout and a 300-second total budget per LLM call. Tool effects are not replayed by retries; stop remains responsive during LLM waits.

See [LLM resilience and safe rollout](docs/LLM_RESILIENCE.md) for configuration, streaming safety, tests, and the Ollama working-directory repair.

### TUI layout and troubleshooting

The TUI preserves newlines, blank lines and indentation in messages (including
pasted text), tool output and status/error banners. Use **Shift+Enter** for a
newline, **PageUp/PageDown** or the mouse wheel to scroll. Streaming follows the
bottom only while you are at the bottom; the last reply has space above the
composer so its final line and border remain visible. Completion popups stay
above the input box, including on small terminals. Finished reply bubbles remain
visible while saved history catches up. A disconnected stream keeps its partial
reply (marked interrupted) until the authoritative reply arrives, rather than
making the message box disappear or appending potentially missing fragments.

For provider diagnostics, enable debug logging on the **gateway** process:

```bash
RUST_LOG=warn,praxis::gateway::llm=debug praxis run
RUST_LOG=warn,praxis::tui=debug praxis chat
```

Daily logs are written under `LOG_DIR` (default `./logs`):
`praxis.log.YYYY-MM-DD` on the gateway and `praxis-tui.log.YYYY-MM-DD` for the
TUI. TUI logs never go to its terminal. Provider diagnostics include attempt
number, timing, response/event counts, failure category and safe request IDs,
not raw response bodies, prompts, tokens or tool arguments. Include the specific
error and request ID when reporting failures; do not share credentials.

Codex errors distinguish malformed SSE/JSON, interrupted streams, incomplete
responses, reasoning-only output, and invalid tool calls. An unfinished stream
never executes its tool previews. Codex's subscription endpoint does not accept
an increased `max_tokens` budget, so output-limit failures ask for a shorter
response rather than replaying the same request repeatedly.

### GPU-Router-Integration (pgpu, Bauplan §12.4)

Wenn Praxis seine LLM-/TTS-/STT-Backends über den pgpu-GPU-Router erreicht
(`LLAMACPP_API_BASE`/`COMFYUI_BASE_URL`/`VOSK_SERVER_URL` auf die Router-IP,
Ports identisch zum Direktbetrieb):

```env
GPU_ROUTER_URL=http://100.105.6.69:8080   # Dashboard/API des Routers
GPU_ROUTER_TOKEN=<ROUTER_TOKEN>           # gleicher Token wie im Router-Deploy
GPU_ROUTER_WAIT_S=120                     # Hold-Fenster auf kaltem Slot (Default 120)
```

Ohne `GPU_ROUTER_URL` sind alle Funktionen No-Ops (lokaler Betrieb bleibt
unberührt). Damit:
- hält der Router kalte Slots über `X-Router-Wait` bis healthy wach
  (impliziter Wake) — der erste Chat nach Idle-Stopp geht durch, statt am
  Retry-Backoff zu scheitern,
- antwortet der ComfyUI-Proxy auf kaltem media-Slot mit 503 +
  `X-Router-State`; Praxis stößt Wake an und scheitert schnell + klar
  („GPU-Slot warming — kein Job übermittelt“) statt irrelevanter
  Server-Fehler-Meldungen,
- hält `X-Router-Job-Id` den Slot über die Lücken zwischen TTS-Sätzen
  eines Antwortblocks busy (kein Idle-Stop mittendrin),
- wärmt der media-Slot preemptiv, während die Antwort noch generiert
  (nur wenn die Antwort im Kanal wirklich gesprochen wird).

### VPS-/Container-Deploy (mit GPU-Router + NetBird in einer Compose)

`bash deploy/build.sh [--push]` baut `vayayo/praxis` (Binary mit songbird,
Node + gebündeltes POML-CLI). Die komplette Stack-Compose (NetBird-Peer,
GPU-Router, STT, Praxis — ein Netzwerk-Namespace, kein Port-Publishing)
liegt im pgpu-Repo: `pgpu/deploy/vps-compose.yml`, Anleitung
`pgpu/deploy/vps/README.md`. Sicherheitsmodell dort: Secrets nur im
verschllüsselten Store (Dashboard-Settings), Master-Key-Zustellung per
Root-Entrypoint → tmpfs → lesen+löschen (nie in env/argv — die Agent-Shell
im shared VM-mode kann Store und Key nicht lesen), Erststart legt headless
einen leeren Store an (`MASTER_KEY_FILE`, `SECRETS_DIR`).

## VM Mode

The optional `vm` package provides 25 QEMU tools. Build with `compatibility` or
`vm`, or install its headless worker for a core without QEMU. Install the assets,
set `VM_ENABLED=true`, and enable the required
tool flags. The workflow also controls which tools the model can use. Common tools:

| Tool | Description |
|------|-------------|
| `vm_start` | Start/create a VM |
| `vm_stop` | Stop a VM |
| `vm_shell` | Execute shell command in VM |
| `vm_keys` | Send keyboard input (supports special keys, ctrl+, F-keys) |
| `vm_mouse` | Mouse control (move, click, double-click, drag, scroll) |
| `vm_screenshot` | Capture VM display and save an image artifact |
| `vm_file_transfer` | Transfer files to/from VM via shared folder |
| `vm_snapshot` | Create VM snapshot |
| `vm_shared_folder` | Add shared host↔VM folder |

### VM_MODE=shared vs vm

- **shared**: Enabled VM tools operate on guests; legacy file/shell tools operate on the host.
- **vm**: In agent mode, `execute_terminal`, `read_file`, `write_file` and `edit_file` use the registered guest backend. Chat retains its host behavior. Verified Rust capabilities still operate on the pinned host workspace when the workflow allows them. VM mode is an execution-backend choice; shared folders and tool allow-lists determine the available host access. A disabled VM binding fails guest calls instead of falling back to host execution.

### Auto-Screenshot

After a successful VM tool call, the optional VM service can capture the screen. The agent uses the returned image artifact in its next context. Disabling the package removes this hook; unavailable screenshots are not reported as successful captures.

Context settings to control this:
```json
{
  "settings": {"vm_screenshot_enabled": true}
}
```

### Secrets in VM

The VM package starts with **no credential grants**. Declare the required secret
names and a per-guest mapping in `plugins/vm/plugin.json`, then configure the
secret value through the dashboard and restart. Example manifest fields:

```json
{
  "secrets": ["vm_git_token"],
  "context": {
    "credential_grants": {
      "praxis-vm": { "VM_GIT_TOKEN": "vm_git_token" }
    }
  }
}
```

Only that guest's explicitly granted files are exposed by the read-only
`praxis-secrets` 9p share. Inside a Linux guest, mount it and read the needed file:

```bash
sudo mkdir -p /run/secrets
sudo mount -t 9p -o trans=virtio,version=9p2000.L praxis-secrets /run/secrets
cat /run/secrets/VM_GIT_TOKEN
```

The old blanket injector and environment loader are removed. Preparing a guest
start removes obsolete credential files, including old provider/admin exports;
disks and ordinary shared files are preserved. See [VM credential grants](docs/VM_PLUGIN.md#credential-grants).

### Shared Folders

The host exports `{DATA_DIR}/shared/` through the `praxis-shared` 9p share. Mount
it at `/mnt/shared` in the guest (or configure its guest startup to do so):

```bash
sudo mkdir -p /mnt/shared
sudo mount -t 9p -o trans=virtio,version=9p2000.L praxis-shared /mnt/shared
```

Use `vm_shared_folder` to configure additional shares.

## Dashboard

Access at `http://localhost:1337` (or your `DASHBOARD_PORT`).

### TLS (HTTPS) — Required for noVNC

noVNC (the web-based VNC viewer) requires a secure context (TLS) for keyboard/mouse input. Enable with `DASHBOARD_TLS=true`:

```env
DASHBOARD_TLS=true
```

On first startup, a self-signed certificate is generated at `./data/tls/cert.pem` with SANs for `localhost`, `127.0.0.1`, your hostname, and your local IP.

**Trust the cert (Linux):**
```bash
sudo cp ./data/tls/cert.pem /usr/local/share/ca-certificates/praxis.crt
sudo update-ca-certificates
```

Then access via `https://<your-ip>:1337`.

**Browser workaround** (no trusting needed):  
Add `https://<your-ip>:1337` as a secure origin:
- **Chrome**: `chrome://flags/#unsafely-treat-insecure-origin-as-secure`
- **Firefox**: `about:config` → `network.websocket.allowInsecureFromHTTPS` = `true`

Tabs:
- **Overview**: Version, users, messages, uptime
- **Users**: Manage user contexts
- **Conversations**: Chat history per user
- **Secrets**: Manage encrypted secrets (custom key-value pairs)
- **Skills**: Upload/view skill definitions
- **Settings**: Dashboard admin password
- **Models**: LLM provider config
- **Logs**: Application logs
- **Pairings**: Discord user pairings
- **VM**: Start/stop VMs, VNC view, activity log with all LLM tool calls
- **Statemachine**: Workflow state machine files (.sm)

## Tool Calling

**The LLM receives the FULL tool response by default**, not a 2,000-character
preview. Every builtin/plugin tool schema accepts optional `_output` controls
for a line range, tail, literal search, JSON field or explicit paging. These are
model-facing parameters, consumed by the gateway before tool execution.
`read_tool_result` can inspect the saved response again without replaying an
action. See [Tool output selection](docs/TOOL_OUTPUTS.md) for examples, scope,
retention and explicit capture limits. Image content parts remain separate.

The LLM has access to these built-in tools:

| Tool | Description |
|------|-------------|
| `execute_terminal` | Run shell command on host (or VM if VM_MODE=vm) |
| `write_file` | Create/overwrite file |
| `edit_file` | Find/replace in file |
| `read_file` | Read file contents |
| `read_tool_result` | Full/selected saved tool response without re-execution |
| `search_skills` | Search bounded metadata results; strategy is configured in POML |
| `use_skill` | Lazily load one permitted skill's instructions with explicit parameters |
| `update_template` | Strictly validate and save a POML template without overwriting on render failure |
| `get_context` | Read user context |
| `set_context` | Set context variable |
| `delete_context` | Delete context variable |
| `agent_next` | Advance workflow step |
| `agent_complete` | Mark task done |
| `agent_set_path` | Set working directory |
| `agent_feedback` | Send progress message |
| `learn_fact` | Store a fact |
| `learn_preference` | Store user preference |
| `learn_topic` | Track conversation topic |
| `discord_send_message` | Send Discord message |
| `discord_send_embed` | Send Discord rich embed |
| `discord_upload_file` | Upload file to Discord |

## Context Settings

Control behavior per user with typed settings and custom application data. Merge only the intended leaves; do not replace unrelated saved values:

```json
{
  "settings": {
    "sm_file": "standard",
    "system_template": "standard",
    "active_skill": null,
    "vm_screenshot_enabled": true,
    "history_with_toolcalls": true,
    "history_token_limit": 500000,
    "compaction_enabled": true,
    "compaction_token_limit": 500000
  },
  "custom_data": {"user_template": "user"}
}
```

See the [complete context reference](docs/CONTEXT_VARIABLES.md). Legacy `cl_file` and `cl_data` inputs are normalized to canonical `sm_file` and `sm_data`; stored workflow data is preserved.

## Templates (POML)

Customize LLM behavior with POML templates in `templates/`:

- `standard.poml` — Default general assistant with a semantic task blueprint
- `language_instructor.poml` — Configurable language practice
- `code_assistant.poml`, `researcher.poml` — Focused personas
- `blueprints/*.json`, `shared/*.poml` — Customizable semantic roles, skills/tools/memory and current-input context
- `system.poml`, `language_learning.poml`, `roles/*.poml`, `tasks/*.poml` — Compatible names with valid, guarded POML
- `compaction.poml` — Evidence-preserving conversation summarization

The built-in **`poml_templates` skill** creates and validates templates using Microsoft's POML syntax. Ask Praxis to use it in agent mode. Configure `POML_CLI` to the installed Microsoft JavaScript CLI path; Node is required. The skill loads via `use_skill`, then guides the agent to save through `update_template`. Creating a template does not activate it.

Discovery is configurable through `templates/discovery/skills.poml`; the default prompt contains no full catalog. `search_skills` uses a persistent metadata index, then `use_skill` loads one workflow. Manifests support independent `skill_hidden` and `user_only` flags.

Use Discord `/skill skillname:skill_creator` to create skills through the existing POML authoring skill. The creator is user-only by default. `/skill skillname:off` clears selection and `/skill` browses a bounded page. Agent context tools/SM transitions cannot change persistent selection. See [Skills and POML discovery](docs/SKILLS.md) for configuration, indexing and media safety.

See [Skills and POML authoring](docs/SKILLS.md), [Semantic templates and workflows](docs/POML_WORKFLOWS.md), and the [workflow/UI change report](docs/WORKFLOW_UI_CHANGE_REPORT.md). `examples/poml-test-context.json` supplies complete synthetic render data; `scripts/test_poml_templates.py` strictly tests every shipped template.

## Statemachine (.sm)

Workflows live in `contexts/`. Canonical selection is `settings.sm_file` (highest priority), then root `sm_file`, otherwise `standard`. `.sm` files are preferred; `.cl` and `cl_file` inputs remain backward-compatible. Invalid explicit selections fail visibly.

Both message paths route **before** rendering, using the current `custom_data.user_prompt`. The default `standard.sm` switches profiles for explicit requests such as “Be a language instructor” or “Act as a researcher”; the selection persists. It does not override a manual template selection on every ordinary message. Edit routing rules in `.sm`, not Rust.

### Valid workflow example

```ini
@name "My workflow"
@version "1.0"
@steps [plan, coding, testing, done]

[state plan]
settings.system_template = "tasks/plan"

[state coding]
settings.system_template = "tasks/code"

[state testing]
settings.system_template = "tasks/test"

[state done]
settings.system_template = "tasks/done"
```

Assign with `/context set settings.sm_file=my_workflow`. `agent_next` follows `@steps`; a done state is not evidence that tests passed. State assignments currently use string-compatible values; use typed context tools for JSON booleans/objects.

Additional sections:

```ini
[transitions]
plan -> coding : when sm_data.approved == true

[auto]
sm_data.needs_review == true -> use testing

[overrides]
if custom_data.user_prompt =~ "(?i)^reset role$" -> settings.system_template = "standard"
```

Auto-rules are ordered (first match wins). Conditions support dotted paths, comparisons, regex `=~` / `!~`, and quote-aware logical operators. Tool history is under `custom_data.used_tools` / `custom_data.tool_history`. Regex backslashes are literal in `.sm`; do not double them as if writing JSON. The old inline `transition -> ... on next` and `auto_rule:` examples are not supported parser syntax.

Workflow routing does **not** mutate global credentials. Configure voice settings through the normal user context/secrets controls. Tags require `settings.tags_enabled`; tools retain their ordinary permission checks.

The dashboard can list/edit/create `.sm` files and still read legacy `.cl`. See [Semantic templates and workflows](docs/POML_WORKFLOWS.md) for customization, semantic-role examples, the complete synthetic test fixture and deployment details.

## Plugins

The [pluginization roadmap](plan/PLUGINIZATION.md) starts with VM support and
the full dashboard, then moves the remaining tools, providers and workflows
into optional packages. Its target is a small runtime with three builtin
`file_ops` tools. See the [delivery checklist](plan/README.md),
[the pluginization handoff](docs/PLUGINIZATION_HANDOFF.md) and
[architecture overview](docs/PLUGIN_FIRST_PLAN.md). Current plugins install
tools and native service bindings. The VM engine now lives in an optional Rust
package and an independently installed worker. The legacy `read_file`/`edit_file`
helpers and the shell tools now live in optional crates plus separately
installed packages; the shell background-job worker owns a process-memory job
registry the one-shot transport cannot. `understand_image` is now an optional
`vision` package as well. The VM package contributes its
API, VNC/noVNC assets and dashboard page through a generic authenticated host
proxy, including in a build without VM support. The package also owns `praxis vm`
commands and screenshot capture; the host forwards arguments and delivery through
the selected binding. Per-user guest ownership and upgrade/recovery policy, the
optional whole dashboard and generic process manifests remain planned work.

The [shared execution boundary](docs/PLUGIN_RUNTIME.md) is the first implemented
step: tool owners are checked consistently across discovery, workflow preflight
and chat/agent/Decision IR execution. User event delivery, template resolution
and catalog synchronization are now core services, with independently owned
retention, cron and shell workers. Each task pins its plugin declarations and
action receipts record the registry revision. Live tool disable flags still
apply. Provider clients initialize lazily so management works before LLM setup.
Native feature adapters can now bind service-backed tools with host-issued task
identity, workspace, cancellation, deadline and scoped storage. Contracts and
receipts stay in core. See [native invocation API v1](docs/NATIVE_FEATURE_SERVICES.md) and
[the VM package](docs/VM_PLUGIN.md). Its [process protocol](docs/PLUGIN_PROCESS_PROTOCOL.md)
adds versioned handshakes, bounded calls, cancellation and live crash detection;
failed instances cannot silently replay actions or fall back to host execution.

**POML, contexts and state machines remain in core.** Local workflows can run
without feature plugins or the dashboard. Full POML rendering requires Node and
`POML_CLI`; a workflow declaring a plugin capability still requires that plugin.
See [plugin-free workflows and regression checks](docs/CORE_WORKFLOWS.md).

Install plugins by placing a directory with `plugin.json` in `plugins/`:

```bash
./target/release/praxis plugin install ./my-plugin
./target/release/praxis plugin list
./target/release/praxis plugin uninstall my-plugin
```

Plugin manifest (`plugin.json`):
```json
{
  "name": "my-plugin",
  "version": "1.0.0",
  "enabled": true,
  "tools": [...],
  "context": {...},
  "secrets": ["MY_API_KEY"]
}
```

### Media plugins

- `plugins/elevenlabs_tts/`: `elevenlabs_tts` generates downloadable MP3 speech.
- `plugins/openrouter_image/`: `openrouter_image_generate` generates PNG/JPEG/WebP via OpenRouter's dedicated Image API.
- `plugins/minimax_image/`: `minimax_image_generate` and `minimax_image_analyze` generate and analyze images via the MiniMax image and vision APIs.

All three are standalone Python 3 plugins with encrypted-secret/environment support and local file output. They make paid API calls only when invoked, without automatic retries. See [setup, parameters and offline tests](docs/MEDIA_PLUGINS.md).

### ComfyUI workflows

`plugins/comfyui/` adds `comfyui_nodes`, `comfyui_workflow`, `comfyui_run`, and `comfyui_result` through the existing plugin system. Discover installed nodes/models, create complete graphs, edit nodes/connections, save/download JSON, and run workflows with explicit prompt, arbitrary local-file, and typed parameter bindings. Configure `COMFYUI_BASE_URL`; no Discord changes or automatic model installation. Editor JSON can be managed, but execution requires API format. Includes complete text-to-image/image-to-image templates and offline tests; live GPU execution still requires a configured server and compatible models. See [installation, API research and examples](plugins/comfyui/README.md).

## Service (systemd)

```bash
./target/release/praxis service install
sudo systemctl enable praxis
sudo systemctl start praxis
./target/release/praxis service logs -f
```

## Containers

This repository does not currently include a Dockerfile or a prebuilt container image. For a custom container, expose dashboard port `1337` and gateway port `3537`, persist the installation assets and data, and mount the verified project separately through `WORKSPACE_DIR`.

## API

### Gateway API (port 3537)

```bash
# Chat
curl -X POST http://localhost:3537/v1/chat \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"user_id":"user1","message":"Hello"}'

# Chat with file
curl -X POST http://localhost:3537/v1/chat \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -F "user_id=user1" \
  -F "message=Analyze this" \
  -F "file=@photo.jpg"

# Export user
curl http://localhost:3537/v1/users/user1/export \
  -H "Authorization: Bearer $GATEWAY_API_KEY"

# Import user
curl -X POST http://localhost:3537/v1/users/user1/import \
  -H "Authorization: Bearer $GATEWAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d @export.json

# Providers: status and login (what the TUI /login uses)
curl http://localhost:3537/v1/providers -H "Authorization: Bearer $GATEWAY_API_KEY"
curl -X POST http://localhost:3537/v1/providers/login \
  -H "Authorization: Bearer $GATEWAY_API_KEY" -H "Content-Type: application/json" \
  -d '{"provider":"ollama","api_base":"http://127.0.0.1:11434","model":"qwen3:8b"}'
```

### Dashboard API (port 1337)

Protected with session auth (login via browser).

## License

See LICENSE file.
