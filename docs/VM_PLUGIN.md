# VM package

`crates/praxis-vm` owns QEMU/QMP/serial operations, the 25 VM tool implementations,
screen capture and credential-directory preparation. It has no Praxis database,
provider, POML or dashboard dependency and does not read `DATA_DIR`, socket mode or
user preferences from global process state. The host supplies configuration,
authenticated invocation identity, user preferences and explicit credentials.

`plugins/vm/plugin.json` declares the tools through native invocation API v1.
`src/runtime/vm.rs` binds the implementation, selects the legacy execution backend
and supplies the user's keyboard/screenshot settings. CLI commands are contributed
by `src/runtime/vm/cli.rs`; the entry point registers them only in builds with VM.
Creating/registering the backend does not create guest directories or processes.
The optional `praxis-vm-service` worker initializes separately at host startup.
Guest names use 1–80 ASCII letters, digits, underscores, hyphens or dots; storage
directory names and `.`/`..` are reserved.

## Installation and compatibility

Use `--features compatibility` (the normal default) or `--features vm`, install
the public assets, configure `VM_ENABLED=true` and install QEMU. The compatibility
preset can explicitly enable the 25 VM tool flags. See the exact commands in
[installation presets](INSTALLATION_PRESETS.md).

Historical flags in `DATA_DIR/tools.json` remain effective until an explicit
plugin flag overrides them. The tools have one owner, `vm`; they are no longer
also executed as builtins. Disabled flags remain disabled on subsequent starts.
The host's selected backend is fixed in the registry snapshot. An unavailable or
disabled guest binding cannot fall back to running an agent command on the host.

Configured autostart still starts `praxis-vm`. A reachable existing guest is
reattached via QMP; the host no longer kills arbitrary processes by matching a VM
name. Cancelled, unfinished startup kills only its newly spawned child. Committed
guests and their disks survive native-service drain/host shutdown. A build without
VM plus `VM_ENABLED=true` requires an installed worker selected by
`VM_SERVICE_EXECUTABLE`; otherwise it fails setup before VM initialization.

For headless process hosting, build/install `praxis-vm-service` and set
`VM_SERVICE_EXECUTABLE` to an absolute path or a path relative to `ROOT_DIR`.
The same VM manifest/tools work with a core without the QEMU crate. Leaving
this setting unset preserves the native backend. Follow
[the installation commands](INSTALLATION_PRESETS.md#install-the-headless-vm-worker).

## Execution scope

VM operations return guest-scoped results, including the authenticated user/task,
the operation, output and `verified: false`. They cannot create a verified host
workspace receipt or satisfy host coding completion guards. Shell/network/package
effects may persist after cancellation; VM snapshots do not undo external network
effects. Native invocation deadlines cap a call at the declared 300-second budget.

In `VM_MODE=vm`, the agent's legacy terminal/read/write/edit aliases use the
registered guest backend. Chat retains host aliases. Contracted Rust capabilities
remain pinned to the host workspace. Workflows and tool flags control access;
shared folders intentionally expose their configured host resources.

Capture/retention is an optional VM-service hook. The agent consumes actual
returned screenshot artifacts. `settings.vm_keyboard_layout`,
`settings.vm_screenshot_enabled` and `settings.vm_screenshot_limit` are resolved
for the authenticated caller, with no `default`-user database fallback.

## Credential grants

Edit only these fields in the installed VM manifest, retaining its tool table:

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

Set `vm_git_token` through Dashboard → Secrets, then restart. Each mapping selects
a declared plugin secret and a credential filename for one guest. The default
mapping is empty. No automatic provider/gateway/admin export or environment loader
remains. Linux guests can mount the read-only `praxis-secrets` 9p share at
`/run/secrets` and read the required file. Mounting is a guest setup action.

Credential preparation removes obsolete files from that guest's credential
directory, including old blanket-injection files, but preserves disks/shared
files. Unix directories/files use `0700`/`0600`; symlinked credential paths and
invalid names fail before credential writes. Manual VM CLI starts have no implicit
credential grants; configured runtime autostart and native tools use the manifest.

## Transitional boundary and next step

The engine supports an optional **native Rust package** and an independently
installed **headless worker**. The [process protocol](PLUGIN_PROCESS_PROTOCOL.md)
handles health, caller scope, deadlines, cancellation and worker crashes. Stopped
instances are not restarted/replayed automatically; restart Praxis for a fresh
binding. Installing a manifest alone still does not install an executable.

VM ownership policy, routes/noVNC assets, the dashboard page and independent CLI
are the next slice. Current dashboard/delivery adapters obtain the native bound
instance through a host-only bridge; they are unavailable in process mode and
cannot lazy-initialize a manager. Automatic screenshot delivery through that
bridge remains native-only; worker tool results still retain screenshot paths.
The process option serves headless tools and configured autostart.
The legacy `/websockify` alias now uses the authenticated VNC adapter, and clients
send the dashboard token. Refresh updated dashboard assets after upgrade.

## Verification

```bash
cargo test -p praxis-vm --locked
cargo test -p praxis-plugin-api --locked
cargo test --lib --locked --no-default-features runtime::vm_process_tests
cargo test --lib --locked runtime::vm_tests
cargo test --lib --locked dashboard::routes::vm_tests
cargo test --lib --locked --no-default-features runtime::vm_tests
cargo tree -p praxis --no-default-features
```

Tests use a local QMP fixture and a controlled child-process stub, including
reattachment and cancelled-startup cleanup. They do not boot a real guest or use
live model inference. A real QEMU guest, OS installation and interactive noVNC
keyboard/mouse remain operator smoke checks.
