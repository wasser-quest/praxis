# Installation presets

The `compatibility` preset keeps the source distribution's previous feature set
through the plugin migration. There are two separate operations:

| Operation | Command | Result |
|---|---|---|
| Build the normal distribution | `cargo build --release --locked --features compatibility` | Includes the optional native VM crate; this is also the default build |
| Install its public assets | `praxis install-preset compatibility --directory INSTALLATION` | Fills missing workflows, prompts, skills, dashboard files, shipped plugin folders and noVNC runtime files |
| Update the dashboard | Add `--update-dashboard` | Replaces changed dashboard files with backups |
| Grant VM tool access once | Add `--enable-vm-tools --data-dir DATA_DIRECTORY` | Enables the 25 VM plugin tool flags in the selected data directory |
| Build without VM | `cargo build --release --locked --no-default-features` | Excludes `praxis-vm` and the VM CLI; other feature extractions remain future work |

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

## Optional capabilities and headless use

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
`--no-default-features` and `VM_ENABLED=false`. This is a build without VM, not yet
the final minimal plugin-only runtime; dashboard, providers and remaining tools
are extracted in subsequent plan steps.
