# Installation presets

The `compatibility` preset keeps the source distribution's previous feature set
through the plugin migration. There are two separate operations:

| Operation | Command | Result |
|---|---|---|
| Build the normal distribution | `cargo build --release --locked --features compatibility` | Includes native VM, dashboard and legacy file adapters; this is also the default build |
| Install its public assets | `praxis install-preset compatibility --directory INSTALLATION` | Fills missing workflows, prompts, skills, dashboard files, shipped plugin folders and noVNC runtime files |
| Update the dashboard | Add `--update-dashboard` | Replaces changed dashboard files with backups |
| Grant VM tool access once | Add `--enable-vm-tools --data-dir DATA_DIRECTORY` | Enables the 25 VM plugin tool flags in the selected data directory |
| Build without a linked VM engine | `cargo build --release --locked --no-default-features --features dashboard` | Excludes `praxis-vm` and the VM CLI; can use an installed VM worker |
| Headless runtime | `cargo build --release --locked --no-default-features` | Excludes dashboard, VM engine, legacy read/edit and shell code; gateway, CLI, POML/SM workflows and receipts remain |
| Headless with native legacy file tools | `cargo build --release --locked --no-default-features --features legacy_file_ops` | Keeps `read_file`/`edit_file`, without VM or dashboard |
| Headless with native shell tools | `cargo build --release --locked --no-default-features --features shell` | Keeps `execute_terminal`/`run_background`/`background_status`, without VM or dashboard |
| VM without dashboard | `cargo build --release --locked --no-default-features --features vm` | Native VM tools/CLI, no dashboard listener |

The installer does not start services, run QEMU, call providers, read `.env` or
unlock secrets. Without `--enable-vm-tools` it does not open/create a database or
change tool flags. Paths in `--data-dir` resolve relative to `--directory` unless
absolute; the default is `INSTALLATION/data`. **Supply your actual `DATA_DIR`**
when opting into VM flags for an existing installation. The command deliberately
does not infer this from a secrets/configuration file.

## Fresh installation

```bash
git clone https://github.com/wasser-quest/praxis.git
cd praxis
cargo build --release --locked --features compatibility
./target/release/praxis install-preset compatibility --directory "$PWD"
./target/release/praxis onboard --interactive
./target/release/praxis run
```

Dashboard: `http://localhost:1337`; gateway: `http://localhost:3537`. Both bind
`0.0.0.0` by default. Onboarding configures credentials/settings; the preset only
installs public assets. Node and the configured `POML_CLI` renderer remain separate
requirements. Python script plugins require `python3` and their configured
credentials/backends. The preset copies every shipped plugin helper rather than
only its `plugin.json`.

## Upgrade an existing installation

Stop Praxis using your existing process manager. From the updated source checkout:

```bash
cargo build --release --locked --features compatibility
PRAXIS_INSTALL=/absolute/path/to/existing/installation
./target/release/praxis install-preset compatibility \
  --directory "$PRAXIS_INSTALL" --update-dashboard
install -m 755 ./target/release/praxis "$PRAXIS_INSTALL/praxis"
cd "$PRAXIS_INSTALL"
./praxis run
```

Use your service manager to restart if applicable. Existing `.env`, data,
credentials, tool flags and operator-edited templates/manifests/helpers are
preserved. The dashboard update is backed up. Existing plugin implementations are
preserved too: compare them with the updated source when intentionally upgrading
a customized package. Refresh the browser so it loads the current dashboard code.

`ROOT_DIR` still selects installation assets; `WORKSPACE_DIR` selects the prepared
project for verified host actions. These are separate from `DATA_DIR`. Keep the
existing paths. If `PLUGINS_DIR` points elsewhere, install/copy each complete
shipped plugin folder there using `praxis plugin install SOURCE_FOLDER`; the
preset targets `INSTALLATION/plugins` and does not override `PLUGINS_DIR`.

## VM compatibility

Install QEMU on the host (`qemu-system-x86_64` or `qemu-system-aarch64`, plus
`qemu-img`). From the installation directory:

```bash
./praxis install-preset compatibility --directory "$PWD" \
  --enable-vm-tools --data-dir /actual/path/to/data
```

Set `VM_ENABLED=true` in `.env`, keep the desired `VM_MODE`, ensure the installed
`plugins/vm/plugin.json` has `enabled: true`, and restart. Existing VM flags remain
effective unless this explicit activation command overrides them. Once installed,
disable individual tools in Dashboard → Tools; restarting does not reenable them.

The VM retains `DATA_DIR/vm` and `DATA_DIR/shared`. Existing provider/admin
credentials are no longer exported to every guest. Configure explicit grants if
a guest needs a credential. See [VM package migration](VM_PLUGIN.md).

## Install the headless VM worker

To keep QEMU code outside the Praxis binary, build the worker and host separately.
Install QEMU utilities on the same machine. From the source checkout:

```bash
cargo build --release --locked -p praxis-vm-worker --bin praxis-vm-service
cargo build --release --locked -p praxis --no-default-features
PRAXIS_INSTALL=/absolute/path/to/installation
./target/release/praxis install-preset compatibility \
  --directory "$PRAXIS_INSTALL" --update-dashboard \
  --enable-vm-tools --data-dir /actual/path/to/data
mkdir -p "$PRAXIS_INSTALL/plugins/vm/bin"
install -m 755 ./target/release/praxis "$PRAXIS_INSTALL/praxis"
install -m 755 ./target/release/praxis-vm-service \
  "$PRAXIS_INSTALL/plugins/vm/bin/praxis-vm-service"
```

Keep existing data/workspace paths and add these to that installation's `.env`:

```dotenv
VM_ENABLED=true
VM_SERVICE_EXECUTABLE=plugins/vm/bin/praxis-vm-service
VM_MODE=shared
```

Then restart from the installation directory, for example:

```bash
cd "$PRAXIS_INSTALL"
./praxis run --no-dashboard --no-discord
```

Use `VM_MODE=vm` if agent legacy shell/file aliases should use the guest. The
installed manifest must be enabled and tool flags still apply. An absolute worker
path also works, including with a separate `PLUGINS_DIR`. The preset installs
public assets, not this executable: install/update the worker explicitly while
Praxis is stopped. A missing/incompatible worker fails setup without fallback or
automatic retry. A compatible worker can later be installed without a host rebuild.

For the VM dashboard and VNC, run `./praxis run --no-discord` without
`--no-dashboard`. Open `http://localhost:1337` (or HTTPS if configured), sign in,
and select the contributed **Virtual machines** page. The listener still binds
`0.0.0.0`. No provider credentials are needed for management. The worker embeds
its UI/noVNC assets and licenses; a core-only installation does not need to copy
a VM UI directory. Missing/disabled/unready VM packages contribute no page.

Update host and worker from the same checkout and refresh dashboard assets with
`--update-dashboard`; older workers lack the required `capture` control or `--cli`
entry point. `./praxis vm --help`, `./praxis vm status` and screenshot delivery now
work through the installed worker with a core-only host. See the complete
[operator commands and screenshot settings](VM_PLUGIN.md#operator-cli-and-screenshots).
The default native build retains its VM dashboard without
setting `VM_SERVICE_EXECUTABLE`. See [web contributions](PLUGIN_WEB_CONTRIBUTIONS.md)
and [protocol and lifecycle](PLUGIN_PROCESS_PROTOCOL.md).

## Optional capabilities and headless use

The core-only build omits native `read_file`/`edit_file`. To keep their code
outside the host, install `praxis-legacy-file-ops` independently:

```bash
./scripts/install-legacy-file-ops-package.sh /actual/PLUGINS_DIR
```

Restart Praxis with that `PLUGINS_DIR`. The same names and results become
available through the package; existing disabled per-tool choices are inherited
until explicitly overridden in plugin flags. The compatibility build already
links this crate and needs no separate file-tool install. Other tool packages
still ship in core while their extraction proceeds. See
[tool package installation and transport](TOOL_PACKAGES.md#independently-installed-legacy-file-operations).

The core-only build also omits native shell execution. Install the shell package
to keep `execute_terminal`, `run_background` and `background_status` outside the
host:

```bash
./scripts/install-shell-package.sh /actual/PLUGINS_DIR
```

Foreground `execute_terminal` then works through the package immediately after
restart. Durable `run_background`/`background_status` additionally need
`SHELL_SERVICE_EXECUTABLE=plugins/shell/bin/praxis-shell` (relative to `ROOT_DIR`
or absolute) so the host can bind the long-lived job worker. Existing per-tool
flags are inherited until explicitly overridden in plugin flags. See
[tool package installation and transport](TOOL_PACKAGES.md#independently-installed-shell-and-background-jobs).

Verified Rust is intentionally opt-in:

```bash
./praxis plugin install ./examples/plugins/verified-rust
./praxis run --workspace-dir /absolute/path/to/prepared/rust/project
```

Select the corresponding workflow and restart after installation. The
[reproducible Snake instructions](SNAKE_IR_TEST.md) include project creation,
dependency installation and the complete test prompt.

To run without the dashboard/Discord, keep the same installed workflows and use
`praxis run --no-dashboard --no-discord`. POML/context/SM/Decision IR/receipts stay
in core. To exclude the VM implementation from a build, use
`--no-default-features` and `VM_ENABLED=false`, or install the worker above.
This is a build without a linked VM engine, not yet
the final minimal plugin-only runtime; dashboard, providers and remaining tools
are extracted in subsequent plan steps.
